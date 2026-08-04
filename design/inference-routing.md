# Inference Routing

Post Office treats every OpenAI-compatible endpoint as a provider profile. A
profile identifies an endpoint/model pair and records operational metadata:
quality tier, cost, timeout, enabled state, and the user's privacy assessment.
Privacy metadata is informational unless a routing policy requires local or ZDR
providers. This is intentional because services such as OpenRouter can expose
different retention policies per upstream model.

Rules reference a routing policy by id. A policy contains an ordered list of
provider profile ids, a minimum quality tier, a privacy requirement, and whether
fallback is allowed. The same policy applies to production inference, testing,
bulk evaluation, and rule chat.

The router retries transient provider failures, then moves to the next eligible
candidate. It never chooses a candidate below the policy's quality or privacy
requirements. When no eligible candidate succeeds, production work is retained
in the local inference queue and retried with backoff.

Rate limits are different from ordinary transient failures: a 429 immediately
advances to the next eligible candidate. When a provider reports a reset time,
Post Office persists that cooldown and skips the provider until then. The
Inference Studio shows the reset time so a vendor quota does not create a
request storm or hide why a route is temporarily unavailable.

## Durable Jobs

An inference job stores only the Gmail message id, rule id, source, retry state,
and error metadata. The worker reloads the current rule on every attempt, so
rule edits affect retries by default. Each completed inference records the
provider, model, and policy used in local history.

Gmail cursor replay can advance after a failed inference has been durably
queued. This separates mailbox ingestion from provider availability and means
temporary travel, endpoint, or network failures do not lose work.

Provider API keys for additional profiles are stored in the OS keyring. The
legacy `llm.*` settings remain as the default endpoint migration path.
