-- VeloxLedger Initial Schema
-- High-performance transactional ledger with double-entry accounting
-- Designed for ACID compliance and optimistic locking

-- Enable required extensions
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
CREATE EXTENSION IF NOT EXISTS "pgcrypto";

-- ============================================================================
-- ENUM TYPES
-- ============================================================================

CREATE TYPE account_type AS ENUM (
    'asset',
    'liability',
    'equity',
    'revenue',
    'expense'
);

CREATE TYPE account_status AS ENUM (
    'active',
    'frozen',
    'closed'
);

CREATE TYPE transaction_status AS ENUM (
    'pending',
    'committed',
    'failed',
    'reversed'
);

CREATE TYPE entry_type AS ENUM (
    'debit',
    'credit'
);

-- ============================================================================
-- IDEMPOTENCY KEYS TABLE
-- Stores processed idempotency keys to prevent duplicate operations
-- ============================================================================

CREATE TABLE idempotency_keys (
    key TEXT PRIMARY KEY,
    resource_type TEXT NOT NULL,           -- 'account' or 'transaction'
    resource_id UUID NOT NULL,
    response_data JSONB,                   -- Cached response
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT (NOW() + INTERVAL '24 hours')
);

CREATE INDEX idx_idempotency_keys_expires_at ON idempotency_keys(expires_at);

-- ============================================================================
-- ACCOUNTS TABLE
-- Core account entity with optimistic locking support
-- ============================================================================

CREATE TABLE accounts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    external_id TEXT UNIQUE,
    name TEXT NOT NULL,
    currency CHAR(3) NOT NULL,             -- ISO 4217
    account_type account_type NOT NULL,
    status account_status NOT NULL DEFAULT 'active',

    -- Balances stored as NUMERIC for precision (maps to rust_decimal)
    balance NUMERIC(28, 12) NOT NULL DEFAULT 0,
    available_balance NUMERIC(28, 12) NOT NULL DEFAULT 0,
    pending_balance NUMERIC(28, 12) NOT NULL DEFAULT 0,

    -- Optimistic Locking: version increments on every update
    version BIGINT NOT NULL DEFAULT 1,

    -- Metadata as JSONB for flexibility
    metadata JSONB NOT NULL DEFAULT '{}',

    -- Timestamps
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- Constraints
    CONSTRAINT chk_balance_sign CHECK (
        (account_type IN ('asset', 'expense') AND balance >= 0) OR
        (account_type IN ('liability', 'equity', 'revenue'))
    ),
    CONSTRAINT chk_currency_format CHECK (currency ~ '^[A-Z]{3}$')
);

-- Indexes for accounts
CREATE INDEX idx_accounts_external_id ON accounts(external_id) WHERE external_id IS NOT NULL;
CREATE INDEX idx_accounts_currency ON accounts(currency);
CREATE INDEX idx_accounts_type ON accounts(account_type);
CREATE INDEX idx_accounts_status ON accounts(status);
CREATE INDEX idx_accounts_created_at ON accounts(created_at DESC);

-- Partial index for active accounts (most common query)
CREATE INDEX idx_accounts_active ON accounts(id) WHERE status = 'active';

-- ============================================================================
-- TRANSACTIONS TABLE
-- Represents a complete double-entry transaction
-- ============================================================================

CREATE TABLE transactions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    idempotency_key TEXT UNIQUE NOT NULL,
    reference TEXT,                        -- External reference
    description TEXT,
    status transaction_status NOT NULL DEFAULT 'pending',
    error_message TEXT,
    metadata JSONB NOT NULL DEFAULT '{}',

    -- Timestamps
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    committed_at TIMESTAMPTZ
);

-- Indexes for transactions
CREATE INDEX idx_transactions_idempotency_key ON transactions(idempotency_key);
CREATE INDEX idx_transactions_reference ON transactions(reference) WHERE reference IS NOT NULL;
CREATE INDEX idx_transactions_status ON transactions(status);
CREATE INDEX idx_transactions_created_at ON transactions(created_at DESC);
CREATE INDEX idx_transactions_committed_at ON transactions(committed_at DESC) WHERE committed_at IS NOT NULL;

-- ============================================================================
-- ENTRIES TABLE
-- Individual debit/credit entries within a transaction
-- ============================================================================

CREATE TABLE entries (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    transaction_id UUID NOT NULL REFERENCES transactions(id) ON DELETE CASCADE,
    account_id UUID NOT NULL REFERENCES accounts(id),
    entry_type entry_type NOT NULL,

    -- Amount is always positive; entry_type determines direction
    amount NUMERIC(28, 12) NOT NULL CHECK (amount > 0),

    description TEXT,

    -- Snapshot of account balance after this entry applied
    balance_after NUMERIC(28, 12),

    metadata JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- Ensure referential integrity
    CONSTRAINT fk_entries_transaction FOREIGN KEY (transaction_id)
        REFERENCES transactions(id) ON DELETE CASCADE,
    CONSTRAINT fk_entries_account FOREIGN KEY (account_id)
        REFERENCES accounts(id) ON DELETE RESTRICT
);

-- Indexes for entries
CREATE INDEX idx_entries_transaction_id ON entries(transaction_id);
CREATE INDEX idx_entries_account_id ON entries(account_id);
CREATE INDEX idx_entries_account_created ON entries(account_id, created_at DESC);
CREATE INDEX idx_entries_type ON entries(entry_type);

-- Composite index for account statement queries
CREATE INDEX idx_entries_account_timeline ON entries(account_id, created_at DESC, entry_type);

-- ============================================================================
-- WEBHOOK EVENTS TABLE
-- Queue for outbound webhook notifications
-- ============================================================================

CREATE TABLE webhook_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    event_type TEXT NOT NULL,              -- e.g., 'transaction.committed'
    payload JSONB NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending', -- pending, delivered, failed
    retry_count INT NOT NULL DEFAULT 0,
    max_retries INT NOT NULL DEFAULT 5,
    next_retry_at TIMESTAMPTZ,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    delivered_at TIMESTAMPTZ
);

CREATE INDEX idx_webhook_events_status ON webhook_events(status);
CREATE INDEX idx_webhook_events_next_retry ON webhook_events(next_retry_at)
    WHERE status = 'pending' AND next_retry_at IS NOT NULL;

-- ============================================================================
-- ACCOUNT BALANCE HISTORY (AUDIT TRAIL)
-- Immutable log of all balance changes
-- ============================================================================

CREATE TABLE balance_history (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id UUID NOT NULL REFERENCES accounts(id),
    entry_id UUID REFERENCES entries(id),
    previous_balance NUMERIC(28, 12) NOT NULL,
    new_balance NUMERIC(28, 12) NOT NULL,
    change_amount NUMERIC(28, 12) NOT NULL,
    version_before BIGINT NOT NULL,
    version_after BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_balance_history_account ON balance_history(account_id, created_at DESC);

-- ============================================================================
-- FUNCTIONS
-- ============================================================================

-- Function to update updated_at timestamp
CREATE OR REPLACE FUNCTION update_updated_at_column()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Trigger for accounts updated_at
CREATE TRIGGER trg_accounts_updated_at
    BEFORE UPDATE ON accounts
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

-- Function to validate double-entry balance
-- Sum of debits must equal sum of credits for a transaction
CREATE OR REPLACE FUNCTION validate_transaction_balance(p_transaction_id UUID)
RETURNS BOOLEAN AS $$
DECLARE
    v_debit_sum NUMERIC(28, 12);
    v_credit_sum NUMERIC(28, 12);
BEGIN
    SELECT
        COALESCE(SUM(CASE WHEN entry_type = 'debit' THEN amount ELSE 0 END), 0),
        COALESCE(SUM(CASE WHEN entry_type = 'credit' THEN amount ELSE 0 END), 0)
    INTO v_debit_sum, v_credit_sum
    FROM entries
    WHERE transaction_id = p_transaction_id;

    RETURN v_debit_sum = v_credit_sum;
END;
$$ LANGUAGE plpgsql;

-- ============================================================================
-- CLEANUP JOB HELPER
-- ============================================================================

-- Function to cleanup expired idempotency keys (run periodically)
CREATE OR REPLACE FUNCTION cleanup_expired_idempotency_keys()
RETURNS INTEGER AS $$
DECLARE
    deleted_count INTEGER;
BEGIN
    DELETE FROM idempotency_keys
    WHERE expires_at < NOW();

    GET DIAGNOSTICS deleted_count = ROW_COUNT;
    RETURN deleted_count;
END;
$$ LANGUAGE plpgsql;

-- ============================================================================
-- PERFORMANCE CONFIGURATION RECOMMENDATIONS
-- ============================================================================

-- Run these as superuser for production:
--
-- -- For high-throughput OLTP
-- ALTER SYSTEM SET shared_buffers = '4GB';
-- ALTER SYSTEM SET effective_cache_size = '12GB';
-- ALTER SYSTEM SET work_mem = '256MB';
-- ALTER SYSTEM SET maintenance_work_mem = '1GB';
--
-- -- WAL settings for durability without sacrificing performance
-- ALTER SYSTEM SET wal_buffers = '64MB';
-- ALTER SYSTEM SET checkpoint_completion_target = 0.9;
-- ALTER SYSTEM SET synchronous_commit = 'on';
--
-- -- Connection pooling (use PgBouncer in production)
-- ALTER SYSTEM SET max_connections = 200;
