ALTER TABLE workflow_runs ADD COLUMN manual_retry INTEGER NOT NULL DEFAULT 0;
ALTER TABLE workflow_llm_attempts ADD COLUMN endpoint TEXT;

UPDATE workflow_runs
SET attempt_count = 3
WHERE attempt_count > 3;

CREATE TABLE IF NOT EXISTS llm_endpoint_status (
    endpoint          TEXT PRIMARY KEY,
    unavailable_until TEXT,
    last_error        TEXT,
    updated_at        TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_llm_endpoint_status_unavailable
ON llm_endpoint_status(unavailable_until);
