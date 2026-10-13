-- Per-rule label description overrides. The global default lives in
-- label_qualifications(account_email, label_id); a row here replaces individual
-- fields for one rule only. Empty fields fall back to the global value.
CREATE TABLE IF NOT EXISTS rule_label_overrides (
    account_email     TEXT NOT NULL,
    rule_id           INTEGER NOT NULL REFERENCES rules(id) ON DELETE CASCADE,
    label_id          TEXT NOT NULL,
    description       TEXT NOT NULL DEFAULT '',
    examples          TEXT NOT NULL DEFAULT '[]',
    negative_examples TEXT NOT NULL DEFAULT '[]',
    updated_at        TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (rule_id, label_id)
);

CREATE INDEX IF NOT EXISTS idx_rule_label_overrides_account
ON rule_label_overrides(account_email, rule_id);
