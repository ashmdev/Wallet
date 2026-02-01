use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::Type;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[sqlx(type_name = "entry_type", rename_all = "lowercase")]
pub enum EntryType {
    Debit,
    Credit,
}

impl EntryType {
    pub fn opposite(&self) -> Self {
        match self {
            EntryType::Debit => EntryType::Credit,
            EntryType::Credit => EntryType::Debit,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Entry {
    pub id: Uuid,
    pub transaction_id: Uuid,
    pub account_id: Uuid,
    pub entry_type: EntryType,
    pub amount: Decimal,
    pub description: Option<String>,
    pub balance_after: Option<Decimal>,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct CreateEntryParams {
    pub account_id: Uuid,
    pub entry_type: EntryType,
    pub amount: Decimal,
    pub description: Option<String>,
    pub metadata: serde_json::Value,
}

impl CreateEntryParams {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.amount <= Decimal::ZERO {
            return Err("Entry amount must be positive");
        }
        Ok(())
    }

    pub fn debit(account_id: Uuid, amount: Decimal) -> Self {
        Self {
            account_id,
            entry_type: EntryType::Debit,
            amount,
            description: None,
            metadata: serde_json::json!({}),
        }
    }

    pub fn credit(account_id: Uuid, amount: Decimal) -> Self {
        Self {
            account_id,
            entry_type: EntryType::Credit,
            amount,
            description: None,
            metadata: serde_json::json!({}),
        }
    }
}

#[derive(Debug, Clone)]
pub struct EntrySet {
    entries: Vec<CreateEntryParams>,
}

impl EntrySet {
    pub fn new(entries: Vec<CreateEntryParams>) -> Result<Self, &'static str> {
        if entries.len() < 2 {
            return Err("Transaction must have at least 2 entries");
        }

        let set = Self { entries };
        set.validate_balance()?;
        Ok(set)
    }

    pub fn validate_balance(&self) -> Result<(), &'static str> {
        let mut debit_sum = Decimal::ZERO;
        let mut credit_sum = Decimal::ZERO;

        for entry in &self.entries {
            match entry.entry_type {
                EntryType::Debit => debit_sum += entry.amount,
                EntryType::Credit => credit_sum += entry.amount,
            }
        }

        if debit_sum != credit_sum {
            return Err("Debits must equal credits (double-entry balance violation)");
        }

        Ok(())
    }

    pub fn entries(&self) -> &[CreateEntryParams] {
        &self.entries
    }

    pub fn into_entries(self) -> Vec<CreateEntryParams> {
        self.entries
    }

    pub fn total_debits(&self) -> Decimal {
        self.entries
            .iter()
            .filter(|e| e.entry_type == EntryType::Debit)
            .map(|e| e.amount)
            .sum()
    }

    pub fn total_credits(&self) -> Decimal {
        self.entries
            .iter()
            .filter(|e| e.entry_type == EntryType::Credit)
            .map(|e| e.amount)
            .sum()
    }
}
