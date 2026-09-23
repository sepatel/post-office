ALTER TABLE workflow_llm_attempts ADD COLUMN prompt_tokens INTEGER;
ALTER TABLE workflow_llm_attempts ADD COLUMN completion_tokens INTEGER;
ALTER TABLE workflow_llm_attempts ADD COLUMN total_tokens INTEGER;

CREATE INDEX IF NOT EXISTS idx_workflow_llm_attempts_created
ON workflow_llm_attempts(created_at);
