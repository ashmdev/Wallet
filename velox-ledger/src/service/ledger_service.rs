use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use prost_types::Timestamp;
use rust_decimal::Decimal;
use sqlx::PgPool;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::domain::{
    AccountStatus, AccountType, CreateAccountParams, CreateEntryParams, CreateTransactionParams,
    EntrySet, EntryType, TransactionEvent, TransactionEventType, TransactionStatus,
};
use crate::error::LedgerError;
use crate::proto::ledger_service_server::LedgerService;
use crate::proto::{self, *};
use crate::repository::{AccountRepository, IdempotencyRepository, TransactionRepository};
use crate::webhook::WebhookDispatcher;

pub struct LedgerServiceImpl {
    pool: PgPool,
    webhook: Arc<WebhookDispatcher>,
}

impl LedgerServiceImpl {
    pub fn new(pool: PgPool, webhook: Arc<WebhookDispatcher>) -> Self {
        Self { pool, webhook }
    }

    fn decimal_to_proto(d: Decimal) -> Option<proto::Decimal> {
        Some(proto::Decimal {
            value: d.to_string(),
        })
    }

    fn proto_to_decimal(d: Option<proto::Decimal>) -> Result<Decimal, Status> {
        let d = d.ok_or_else(|| Status::invalid_argument("Decimal value required"))?;
        d.value
            .parse::<Decimal>()
            .map_err(|_| Status::invalid_argument(format!("Invalid decimal: {}", d.value)))
    }

    fn proto_to_decimal_or_zero(d: Option<proto::Decimal>) -> Result<Decimal, Status> {
        match d {
            Some(d) => d
                .value
                .parse::<Decimal>()
                .map_err(|_| Status::invalid_argument(format!("Invalid decimal: {}", d.value))),
            None => Ok(Decimal::ZERO),
        }
    }

    fn datetime_to_timestamp(dt: chrono::DateTime<Utc>) -> Option<Timestamp> {
        Some(Timestamp {
            seconds: dt.timestamp(),
            nanos: dt.timestamp_subsec_nanos() as i32,
        })
    }

    fn proto_account_type(t: i32) -> Result<AccountType, Status> {
        match proto::AccountType::try_from(t) {
            Ok(proto::AccountType::Asset) => Ok(AccountType::Asset),
            Ok(proto::AccountType::Liability) => Ok(AccountType::Liability),
            Ok(proto::AccountType::Equity) => Ok(AccountType::Equity),
            Ok(proto::AccountType::Revenue) => Ok(AccountType::Revenue),
            Ok(proto::AccountType::Expense) => Ok(AccountType::Expense),
            _ => Err(Status::invalid_argument("Invalid account type")),
        }
    }

    fn account_type_to_proto(t: AccountType) -> i32 {
        match t {
            AccountType::Asset => proto::AccountType::Asset as i32,
            AccountType::Liability => proto::AccountType::Liability as i32,
            AccountType::Equity => proto::AccountType::Equity as i32,
            AccountType::Revenue => proto::AccountType::Revenue as i32,
            AccountType::Expense => proto::AccountType::Expense as i32,
        }
    }

    fn account_status_to_proto(s: AccountStatus) -> i32 {
        match s {
            AccountStatus::Active => proto::AccountStatus::Active as i32,
            AccountStatus::Frozen => proto::AccountStatus::Frozen as i32,
            AccountStatus::Closed => proto::AccountStatus::Closed as i32,
        }
    }

    fn transaction_status_to_proto(s: TransactionStatus) -> i32 {
        match s {
            TransactionStatus::Pending => proto::TransactionStatus::Pending as i32,
            TransactionStatus::Committed => proto::TransactionStatus::Committed as i32,
            TransactionStatus::Failed => proto::TransactionStatus::Failed as i32,
            TransactionStatus::Reversed => proto::TransactionStatus::Reversed as i32,
        }
    }

    fn entry_type_to_proto(e: EntryType) -> i32 {
        match e {
            EntryType::Debit => proto::EntryType::Debit as i32,
            EntryType::Credit => proto::EntryType::Credit as i32,
        }
    }

    fn proto_entry_type(t: i32) -> Result<EntryType, Status> {
        match proto::EntryType::try_from(t) {
            Ok(proto::EntryType::Debit) => Ok(EntryType::Debit),
            Ok(proto::EntryType::Credit) => Ok(EntryType::Credit),
            _ => Err(Status::invalid_argument("Invalid entry type")),
        }
    }

    fn domain_account_to_proto(a: crate::domain::Account) -> proto::Account {
        proto::Account {
            id: a.id.to_string(),
            external_id: a.external_id.unwrap_or_default(),
            name: a.name,
            currency: a.currency,
            account_type: Self::account_type_to_proto(a.account_type),
            status: Self::account_status_to_proto(a.status),
            balance: Self::decimal_to_proto(a.balance),
            available_balance: Self::decimal_to_proto(a.available_balance),
            pending_balance: Self::decimal_to_proto(a.pending_balance),
            version: a.version,
            metadata: serde_json::from_value(a.metadata).unwrap_or_default(),
            created_at: Self::datetime_to_timestamp(a.created_at),
            updated_at: Self::datetime_to_timestamp(a.updated_at),
        }
    }

    fn domain_transaction_to_proto(
        t: crate::domain::Transaction,
        entries: Vec<crate::domain::Entry>,
    ) -> proto::Transaction {
        proto::Transaction {
            id: t.id.to_string(),
            idempotency_key: t.idempotency_key,
            reference: t.reference.unwrap_or_default(),
            description: t.description.unwrap_or_default(),
            status: Self::transaction_status_to_proto(t.status),
            entries: entries.into_iter().map(Self::domain_entry_to_proto).collect(),
            error_message: t.error_message.unwrap_or_default(),
            metadata: serde_json::from_value(t.metadata).unwrap_or_default(),
            created_at: Self::datetime_to_timestamp(t.created_at),
            committed_at: t.committed_at.and_then(Self::datetime_to_timestamp),
        }
    }

    fn domain_entry_to_proto(e: crate::domain::Entry) -> proto::TransactionEntry {
        proto::TransactionEntry {
            id: e.id.to_string(),
            transaction_id: e.transaction_id.to_string(),
            account_id: e.account_id.to_string(),
            entry_type: Self::entry_type_to_proto(e.entry_type),
            amount: Self::decimal_to_proto(e.amount),
            description: e.description.unwrap_or_default(),
            balance_after: e.balance_after.and_then(Self::decimal_to_proto),
            metadata: serde_json::from_value(e.metadata).unwrap_or_default(),
            created_at: Self::datetime_to_timestamp(e.created_at),
        }
    }
}

#[tonic::async_trait]
impl LedgerService for LedgerServiceImpl {
    async fn create_account(
        &self,
        request: Request<CreateAccountRequest>,
    ) -> Result<Response<CreateAccountResponse>, Status> {
        let req = request.into_inner();

        if req.idempotency_key.is_empty() {
            return Err(Status::invalid_argument("idempotency_key is required"));
        }

        let account_type = Self::proto_account_type(req.account_type)?;
        let initial_balance = Self::proto_to_decimal_or_zero(req.initial_balance)?;

        let mut tx = self.pool.begin().await.map_err(LedgerError::from)?;

        // Check idempotency
        let account_id = Uuid::now_v7();
        let existing = IdempotencyRepository::check_and_set(
            &mut tx,
            &req.idempotency_key,
            "account",
            account_id,
            None,
        )
        .await
        .map_err(LedgerError::from)?;

        if let Some(record) = existing {
            tx.rollback().await.ok();
            let account = AccountRepository::get_by_id(&self.pool, record.resource_id)
                .await
                .map_err(LedgerError::from)?;
            return Ok(Response::new(CreateAccountResponse {
                account: Some(Self::domain_account_to_proto(account)),
                already_existed: true,
            }));
        }

        let params = CreateAccountParams {
            external_id: if req.external_id.is_empty() {
                None
            } else {
                Some(req.external_id)
            },
            name: req.name,
            currency: req.currency.to_uppercase(),
            account_type,
            initial_balance: Some(initial_balance),
            metadata: serde_json::to_value(&req.metadata).unwrap_or_default(),
        };

        let account = AccountRepository::create(&mut tx, &params)
            .await
            .map_err(LedgerError::from)?;

        tx.commit().await.map_err(LedgerError::from)?;

        Ok(Response::new(CreateAccountResponse {
            account: Some(Self::domain_account_to_proto(account)),
            already_existed: false,
        }))
    }

    async fn get_account(
        &self,
        request: Request<GetAccountRequest>,
    ) -> Result<Response<GetAccountResponse>, Status> {
        let req = request.into_inner();

        let account = match req.identifier {
            Some(get_account_request::Identifier::Id(id)) => {
                let uuid = Uuid::parse_str(&id)
                    .map_err(|_| Status::invalid_argument("Invalid account ID"))?;
                AccountRepository::get_by_id(&self.pool, uuid).await
            }
            Some(get_account_request::Identifier::ExternalId(external_id)) => {
                AccountRepository::get_by_external_id(&self.pool, &external_id).await
            }
            None => return Err(Status::invalid_argument("Account identifier required")),
        }
        .map_err(LedgerError::from)?;

        Ok(Response::new(GetAccountResponse {
            account: Some(Self::domain_account_to_proto(account)),
        }))
    }

    async fn get_balance(
        &self,
        request: Request<GetBalanceRequest>,
    ) -> Result<Response<GetBalanceResponse>, Status> {
        let req = request.into_inner();

        let uuid = Uuid::parse_str(&req.account_id)
            .map_err(|_| Status::invalid_argument("Invalid account ID"))?;

        let account = AccountRepository::get_by_id(&self.pool, uuid)
            .await
            .map_err(LedgerError::from)?;

        Ok(Response::new(GetBalanceResponse {
            account_id: account.id.to_string(),
            balance: Self::decimal_to_proto(account.balance),
            available_balance: Self::decimal_to_proto(account.available_balance),
            pending_balance: Self::decimal_to_proto(account.pending_balance),
            currency: account.currency,
            as_of: Self::datetime_to_timestamp(Utc::now()),
        }))
    }

    async fn list_accounts(
        &self,
        request: Request<ListAccountsRequest>,
    ) -> Result<Response<ListAccountsResponse>, Status> {
        let req = request.into_inner();

        let filter_type = if req.filter_type == 0 {
            None
        } else {
            Some(Self::proto_account_type(req.filter_type)?)
        };

        let filter_status = match proto::AccountStatus::try_from(req.filter_status) {
            Ok(proto::AccountStatus::Active) => Some(AccountStatus::Active),
            Ok(proto::AccountStatus::Frozen) => Some(AccountStatus::Frozen),
            Ok(proto::AccountStatus::Closed) => Some(AccountStatus::Closed),
            _ => None,
        };

        let filter_currency = if req.filter_currency.is_empty() {
            None
        } else {
            Some(req.filter_currency.as_str())
        };

        let page_token = if req.page_token.is_empty() {
            None
        } else {
            Some(req.page_token.as_str())
        };

        let (accounts, next_token, total) = AccountRepository::list(
            &self.pool,
            req.page_size,
            page_token,
            filter_type,
            filter_status,
            filter_currency,
        )
        .await
        .map_err(LedgerError::from)?;

        Ok(Response::new(ListAccountsResponse {
            accounts: accounts.into_iter().map(Self::domain_account_to_proto).collect(),
            next_page_token: next_token.unwrap_or_default(),
            total_count: total,
        }))
    }

    async fn post_transaction(
        &self,
        request: Request<PostTransactionRequest>,
    ) -> Result<Response<PostTransactionResponse>, Status> {
        let req = request.into_inner();

        if req.idempotency_key.is_empty() {
            return Err(Status::invalid_argument("idempotency_key is required"));
        }

        if req.entries.len() < 2 {
            return Err(Status::invalid_argument(
                "Transaction must have at least 2 entries (double-entry)",
            ));
        }

        // Check for existing transaction with same idempotency key
        if let Some(existing) =
            TransactionRepository::get_by_idempotency_key(&self.pool, &req.idempotency_key)
                .await
                .map_err(LedgerError::from)?
        {
            let entries = TransactionRepository::get_entries(&self.pool, existing.id)
                .await
                .map_err(LedgerError::from)?;
            return Ok(Response::new(PostTransactionResponse {
                transaction: Some(Self::domain_transaction_to_proto(existing, entries)),
                already_existed: true,
            }));
        }

        // Parse and validate entries
        let mut entry_params = Vec::with_capacity(req.entries.len());
        let mut account_ids: Vec<Uuid> = Vec::with_capacity(req.entries.len());

        for entry in &req.entries {
            let account_id = Uuid::parse_str(&entry.account_id)
                .map_err(|_| Status::invalid_argument("Invalid account ID in entry"))?;
            let entry_type = Self::proto_entry_type(entry.entry_type)?;
            let amount = Self::proto_to_decimal(entry.amount.clone())?;

            if amount <= Decimal::ZERO {
                return Err(Status::invalid_argument("Entry amount must be positive"));
            }

            account_ids.push(account_id);
            entry_params.push(CreateEntryParams {
                account_id,
                entry_type,
                amount,
                description: if entry.description.is_empty() {
                    None
                } else {
                    Some(entry.description.clone())
                },
                metadata: serde_json::to_value(&entry.metadata).unwrap_or_default(),
            });
        }

        // Validate double-entry balance
        let entry_set = EntrySet::new(entry_params.clone())
            .map_err(|e| Status::invalid_argument(e))?;

        // Parse expected versions for optimistic locking
        let expected_versions: HashMap<Uuid, i64> = req
            .expected_versions
            .iter()
            .filter_map(|(k, v)| Uuid::parse_str(k).ok().map(|id| (id, *v)))
            .collect();

        // Begin atomic transaction
        let mut tx = self.pool.begin().await.map_err(LedgerError::from)?;

        // Lock accounts in consistent order (sorted by ID to prevent deadlocks)
        let accounts = AccountRepository::get_multiple_for_update(&mut tx, &account_ids)
            .await
            .map_err(LedgerError::from)?;

        let accounts_map: HashMap<Uuid, _> = accounts.into_iter().map(|a| (a.id, a)).collect();

        // Validate expected versions (optimistic locking)
        for (account_id, expected_version) in &expected_versions {
            if let Some(account) = accounts_map.get(account_id) {
                if account.version != *expected_version {
                    tx.rollback().await.ok();
                    return Err(LedgerError::OptimisticLockError {
                        account_id: account_id.to_string(),
                        expected_version: *expected_version,
                        actual_version: account.version,
                    }
                    .into());
                }
            }
        }

        // Validate balances and account status
        for entry in entry_set.entries() {
            let account = accounts_map
                .get(&entry.account_id)
                .ok_or_else(|| LedgerError::AccountNotFound(entry.account_id.to_string()))?;

            if account.status != AccountStatus::Active {
                tx.rollback().await.ok();
                return Err(match account.status {
                    AccountStatus::Frozen => LedgerError::AccountFrozen(account.id.to_string()),
                    AccountStatus::Closed => LedgerError::AccountClosed(account.id.to_string()),
                    _ => LedgerError::ValidationError("Account not active".to_string()),
                }
                .into());
            }

            // Check sufficient balance for debits on asset/expense accounts
            if entry.entry_type == EntryType::Debit && !account.can_debit(entry.amount) {
                tx.rollback().await.ok();
                return Err(LedgerError::InsufficientBalance {
                    account_id: account.id.to_string(),
                    required: entry.amount.to_string(),
                    available: account.available_balance.to_string(),
                }
                .into());
            }
        }

        // Create the transaction record
        let transaction_params = CreateTransactionParams {
            idempotency_key: req.idempotency_key.clone(),
            reference: if req.reference.is_empty() {
                None
            } else {
                Some(req.reference)
            },
            description: if req.description.is_empty() {
                None
            } else {
                Some(req.description)
            },
            metadata: serde_json::to_value(&req.metadata).unwrap_or_default(),
        };

        let transaction = TransactionRepository::create(&mut tx, &transaction_params)
            .await
            .map_err(LedgerError::from)?;

        // Apply entries and update balances
        let mut entries = Vec::with_capacity(entry_set.entries().len());
        let mut updated_balances: HashMap<Uuid, Decimal> = HashMap::new();

        for entry_params in entry_set.into_entries() {
            let account = accounts_map.get(&entry_params.account_id).unwrap();

            // Calculate new balance using already updated balance or current balance
            let current_balance = updated_balances
                .get(&entry_params.account_id)
                .copied()
                .unwrap_or(account.balance);

            let new_balance = match entry_params.entry_type {
                EntryType::Debit => account.apply_debit(entry_params.amount),
                EntryType::Credit => account.apply_credit(entry_params.amount),
            };

            // Adjust if we've already modified this account in this transaction
            let adjusted_balance = if updated_balances.contains_key(&entry_params.account_id) {
                let delta = match entry_params.entry_type {
                    EntryType::Debit => {
                        if account.account_type.is_debit_normal() {
                            entry_params.amount
                        } else {
                            -entry_params.amount
                        }
                    }
                    EntryType::Credit => {
                        if account.account_type.is_credit_normal() {
                            entry_params.amount
                        } else {
                            -entry_params.amount
                        }
                    }
                };
                current_balance + delta
            } else {
                new_balance
            };

            updated_balances.insert(entry_params.account_id, adjusted_balance);

            let entry = TransactionRepository::create_entry(
                &mut tx,
                transaction.id,
                &entry_params,
                adjusted_balance,
            )
            .await
            .map_err(LedgerError::from)?;

            // Record balance history
            AccountRepository::record_balance_history(
                &mut tx,
                entry_params.account_id,
                entry.id,
                current_balance,
                adjusted_balance,
                account.version,
                account.version + 1,
            )
            .await
            .map_err(LedgerError::from)?;

            entries.push(entry);
        }

        // Update all account balances
        for (account_id, new_balance) in updated_balances {
            AccountRepository::update_balance_locked(&mut tx, account_id, new_balance)
                .await
                .map_err(LedgerError::from)?;
        }

        // Commit the transaction
        let transaction = TransactionRepository::commit(&mut tx, transaction.id)
            .await
            .map_err(LedgerError::from)?;

        tx.commit().await.map_err(LedgerError::from)?;

        // Dispatch webhook event (non-blocking)
        let event = TransactionEvent {
            event_type: TransactionEventType::TransactionCommitted,
            transaction_id: transaction.id,
            idempotency_key: transaction.idempotency_key.clone(),
            timestamp: Utc::now(),
            payload: serde_json::json!({
                "transaction_id": transaction.id.to_string(),
                "status": "committed",
                "entry_count": entries.len(),
            }),
        };
        self.webhook.dispatch(event).await;

        Ok(Response::new(PostTransactionResponse {
            transaction: Some(Self::domain_transaction_to_proto(transaction, entries)),
            already_existed: false,
        }))
    }

    async fn get_transaction(
        &self,
        request: Request<GetTransactionRequest>,
    ) -> Result<Response<GetTransactionResponse>, Status> {
        let req = request.into_inner();

        let transaction = match req.identifier {
            Some(get_transaction_request::Identifier::Id(id)) => {
                let uuid = Uuid::parse_str(&id)
                    .map_err(|_| Status::invalid_argument("Invalid transaction ID"))?;
                TransactionRepository::get_with_entries(&self.pool, uuid).await
            }
            Some(get_transaction_request::Identifier::IdempotencyKey(key)) => {
                let tx = TransactionRepository::get_by_idempotency_key(&self.pool, &key)
                    .await
                    .map_err(LedgerError::from)?
                    .ok_or_else(|| LedgerError::TransactionNotFound(key))?;
                let entries = TransactionRepository::get_entries(&self.pool, tx.id)
                    .await
                    .map_err(LedgerError::from)?;
                Ok(crate::domain::TransactionWithEntries::new(tx, entries))
            }
            None => return Err(Status::invalid_argument("Transaction identifier required")),
        }
        .map_err(LedgerError::from)?;

        Ok(Response::new(GetTransactionResponse {
            transaction: Some(Self::domain_transaction_to_proto(
                transaction.transaction,
                transaction.entries,
            )),
        }))
    }

    async fn list_transactions(
        &self,
        request: Request<ListTransactionsRequest>,
    ) -> Result<Response<ListTransactionsResponse>, Status> {
        let req = request.into_inner();

        let account_id = if req.account_id.is_empty() {
            None
        } else {
            Some(
                Uuid::parse_str(&req.account_id)
                    .map_err(|_| Status::invalid_argument("Invalid account ID"))?,
            )
        };

        let filter_status = match proto::TransactionStatus::try_from(req.filter_status) {
            Ok(proto::TransactionStatus::Pending) => Some(TransactionStatus::Pending),
            Ok(proto::TransactionStatus::Committed) => Some(TransactionStatus::Committed),
            Ok(proto::TransactionStatus::Failed) => Some(TransactionStatus::Failed),
            Ok(proto::TransactionStatus::Reversed) => Some(TransactionStatus::Reversed),
            _ => None,
        };

        let from_date = req.from_date.map(|t| {
            chrono::DateTime::from_timestamp(t.seconds, t.nanos as u32)
                .unwrap_or_else(|| Utc::now())
        });

        let to_date = req.to_date.map(|t| {
            chrono::DateTime::from_timestamp(t.seconds, t.nanos as u32)
                .unwrap_or_else(|| Utc::now())
        });

        let page_token = if req.page_token.is_empty() {
            None
        } else {
            Some(req.page_token.as_str())
        };

        let (transactions, next_token, total) = TransactionRepository::list(
            &self.pool,
            req.page_size,
            page_token,
            account_id,
            filter_status,
            from_date,
            to_date,
        )
        .await
        .map_err(LedgerError::from)?;

        // Fetch entries for each transaction
        let mut proto_transactions = Vec::with_capacity(transactions.len());
        for tx in transactions {
            let entries = TransactionRepository::get_entries(&self.pool, tx.id)
                .await
                .map_err(LedgerError::from)?;
            proto_transactions.push(Self::domain_transaction_to_proto(tx, entries));
        }

        Ok(Response::new(ListTransactionsResponse {
            transactions: proto_transactions,
            next_page_token: next_token.unwrap_or_default(),
            total_count: total,
        }))
    }

    async fn health_check(
        &self,
        _request: Request<HealthCheckRequest>,
    ) -> Result<Response<HealthCheckResponse>, Status> {
        // Check database connectivity
        let db_healthy = sqlx::query("SELECT 1")
            .fetch_one(&self.pool)
            .await
            .is_ok();

        let status = if db_healthy {
            health_check_response::ServingStatus::Serving
        } else {
            health_check_response::ServingStatus::NotServing
        };

        Ok(Response::new(HealthCheckResponse {
            status: status as i32,
            version: env!("CARGO_PKG_VERSION").to_string(),
            timestamp: Self::datetime_to_timestamp(Utc::now()),
        }))
    }
}
