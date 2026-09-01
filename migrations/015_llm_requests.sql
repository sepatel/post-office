CREATE TABLE IF NOT EXISTS llm_requests (
    id                  INTEGER PRIMARY KEY,
    account_email       TEXT NOT NULL,
    rule_id             INTEGER NOT NULL REFERENCES rules(id) ON DELETE CASCADE,
    request_key         TEXT NOT NULL,
    source              TEXT NOT NULL,
    kind                TEXT NOT NULL,
    provider_id         TEXT,
    model               TEXT NOT NULL,
    policy_id           TEXT,
    email_count         INTEGER NOT NULL,
    prompt_tokens       INTEGER,
    completion_tokens   INTEGER,
    total_tokens        INTEGER,
    duration_ms         INTEGER NOT NULL,
    created_at          TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(account_email, rule_id, request_key)
);

CREATE INDEX IF NOT EXISTS idx_llm_requests_rule_date
ON llm_requests(account_email, rule_id, created_at DESC);
