CREATE TABLE IF NOT EXISTS history_email_meta (
    email_id       TEXT PRIMARY KEY,
    email_sent_at  TEXT,
    updated_at     TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_history_email_meta_sent_at
ON history_email_meta(email_sent_at);
