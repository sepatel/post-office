ALTER TABLE workflow_runs ADD COLUMN route_provider_id TEXT;
ALTER TABLE workflow_runs ADD COLUMN route_endpoint TEXT;
ALTER TABLE workflow_runs ADD COLUMN route_model TEXT;
ALTER TABLE workflow_runs ADD COLUMN route_max_concurrent_requests INTEGER;

CREATE INDEX IF NOT EXISTS idx_workflow_runs_model_route
ON workflow_runs(
    account_email,
    state,
    route_endpoint,
    route_model,
    next_attempt_at,
    id
)
WHERE route_endpoint IS NOT NULL;
