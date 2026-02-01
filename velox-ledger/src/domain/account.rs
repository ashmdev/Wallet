use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::Type;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[sqlx(type_name = "account_type", rename_all = "lowercase")]
pub enum AccountType {
    Asset,
    Liability,
    Equity,
    Revenue,
    Expense,
}

impl AccountType {
    pub fn is_debit_normal(&self) -> bool {
        matches!(self, AccountType::Asset | AccountType::Expense)
    }

    pub fn is_credit_normal(&self) -> bool {
        matches!(
            self,
            AccountType::Liability | AccountType::Equity | AccountType::Revenue
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[sqlx(type_name = "account_status", rename_all = "lowercase")]
pub enum AccountStatus {
    Active,
    Frozen,
    Closed,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Account {
    pub id: Uuid,
    pub external_id: Option<String>,
    pub name: String,
    pub currency: String,
    pub account_type: AccountType,
    pub status: AccountStatus,
    pub balance: Decimal,
    pub available_balance: Decimal,
    pub pending_balance: Decimal,
    pub version: i64,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Account {
    pub fn can_debit(&self, amount: Decimal) -> bool {
        if self.status != AccountStatus::Active {
            return false;
        }

        match self.account_type {
            AccountType::Asset | AccountType::Expense => self.available_balance >= amount,
            AccountType::Liability | AccountType::Equity | AccountType::Revenue => true,
        }
    }

    pub fn can_credit(&self, _amount: Decimal) -> bool {
        self.status == AccountStatus::Active
    }

    pub fn apply_debit(&self, amount: Decimal) -> Decimal {
        match self.account_type {
            AccountType::Asset | AccountType::Expense => self.balance + amount,
            AccountType::Liability | AccountType::Equity | AccountType::Revenue => {
                self.balance - amount
            }
        }
    }

    pub fn apply_credit(&self, amount: Decimal) -> Decimal {
        match self.account_type {
            AccountType::Asset | AccountType::Expense => self.balance - amount,
            AccountType::Liability | AccountType::Equity | AccountType::Revenue => {
                self.balance + amount
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct CreateAccountParams {
    pub external_id: Option<String>,
    pub name: String,
    pub currency: String,
    pub account_type: AccountType,
    pub initial_balance: Option<Decimal>,
    pub metadata: serde_json::Value,
}

impl CreateAccountParams {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.name.is_empty() {
            return Err("Account name cannot be empty");
        }
        if self.currency.len() != 3 {
            return Err("Currency must be 3 characters (ISO 4217)");
        }
        if let Some(balance) = self.initial_balance {
            if balance < Decimal::ZERO {
                return Err("Initial balance cannot be negative");
            }
        }
        Ok(())
    }
}
