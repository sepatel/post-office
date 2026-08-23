-- Per-account label cache. Label ids are mailbox-local and system ids collide
-- across accounts, so this must never be shared between accounts.
CREATE TABLE IF NOT EXISTS gmail_labels (
    account_email TEXT PRIMARY KEY,
    labels        TEXT NOT NULL,
    fetched_at    TEXT NOT NULL DEFAULT (datetime('now'))
);
