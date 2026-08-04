CREATE TABLE IF NOT EXISTS llm_usage (
    id                  INTEGER PRIMARY KEY,
    history_id          INTEGER REFERENCES history(id) ON DELETE SET NULL,
    email_id            TEXT NOT NULL,
    rule_id             INTEGER NOT NULL REFERENCES rules(id) ON DELETE CASCADE,
    model               TEXT,
    prompt_tokens       INTEGER,
    completion_tokens   INTEGER,
    total_tokens        INTEGER,
    duration_ms         INTEGER,
    estimated_cost_usd  REAL,
    source              TEXT NOT NULL DEFAULT 'cycle',
    created_at          TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_llm_usage_rule_date ON llm_usage(rule_id, created_at);
CREATE INDEX IF NOT EXISTS idx_llm_usage_source_date ON llm_usage(source, created_at);
CREATE INDEX IF NOT EXISTS idx_llm_usage_history ON llm_usage(history_id);
