pub mod account_repository;
pub mod idempotency_repository;
pub mod transaction_repository;

pub use account_repository::AccountRepository;
pub use idempotency_repository::IdempotencyRepository;
pub use transaction_repository::TransactionRepository;

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::time::Duration;

use crate::config::DatabaseConfig;
use crate::error::{LedgerError, Result};

pub async fn create_pool(config: &DatabaseConfig) -> Result<PgPool> {
    PgPoolOptions::new()
        .max_connections(config.max_connections)
        .min_connections(config.min_connections)
        .acquire_timeout(Duration::from_secs(config.acquire_timeout_secs))
        .idle_timeout(Duration::from_secs(config.idle_timeout_secs))
        .max_lifetime(Duration::from_secs(config.max_lifetime_secs))
        .connect(&config.url)
        .await
        .map_err(|e| LedgerError::DatabaseError(e.to_string()))
}

pub async fn run_migrations(pool: &PgPool) -> Result<()> {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .map_err(|e| LedgerError::DatabaseError(format!("Migration failed: {}", e)))
}
