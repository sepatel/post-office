-- Names the choice each multiple-match verdict asked about. `verdict_choice`
-- only carries a name when rverdict matched, so without this a "no" row is
-- anonymous and the user cannot tell which label it refers to.
--
-- Lets feedback be recorded against one label rather than the whole step, so
-- being wrong about Mailspring does not mark the other 56 labels wrong too.
-- Existing rows keep NULL: their framing predates this and cannot be mapped
-- back without re-deriving the menu from the run's rule snapshot.
ALTER TABLE workflow_verdicts ADD COLUMN target TEXT;

ALTER TABLE workflow_feedback ADD COLUMN verdict_id INTEGER REFERENCES workflow_verdicts(id) ON DELETE CASCADE;

CREATE INDEX IF NOT EXISTS idx_workflow_feedback_verdict
ON workflow_feedback(verdict_id);
