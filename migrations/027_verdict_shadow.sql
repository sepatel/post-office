-- Local decisions (rverdict) recorded beside the LLM's, one row per question
-- framing per decided step. Additive only: builds without this migration
-- ignore these tables.
CREATE TABLE IF NOT EXISTS workflow_verdicts (
    id                  INTEGER PRIMARY KEY,
    step_id             INTEGER NOT NULL REFERENCES workflow_steps(id) ON DELETE CASCADE,
    run_id              INTEGER NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
    account_email       TEXT NOT NULL,
    rule_index          INTEGER NOT NULL,
    rule_legacy_id      INTEGER NOT NULL,
    rule_name           TEXT NOT NULL,
    -- 'noul' | 'binary' | 'menu' | 'multi:<target>', or '-' for a step that
    -- could not be asked. Multiple-match targets share the 'multi' family.
    framing             TEXT NOT NULL,
    model               TEXT NOT NULL,
    backend             TEXT,
    precision           TEXT,
    -- 'ok' | 'skipped' | 'error'
    status              TEXT NOT NULL,
    detail              TEXT,
    question_json       TEXT,
    -- What each option means, in option order.
    options_json        TEXT,
    kind_json           TEXT,
    logits_json         TEXT,
    probabilities_json  TEXT,
    verdict_matched     INTEGER,
    verdict_choice      TEXT,
    confidence          REAL,
    llm_matched         INTEGER,
    -- JSON array of the LLM's chosen menu entries.
    llm_choices_json    TEXT,
    -- Index of the LLM's answer among the options, for calibration.
    llm_option          INTEGER,
    agrees              INTEGER,
    state_tokens        INTEGER,
    truncated           INTEGER,
    duration_ms         INTEGER,
    created_at          TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(step_id, framing, model)
);

CREATE INDEX IF NOT EXISTS idx_workflow_verdicts_rule
ON workflow_verdicts(account_email, rule_legacy_id, framing, model);

CREATE INDEX IF NOT EXISTS idx_workflow_verdicts_run
ON workflow_verdicts(run_id, rule_index);

-- What the user did about a decision: undoing one of our actions in Gmail,
-- adding a label we did not, or an explicit thumbs up or down.
CREATE TABLE IF NOT EXISTS workflow_feedback (
    id                  INTEGER PRIMARY KEY,
    account_email       TEXT NOT NULL,
    message_id          INTEGER NOT NULL REFERENCES workflow_messages(id) ON DELETE CASCADE,
    run_id              INTEGER REFERENCES workflow_runs(id) ON DELETE CASCADE,
    step_id             INTEGER REFERENCES workflow_steps(id) ON DELETE SET NULL,
    rule_index          INTEGER,
    -- 'label_removed' | 'label_added' | 'restored_from_trash' |
    -- 'restored_from_spam' | 'unarchived' | 'unstarred' | 'thumbs_up' | 'thumbs_down'
    kind                TEXT NOT NULL,
    label_id            TEXT NOT NULL DEFAULT '',
    gmail_history_id    TEXT NOT NULL DEFAULT '',
    created_at          TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(message_id, kind, label_id, gmail_history_id)
);

CREATE INDEX IF NOT EXISTS idx_workflow_feedback_step
ON workflow_feedback(step_id);
