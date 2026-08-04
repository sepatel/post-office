CREATE TABLE IF NOT EXISTS llm_provider_status (
    provider_id         TEXT PRIMARY KEY,
    rate_limited_until  TEXT,
    last_error          TEXT,
    updated_at          TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_llm_provider_status_rate_limited
ON llm_provider_status(rate_limited_until);
