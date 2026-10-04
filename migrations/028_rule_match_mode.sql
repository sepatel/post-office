-- How many matching labels/outcomes a rule may select at once.
ALTER TABLE rules ADD COLUMN match_mode TEXT NOT NULL DEFAULT 'single';

-- Durable, per-account label qualifications. Ids are mailbox-local, so the
-- account column is part of the key and rows are removed with the account.
CREATE TABLE IF NOT EXISTS label_qualifications (
    account_email     TEXT NOT NULL,
    label_id          TEXT NOT NULL,
    description       TEXT NOT NULL DEFAULT '',
    examples          TEXT NOT NULL DEFAULT '[]',
    negative_examples TEXT NOT NULL DEFAULT '[]',
    source            TEXT NOT NULL DEFAULT '',
    updated_at        TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (account_email, label_id)
);
