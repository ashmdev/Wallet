use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Type;
use uuid::Uuid;

use super::entry::Entry;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[sqlx(type_name = "transaction_status", rename_all = "lowercase")]
pub enum TransactionStatus {
    Pending,
    Committed,
    Failed,
    Reversed,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Transaction {
    pub id: Uuid,
    pub idempotency_key: String,
    pub reference: Option<String>,
    pub description: Option<String>,
    pub status: TransactionStatus,
    pub error_message: Option<String>,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub committed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionWithEntries {
    #[serde(flatten)]
    pub transaction: Transaction,
    pub entries: Vec<Entry>,
}

impl TransactionWithEntries {
    pub fn new(transaction: Transaction, entries: Vec<Entry>) -> Self {
        Self {
            transaction,
            entries,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CreateTransactionParams {
    pub idempotency_key: String,
    pub reference: Option<String>,
    pub description: Option<String>,
    pub metadata: serde_json::Value,
}

impl CreateTransactionParams {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.idempotency_key.is_empty() {
            return Err("Idempotency key is required");
        }
        if self.idempotency_key.len() > 255 {
            return Err("Idempotency key must be 255 characters or less");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionEvent {
    pub event_type: TransactionEventType,
    pub transaction_id: Uuid,
    pub idempotency_key: String,
    pub timestamp: DateTime<Utc>,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionEventType {
    TransactionCreated,
    TransactionCommitted,
    TransactionFailed,
    TransactionReversed,
}

impl TransactionEventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TransactionCreated => "transaction.created",
            Self::TransactionCommitted => "transaction.committed",
            Self::TransactionFailed => "transaction.failed",
            Self::TransactionReversed => "transaction.reversed",
        }
    }
}
