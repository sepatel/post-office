CREATE TABLE IF NOT EXISTS rule_inference_policy (
    rule_id   INTEGER PRIMARY KEY REFERENCES rules(id) ON DELETE CASCADE,
    policy_id TEXT NOT NULL DEFAULT 'default',
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS inference_jobs (
    id              INTEGER PRIMARY KEY,
    email_id        TEXT NOT NULL,
    rule_id         INTEGER REFERENCES rules(id) ON DELETE SET NULL,
    source          TEXT NOT NULL DEFAULT 'cycle',
    status          TEXT NOT NULL DEFAULT 'pending',
    attempt_count   INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TEXT NOT NULL DEFAULT (datetime('now')),
    lease_until     TEXT,
    last_error      TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_inference_jobs_due
ON inference_jobs(status, next_attempt_at);

CREATE INDEX IF NOT EXISTS idx_inference_jobs_email_rule
ON inference_jobs(email_id, rule_id, created_at);

CREATE TABLE IF NOT EXISTS inference_attempts (
    id          INTEGER PRIMARY KEY,
    job_id      INTEGER NOT NULL REFERENCES inference_jobs(id) ON DELETE CASCADE,
    provider_id TEXT,
    model       TEXT,
    status      TEXT NOT NULL,
    error       TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_inference_attempts_job
ON inference_attempts(job_id, created_at);

CREATE UNIQUE INDEX IF NOT EXISTS idx_inference_jobs_active
ON inference_jobs(email_id, rule_id)
WHERE status IN ('pending', 'running', 'retrying');
