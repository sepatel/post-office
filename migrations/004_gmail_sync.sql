CREATE TABLE IF NOT EXISTS gmail_sync_state (
    account_email        TEXT PRIMARY KEY,
    last_history_id      TEXT NOT NULL,
    watch_expiration     TEXT,
    watch_status         TEXT NOT NULL DEFAULT 'inactive',
    last_notification_at TEXT,
    last_history_pull_at TEXT,
    last_sync_error      TEXT,
    updated_at           TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS gmail_sync_cursor_log (
    id                 INTEGER PRIMARY KEY,
    account_email      TEXT NOT NULL,
    from_history_id    TEXT,
    to_history_id      TEXT NOT NULL,
    trigger            TEXT NOT NULL,
    processed_messages INTEGER NOT NULL DEFAULT 0,
    created_at         TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_gmail_sync_cursor_log_account_created
ON gmail_sync_cursor_log(account_email, created_at);

CREATE TABLE IF NOT EXISTS gmail_pending_events (
    id            INTEGER PRIMARY KEY,
    account_email TEXT NOT NULL,
    event_key     TEXT NOT NULL UNIQUE,
    min_history_id TEXT NOT NULL,
    received_at   TEXT NOT NULL DEFAULT (datetime('now')),
    status        TEXT NOT NULL DEFAULT 'pending'
);

CREATE INDEX IF NOT EXISTS idx_gmail_pending_events_account_status
ON gmail_pending_events(account_email, status, received_at);
