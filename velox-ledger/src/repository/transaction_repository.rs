use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::{PgPool, Postgres, Transaction as SqlxTransaction};
use uuid::Uuid;

use crate::domain::{
    CreateEntryParams, CreateTransactionParams, Entry, Transaction, TransactionStatus,
    TransactionWithEntries,
};
use crate::error::{LedgerError, Result};

pub struct TransactionRepository;

impl TransactionRepository {
    pub async fn create(
        tx: &mut SqlxTransaction<'_, Postgres>,
        params: &CreateTransactionParams,
    ) -> Result<Transaction> {
        params.validate().map_err(|e| LedgerError::ValidationError(e.to_string()))?;

        let transaction: Transaction = sqlx::query_as(
            r#"
            INSERT INTO transactions (idempotency_key, reference, description, status, metadata)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING
                id, idempotency_key, reference, description, status,
                error_message, metadata, created_at, committed_at
            "#,
        )
        .bind(&params.idempotency_key)
        .bind(&params.reference)
        .bind(&params.description)
        .bind(TransactionStatus::Pending)
        .bind(&params.metadata)
        .fetch_one(&mut **tx)
        .await?;

        Ok(transaction)
    }

    pub async fn create_entry(
        tx: &mut SqlxTransaction<'_, Postgres>,
        transaction_id: Uuid,
        params: &CreateEntryParams,
        balance_after: Decimal,
    ) -> Result<Entry> {
        let entry: Entry = sqlx::query_as(
            r#"
            INSERT INTO entries (
                transaction_id, account_id, entry_type, amount,
                description, balance_after, metadata
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING
                id, transaction_id, account_id, entry_type, amount,
                description, balance_after, metadata, created_at
            "#,
        )
        .bind(transaction_id)
        .bind(params.account_id)
        .bind(params.entry_type)
        .bind(params.amount)
        .bind(&params.description)
        .bind(balance_after)
        .bind(&params.metadata)
        .fetch_one(&mut **tx)
        .await?;

        Ok(entry)
    }

    pub async fn commit(
        tx: &mut SqlxTransaction<'_, Postgres>,
        id: Uuid,
    ) -> Result<Transaction> {
        let transaction: Transaction = sqlx::query_as(
            r#"
            UPDATE transactions
            SET status = $2, committed_at = NOW()
            WHERE id = $1
            RETURNING
                id, idempotency_key, reference, description, status,
                error_message, metadata, created_at, committed_at
            "#,
        )
        .bind(id)
        .bind(TransactionStatus::Committed)
        .fetch_one(&mut **tx)
        .await?;

        Ok(transaction)
    }

    pub async fn fail(
        tx: &mut SqlxTransaction<'_, Postgres>,
        id: Uuid,
        error_message: &str,
    ) -> Result<Transaction> {
        let transaction: Transaction = sqlx::query_as(
            r#"
            UPDATE transactions
            SET status = $2, error_message = $3
            WHERE id = $1
            RETURNING
                id, idempotency_key, reference, description, status,
                error_message, metadata, created_at, committed_at
            "#,
        )
        .bind(id)
        .bind(TransactionStatus::Failed)
        .bind(error_message)
        .fetch_one(&mut **tx)
        .await?;

        Ok(transaction)
    }

    pub async fn get_by_id(pool: &PgPool, id: Uuid) -> Result<Transaction> {
        let transaction: Transaction = sqlx::query_as(
            r#"
            SELECT
                id, idempotency_key, reference, description, status,
                error_message, metadata, created_at, committed_at
            FROM transactions
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| LedgerError::TransactionNotFound(id.to_string()))?;

        Ok(transaction)
    }

    pub async fn get_by_idempotency_key(
        pool: &PgPool,
        idempotency_key: &str,
    ) -> Result<Option<Transaction>> {
        let transaction: Option<Transaction> = sqlx::query_as(
            r#"
            SELECT
                id, idempotency_key, reference, description, status,
                error_message, metadata, created_at, committed_at
            FROM transactions
            WHERE idempotency_key = $1
            "#,
        )
        .bind(idempotency_key)
        .fetch_optional(pool)
        .await?;

        Ok(transaction)
    }

    pub async fn get_with_entries(pool: &PgPool, id: Uuid) -> Result<TransactionWithEntries> {
        let transaction = Self::get_by_id(pool, id).await?;
        let entries = Self::get_entries(pool, id).await?;

        Ok(TransactionWithEntries::new(transaction, entries))
    }

    pub async fn get_entries(pool: &PgPool, transaction_id: Uuid) -> Result<Vec<Entry>> {
        let entries: Vec<Entry> = sqlx::query_as(
            r#"
            SELECT
                id, transaction_id, account_id, entry_type, amount,
                description, balance_after, metadata, created_at
            FROM entries
            WHERE transaction_id = $1
            ORDER BY created_at
            "#,
        )
        .bind(transaction_id)
        .fetch_all(pool)
        .await?;

        Ok(entries)
    }

    pub async fn list(
        pool: &PgPool,
        page_size: i32,
        page_token: Option<&str>,
        account_id: Option<Uuid>,
        filter_status: Option<TransactionStatus>,
        from_date: Option<DateTime<Utc>>,
        to_date: Option<DateTime<Utc>>,
    ) -> Result<(Vec<Transaction>, Option<String>, i32)> {
        let limit = page_size.min(100).max(1);
        let offset_id: Option<Uuid> = page_token.and_then(|t| Uuid::parse_str(t).ok());

        let transactions: Vec<Transaction> = if account_id.is_some() {
            // Filter by account - need to join with entries
            sqlx::query_as(
                r#"
                SELECT DISTINCT
                    t.id, t.idempotency_key, t.reference, t.description, t.status,
                    t.error_message, t.metadata, t.created_at, t.committed_at
                FROM transactions t
                INNER JOIN entries e ON e.transaction_id = t.id
                WHERE
                    ($1::uuid IS NULL OR t.id < $1)
                    AND e.account_id = $2
                    AND ($3::transaction_status IS NULL OR t.status = $3)
                    AND ($4::timestamptz IS NULL OR t.created_at >= $4)
                    AND ($5::timestamptz IS NULL OR t.created_at <= $5)
                ORDER BY t.id DESC
                LIMIT $6
                "#,
            )
            .bind(offset_id)
            .bind(account_id)
            .bind(filter_status)
            .bind(from_date)
            .bind(to_date)
            .bind(limit + 1)
            .fetch_all(pool)
            .await?
        } else {
            sqlx::query_as(
                r#"
                SELECT
                    id, idempotency_key, reference, description, status,
                    error_message, metadata, created_at, committed_at
                FROM transactions
                WHERE
                    ($1::uuid IS NULL OR id < $1)
                    AND ($2::transaction_status IS NULL OR status = $2)
                    AND ($3::timestamptz IS NULL OR created_at >= $3)
                    AND ($4::timestamptz IS NULL OR created_at <= $4)
                ORDER BY id DESC
                LIMIT $5
                "#,
            )
            .bind(offset_id)
            .bind(filter_status)
            .bind(from_date)
            .bind(to_date)
            .bind(limit + 1)
            .fetch_all(pool)
            .await?
        };

        let has_more = transactions.len() > limit as usize;
        let transactions: Vec<Transaction> =
            transactions.into_iter().take(limit as usize).collect();
        let next_token = if has_more {
            transactions.last().map(|t| t.id.to_string())
        } else {
            None
        };

        // Get total count
        let total: i32 = if account_id.is_some() {
            sqlx::query_scalar::<_, Option<i32>>(
                r#"
                SELECT COUNT(DISTINCT t.id)::int
                FROM transactions t
                INNER JOIN entries e ON e.transaction_id = t.id
                WHERE
                    e.account_id = $1
                    AND ($2::transaction_status IS NULL OR t.status = $2)
                    AND ($3::timestamptz IS NULL OR t.created_at >= $3)
                    AND ($4::timestamptz IS NULL OR t.created_at <= $4)
                "#,
            )
            .bind(account_id)
            .bind(filter_status)
            .bind(from_date)
            .bind(to_date)
            .fetch_one(pool)
            .await?
            .unwrap_or(0)
        } else {
            sqlx::query_scalar::<_, Option<i32>>(
                r#"
                SELECT COUNT(*)::int
                FROM transactions
                WHERE
                    ($1::transaction_status IS NULL OR status = $1)
                    AND ($2::timestamptz IS NULL OR created_at >= $2)
                    AND ($3::timestamptz IS NULL OR created_at <= $3)
                "#,
            )
            .bind(filter_status)
            .bind(from_date)
            .bind(to_date)
            .fetch_one(pool)
            .await?
            .unwrap_or(0)
        };

        Ok((transactions, next_token, total))
    }
}
