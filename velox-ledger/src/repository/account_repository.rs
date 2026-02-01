use rust_decimal::Decimal;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::domain::{Account, AccountStatus, AccountType, CreateAccountParams};
use crate::error::{LedgerError, Result};

pub struct AccountRepository;

impl AccountRepository {
    pub async fn create(
        tx: &mut Transaction<'_, Postgres>,
        params: &CreateAccountParams,
    ) -> Result<Account> {
        params.validate().map_err(|e| LedgerError::ValidationError(e.to_string()))?;

        let initial_balance = params.initial_balance.unwrap_or(Decimal::ZERO);

        let account: Account = sqlx::query_as(
            r#"
            INSERT INTO accounts (
                external_id, name, currency, account_type, status,
                balance, available_balance, pending_balance, metadata
            )
            VALUES ($1, $2, $3, $4, $5, $6, $6, 0, $7)
            RETURNING
                id, external_id, name, currency, account_type, status,
                balance, available_balance, pending_balance, version,
                metadata, created_at, updated_at
            "#,
        )
        .bind(&params.external_id)
        .bind(&params.name)
        .bind(&params.currency)
        .bind(&params.account_type)
        .bind(AccountStatus::Active)
        .bind(initial_balance)
        .bind(&params.metadata)
        .fetch_one(&mut **tx)
        .await?;

        Ok(account)
    }

    pub async fn get_by_id(pool: &PgPool, id: Uuid) -> Result<Account> {
        let account: Account = sqlx::query_as(
            r#"
            SELECT
                id, external_id, name, currency, account_type, status,
                balance, available_balance, pending_balance, version,
                metadata, created_at, updated_at
            FROM accounts
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| LedgerError::AccountNotFound(id.to_string()))?;

        Ok(account)
    }

    pub async fn get_by_external_id(pool: &PgPool, external_id: &str) -> Result<Account> {
        let account: Account = sqlx::query_as(
            r#"
            SELECT
                id, external_id, name, currency, account_type, status,
                balance, available_balance, pending_balance, version,
                metadata, created_at, updated_at
            FROM accounts
            WHERE external_id = $1
            "#,
        )
        .bind(external_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| LedgerError::AccountNotFound(external_id.to_string()))?;

        Ok(account)
    }

    pub async fn get_for_update(
        tx: &mut Transaction<'_, Postgres>,
        id: Uuid,
    ) -> Result<Account> {
        let account: Account = sqlx::query_as(
            r#"
            SELECT
                id, external_id, name, currency, account_type, status,
                balance, available_balance, pending_balance, version,
                metadata, created_at, updated_at
            FROM accounts
            WHERE id = $1
            FOR UPDATE
            "#,
        )
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| LedgerError::AccountNotFound(id.to_string()))?;

        Ok(account)
    }

    pub async fn get_multiple_for_update(
        tx: &mut Transaction<'_, Postgres>,
        ids: &[Uuid],
    ) -> Result<Vec<Account>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }

        // Sort IDs to prevent deadlocks (consistent lock ordering)
        let mut sorted_ids = ids.to_vec();
        sorted_ids.sort();

        let accounts: Vec<Account> = sqlx::query_as(
            r#"
            SELECT
                id, external_id, name, currency, account_type, status,
                balance, available_balance, pending_balance, version,
                metadata, created_at, updated_at
            FROM accounts
            WHERE id = ANY($1)
            ORDER BY id
            FOR UPDATE
            "#,
        )
        .bind(&sorted_ids)
        .fetch_all(&mut **tx)
        .await?;

        if accounts.len() != sorted_ids.len() {
            let found_ids: std::collections::HashSet<_> =
                accounts.iter().map(|a| a.id).collect();
            for id in sorted_ids {
                if !found_ids.contains(&id) {
                    return Err(LedgerError::AccountNotFound(id.to_string()));
                }
            }
        }

        Ok(accounts)
    }

    /// Update account balance with optimistic locking
    pub async fn update_balance_optimistic(
        tx: &mut Transaction<'_, Postgres>,
        id: Uuid,
        new_balance: Decimal,
        expected_version: i64,
    ) -> Result<Account> {
        let result = sqlx::query(
            r#"
            UPDATE accounts
            SET
                balance = $2,
                available_balance = $2 - pending_balance,
                version = version + 1
            WHERE id = $1 AND version = $3
            RETURNING version
            "#,
        )
        .bind(id)
        .bind(new_balance)
        .bind(expected_version)
        .fetch_optional(&mut **tx)
        .await?;

        if result.is_none() {
            let current: Option<i64> = sqlx::query_scalar(
                "SELECT version FROM accounts WHERE id = $1",
            )
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?;

            match current {
                Some(actual_version) => {
                    return Err(LedgerError::OptimisticLockError {
                        account_id: id.to_string(),
                        expected_version,
                        actual_version,
                    });
                }
                None => {
                    return Err(LedgerError::AccountNotFound(id.to_string()));
                }
            }
        }

        Self::get_for_update(tx, id).await
    }

    /// Update balance using row-level lock (FOR UPDATE already acquired)
    pub async fn update_balance_locked(
        tx: &mut Transaction<'_, Postgres>,
        id: Uuid,
        new_balance: Decimal,
    ) -> Result<Account> {
        sqlx::query(
            r#"
            UPDATE accounts
            SET
                balance = $2,
                available_balance = $2 - pending_balance,
                version = version + 1
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(new_balance)
        .execute(&mut **tx)
        .await?;

        Self::get_for_update(tx, id).await
    }

    pub async fn record_balance_history(
        tx: &mut Transaction<'_, Postgres>,
        account_id: Uuid,
        entry_id: Uuid,
        previous_balance: Decimal,
        new_balance: Decimal,
        version_before: i64,
        version_after: i64,
    ) -> Result<()> {
        let change_amount = new_balance - previous_balance;

        sqlx::query(
            r#"
            INSERT INTO balance_history (
                account_id, entry_id, previous_balance, new_balance,
                change_amount, version_before, version_after
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(account_id)
        .bind(entry_id)
        .bind(previous_balance)
        .bind(new_balance)
        .bind(change_amount)
        .bind(version_before)
        .bind(version_after)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    pub async fn list(
        pool: &PgPool,
        page_size: i32,
        page_token: Option<&str>,
        filter_type: Option<AccountType>,
        filter_status: Option<AccountStatus>,
        filter_currency: Option<&str>,
    ) -> Result<(Vec<Account>, Option<String>, i32)> {
        let limit = page_size.min(100).max(1);
        let offset_id: Option<Uuid> = page_token
            .and_then(|t| Uuid::parse_str(t).ok());

        let accounts: Vec<Account> = sqlx::query_as(
            r#"
            SELECT
                id, external_id, name, currency, account_type, status,
                balance, available_balance, pending_balance, version,
                metadata, created_at, updated_at
            FROM accounts
            WHERE
                ($1::uuid IS NULL OR id > $1)
                AND ($2::account_type IS NULL OR account_type = $2)
                AND ($3::account_status IS NULL OR status = $3)
                AND ($4::text IS NULL OR currency = $4)
            ORDER BY id
            LIMIT $5
            "#,
        )
        .bind(offset_id)
        .bind(filter_type)
        .bind(filter_status)
        .bind(filter_currency)
        .bind(limit + 1) // Fetch one extra to check for next page
        .fetch_all(pool)
        .await?;

        let has_more = accounts.len() > limit as usize;
        let accounts: Vec<Account> = accounts.into_iter().take(limit as usize).collect();
        let next_token = if has_more {
            accounts.last().map(|a| a.id.to_string())
        } else {
            None
        };

        let total: i32 = sqlx::query_scalar::<_, Option<i32>>(
            r#"
            SELECT COUNT(*)::int
            FROM accounts
            WHERE
                ($1::account_type IS NULL OR account_type = $1)
                AND ($2::account_status IS NULL OR status = $2)
                AND ($3::text IS NULL OR currency = $3)
            "#,
        )
        .bind(filter_type)
        .bind(filter_status)
        .bind(filter_currency)
        .fetch_one(pool)
        .await?
        .unwrap_or(0);

        Ok((accounts, next_token, total))
    }
}
