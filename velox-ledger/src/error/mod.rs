use std::fmt;

use tonic::Status;

pub type Result<T> = std::result::Result<T, LedgerError>;

#[derive(Debug)]
pub enum LedgerError {
    // Validation errors
    ValidationError(String),
    InvalidAmount(String),
    InvalidCurrency(String),
    BalanceViolation(String),

    // Business logic errors
    AccountNotFound(String),
    AccountFrozen(String),
    AccountClosed(String),
    InsufficientBalance {
        account_id: String,
        required: String,
        available: String,
    },
    TransactionNotFound(String),
    DuplicateExternalId(String),

    // Idempotency
    IdempotencyKeyReused {
        key: String,
        original_resource_id: String,
    },

    // Concurrency errors
    OptimisticLockError {
        account_id: String,
        expected_version: i64,
        actual_version: i64,
    },
    ConcurrencyConflict(String),

    // Database errors
    DatabaseError(String),
    ConnectionPoolExhausted,
    TransactionRollback(String),

    // Internal errors
    InternalError(String),
    SerializationError(String),
    ConfigurationError(String),
}

impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ValidationError(msg) => write!(f, "Validation error: {}", msg),
            Self::InvalidAmount(msg) => write!(f, "Invalid amount: {}", msg),
            Self::InvalidCurrency(msg) => write!(f, "Invalid currency: {}", msg),
            Self::BalanceViolation(msg) => write!(f, "Balance violation: {}", msg),
            Self::AccountNotFound(id) => write!(f, "Account not found: {}", id),
            Self::AccountFrozen(id) => write!(f, "Account is frozen: {}", id),
            Self::AccountClosed(id) => write!(f, "Account is closed: {}", id),
            Self::InsufficientBalance {
                account_id,
                required,
                available,
            } => {
                write!(
                    f,
                    "Insufficient balance in account {}: required {}, available {}",
                    account_id, required, available
                )
            }
            Self::TransactionNotFound(id) => write!(f, "Transaction not found: {}", id),
            Self::DuplicateExternalId(id) => {
                write!(f, "Duplicate external ID: {}", id)
            }
            Self::IdempotencyKeyReused {
                key,
                original_resource_id,
            } => {
                write!(
                    f,
                    "Idempotency key '{}' already used for resource {}",
                    key, original_resource_id
                )
            }
            Self::OptimisticLockError {
                account_id,
                expected_version,
                actual_version,
            } => {
                write!(
                    f,
                    "Optimistic lock failed for account {}: expected version {}, actual {}",
                    account_id, expected_version, actual_version
                )
            }
            Self::ConcurrencyConflict(msg) => write!(f, "Concurrency conflict: {}", msg),
            Self::DatabaseError(msg) => write!(f, "Database error: {}", msg),
            Self::ConnectionPoolExhausted => write!(f, "Connection pool exhausted"),
            Self::TransactionRollback(msg) => write!(f, "Transaction rolled back: {}", msg),
            Self::InternalError(msg) => write!(f, "Internal error: {}", msg),
            Self::SerializationError(msg) => write!(f, "Serialization error: {}", msg),
            Self::ConfigurationError(msg) => write!(f, "Configuration error: {}", msg),
        }
    }
}

impl std::error::Error for LedgerError {}

impl From<sqlx::Error> for LedgerError {
    fn from(err: sqlx::Error) -> Self {
        match &err {
            sqlx::Error::RowNotFound => {
                LedgerError::DatabaseError("Row not found".to_string())
            }
            sqlx::Error::PoolTimedOut => LedgerError::ConnectionPoolExhausted,
            sqlx::Error::Database(db_err) => {
                let code = db_err.code().unwrap_or_default();
                match code.as_ref() {
                    "23505" => {
                        // Unique violation
                        LedgerError::DuplicateExternalId(db_err.message().to_string())
                    }
                    "23503" => {
                        // Foreign key violation
                        LedgerError::ValidationError(db_err.message().to_string())
                    }
                    "40001" => {
                        // Serialization failure
                        LedgerError::ConcurrencyConflict(db_err.message().to_string())
                    }
                    _ => LedgerError::DatabaseError(db_err.message().to_string()),
                }
            }
            _ => LedgerError::DatabaseError(err.to_string()),
        }
    }
}

impl From<serde_json::Error> for LedgerError {
    fn from(err: serde_json::Error) -> Self {
        LedgerError::SerializationError(err.to_string())
    }
}

impl From<LedgerError> for Status {
    fn from(err: LedgerError) -> Self {
        match err {
            LedgerError::ValidationError(msg) => Status::invalid_argument(msg),
            LedgerError::InvalidAmount(msg) => Status::invalid_argument(msg),
            LedgerError::InvalidCurrency(msg) => Status::invalid_argument(msg),
            LedgerError::BalanceViolation(msg) => Status::failed_precondition(msg),
            LedgerError::AccountNotFound(id) => {
                Status::not_found(format!("Account not found: {}", id))
            }
            LedgerError::AccountFrozen(id) => {
                Status::failed_precondition(format!("Account frozen: {}", id))
            }
            LedgerError::AccountClosed(id) => {
                Status::failed_precondition(format!("Account closed: {}", id))
            }
            LedgerError::InsufficientBalance {
                account_id,
                required,
                available,
            } => Status::failed_precondition(format!(
                "Insufficient balance in {}: required {}, available {}",
                account_id, required, available
            )),
            LedgerError::TransactionNotFound(id) => {
                Status::not_found(format!("Transaction not found: {}", id))
            }
            LedgerError::DuplicateExternalId(id) => {
                Status::already_exists(format!("External ID already exists: {}", id))
            }
            LedgerError::IdempotencyKeyReused { key, .. } => {
                Status::already_exists(format!("Idempotency key already used: {}", key))
            }
            LedgerError::OptimisticLockError { account_id, .. } => {
                Status::aborted(format!("Concurrent modification of account: {}", account_id))
            }
            LedgerError::ConcurrencyConflict(msg) => Status::aborted(msg),
            LedgerError::DatabaseError(msg) => Status::internal(msg),
            LedgerError::ConnectionPoolExhausted => {
                Status::resource_exhausted("Database connection pool exhausted")
            }
            LedgerError::TransactionRollback(msg) => Status::aborted(msg),
            LedgerError::InternalError(msg) => Status::internal(msg),
            LedgerError::SerializationError(msg) => Status::internal(msg),
            LedgerError::ConfigurationError(msg) => Status::internal(msg),
        }
    }
}

pub trait ResultExt<T> {
    fn with_context<F, S>(self, f: F) -> Result<T>
    where
        F: FnOnce() -> S,
        S: Into<String>;
}

impl<T, E: Into<LedgerError>> ResultExt<T> for std::result::Result<T, E> {
    fn with_context<F, S>(self, f: F) -> Result<T>
    where
        F: FnOnce() -> S,
        S: Into<String>,
    {
        self.map_err(|e| {
            let err = e.into();
            tracing::error!(error = %err, context = %f().into(), "Operation failed");
            err
        })
    }
}
