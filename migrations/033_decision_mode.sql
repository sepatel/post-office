-- Phase 5: verdict_then_llm gate + per-rule calibration + live/shadow origin.
-- Additive only. Old builds ignore the new columns/table.
ALTER TABLE rules ADD COLUMN decision_mode TEXT NOT NULL DEFAULT 'llm';
ALTER TABLE rules ADD COLUMN verdict_threshold REAL NULL;
ALTER TABLE workflow_verdicts ADD COLUMN origin TEXT NOT NULL DEFAULT 'shadow';
CREATE TABLE IF NOT EXISTS verdict_calibration (
    account_email   TEXT NOT NULL,
    rule_legacy_id  INTEGER NOT NULL,
    framing         TEXT NOT NULL,
    model           TEXT NOT NULL,
    params_json     TEXT NOT NULL,
    n               INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (account_email, rule_legacy_id, framing, model)
);
CREATE INDEX IF NOT EXISTS idx_verdict_calibration_rule
ON verdict_calibration(account_email, rule_legacy_id);
