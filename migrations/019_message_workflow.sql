CREATE TABLE IF NOT EXISTS workflow_mailboxes (
    account_email   TEXT PRIMARY KEY,
    history_cursor  TEXT NOT NULL,
    initialized_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS workflow_rule_sets (
    id              INTEGER PRIMARY KEY,
    account_email   TEXT NOT NULL,
    version         INTEGER NOT NULL,
    rules_json      TEXT NOT NULL,
    active          INTEGER NOT NULL DEFAULT 1,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(account_email, version)
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_workflow_rule_sets_active
ON workflow_rule_sets(account_email) WHERE active = 1;

CREATE TABLE IF NOT EXISTS workflow_messages (
    id                  INTEGER PRIMARY KEY,
    account_email       TEXT NOT NULL,
    gmail_message_id    TEXT NOT NULL,
    gmail_thread_id     TEXT,
    gmail_history_id    TEXT,
    message_json        TEXT,
    labels_json         TEXT,
    created_at          TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at          TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(account_email, gmail_message_id)
);

CREATE INDEX IF NOT EXISTS idx_workflow_messages_account_created
ON workflow_messages(account_email, created_at DESC);

CREATE TABLE IF NOT EXISTS workflow_runs (
    id                  INTEGER PRIMARY KEY,
    account_email       TEXT NOT NULL,
    message_id          INTEGER NOT NULL REFERENCES workflow_messages(id) ON DELETE CASCADE,
    rule_set_id         INTEGER NOT NULL REFERENCES workflow_rule_sets(id),
    state               TEXT NOT NULL DEFAULT 'queued',
    next_rule_index     INTEGER NOT NULL DEFAULT 0,
    next_attempt_at     TEXT NOT NULL DEFAULT (datetime('now')),
    lease_token         TEXT,
    lease_until         TEXT,
    last_error          TEXT,
    created_at          TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at          TEXT NOT NULL DEFAULT (datetime('now')),
    completed_at        TEXT,
    UNIQUE(message_id)
);

CREATE INDEX IF NOT EXISTS idx_workflow_runs_due
ON workflow_runs(account_email, state, next_attempt_at);

CREATE TABLE IF NOT EXISTS workflow_steps (
    id                  INTEGER PRIMARY KEY,
    run_id              INTEGER NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
    rule_index          INTEGER NOT NULL,
    rule_legacy_id      INTEGER NOT NULL,
    rule_name           TEXT NOT NULL,
    outcome             TEXT NOT NULL,
    decision_json       TEXT,
    error               TEXT,
    created_at          TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(run_id, rule_index, outcome)
);

CREATE TABLE IF NOT EXISTS workflow_llm_attempts (
    id                  INTEGER PRIMARY KEY,
    run_id              INTEGER NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
    rule_index          INTEGER NOT NULL,
    provider_id         TEXT,
    model               TEXT,
    status              TEXT NOT NULL,
    error               TEXT,
    duration_ms         INTEGER,
    created_at          TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_workflow_llm_attempts_run
ON workflow_llm_attempts(run_id, rule_index, created_at);

CREATE TABLE IF NOT EXISTS workflow_action_plans (
    id                  INTEGER PRIMARY KEY,
    run_id              INTEGER NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
    rule_index          INTEGER NOT NULL,
    add_label_ids       TEXT NOT NULL,
    remove_label_ids    TEXT NOT NULL,
    state               TEXT NOT NULL DEFAULT 'pending',
    attempt_count       INTEGER NOT NULL DEFAULT 0,
    last_error          TEXT,
    created_at          TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at          TEXT NOT NULL DEFAULT (datetime('now')),
    completed_at        TEXT,
    UNIQUE(run_id, rule_index)
);

CREATE INDEX IF NOT EXISTS idx_workflow_action_plans_pending
ON workflow_action_plans(state, updated_at);

CREATE TABLE IF NOT EXISTS workflow_events (
    id                  INTEGER PRIMARY KEY,
    account_email       TEXT NOT NULL,
    message_id          INTEGER REFERENCES workflow_messages(id) ON DELETE CASCADE,
    run_id              INTEGER REFERENCES workflow_runs(id) ON DELETE CASCADE,
    kind                TEXT NOT NULL,
    detail              TEXT,
    created_at          TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_workflow_events_message
ON workflow_events(message_id, created_at);
