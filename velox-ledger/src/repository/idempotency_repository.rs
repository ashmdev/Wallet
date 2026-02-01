use chrono::{Duration, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::error::Result;

pub struct IdempotencyRepository;

#[derive(Debug, sqlx::FromRow)]
pub struct IdempotencyRecord {
    pub key: String,
    pub resource_type: String,
    pub resource_id: Uuid,
    pub response_data: Option<serde_json::Value>,
}

impl IdempotencyRepository {
    pub async fn check_and_set(
        tx: &mut Transaction<'_, Postgres>,
        key: &str,
        resource_type: &str,
        resource_id: Uuid,
        response_data: Option<serde_json::Value>,
    ) -> Result<Option<IdempotencyRecord>> {
        let existing: Option<IdempotencyRecord> = sqlx::query_as(
            r#"
            SELECT key, resource_type, resource_id, response_data
            FROM idempotency_keys
            WHERE key = $1
            FOR UPDATE
            "#,
        )
        .bind(key)
        .fetch_optional(&mut **tx)
        .await?;

        if let Some(record) = existing {
            return Ok(Some(record));
        }

        let expires_at = Utc::now() + Duration::hours(24);

        sqlx::query(
            r#"
            INSERT INTO idempotency_keys (key, resource_type, resource_id, response_data, expires_at)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(key)
        .bind(resource_type)
        .bind(resource_id)
        .bind(response_data)
        .bind(expires_at)
        .execute(&mut **tx)
        .await?;

        Ok(None)
    }

    pub async fn get(pool: &PgPool, key: &str) -> Result<Option<IdempotencyRecord>> {
        let record: Option<IdempotencyRecord> = sqlx::query_as(
            r#"
            SELECT key, resource_type, resource_id, response_data
            FROM idempotency_keys
            WHERE key = $1 AND expires_at > NOW()
            "#,
        )
        .bind(key)
        .fetch_optional(pool)
        .await?;

        Ok(record)
    }

    pub async fn update_response(
        tx: &mut Transaction<'_, Postgres>,
        key: &str,
        response_data: serde_json::Value,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE idempotency_keys
            SET response_data = $2
            WHERE key = $1
            "#,
        )
        .bind(key)
        .bind(response_data)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    pub async fn cleanup_expired(pool: &PgPool) -> Result<u64> {
        let result = sqlx::query("SELECT cleanup_expired_idempotency_keys()")
            .execute(pool)
            .await?;

        Ok(result.rows_affected())
    }
}
