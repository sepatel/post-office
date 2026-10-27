CREATE INDEX IF NOT EXISTS idx_workflow_steps_rule_created
ON workflow_steps(rule_legacy_id, created_at DESC);
