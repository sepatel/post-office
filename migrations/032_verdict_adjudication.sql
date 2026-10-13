-- The queue pages newest-first through the rows where the two answers differ,
-- so the index leads on the sort key rather than on the model it filters by.
CREATE INDEX IF NOT EXISTS idx_workflow_verdicts_disagree
ON workflow_verdicts(id DESC) WHERE agrees = 0 AND status = 'ok';

-- Renames the feedback kinds so they read the way they are used. `verdict_right`
-- and `verdict_wrong` were shown as "LLM right?" while the code treated them as
-- confirming the LLM's answer, so the name said the opposite of the meaning.
UPDATE workflow_feedback SET kind = 'llm_right' WHERE kind IN ('thumbs_up', 'verdict_right');
UPDATE workflow_feedback SET kind = 'llm_wrong' WHERE kind IN ('thumbs_down', 'verdict_wrong');