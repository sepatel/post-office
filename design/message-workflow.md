# Message Workflow

## Purpose

The message workflow is the production processing path. It replaces timestamp
polling and the legacy `history`/`inference_jobs` execution model.

One Gmail message becomes one local `workflow_messages` row and exactly one
automatic `workflow_runs` row. The Gmail message ID is unique per account, so
replayed history pages, reconnects, and rule changes cannot create another run.

## Ingestion

1. The sync loop reads Gmail history from `workflow_mailboxes.history_cursor`.
2. Only `messagesAdded` records create work. Label changes update neither the
   queue nor completed runs, including labels changed by Post Office itself.
3. In one SQLite transaction, arrivals are upserted, runs are queued, and the
   mailbox cursor advances.
4. A first launch records Gmail's current history ID and queues no old mail.
   A history-expiry response reseeds at the current history ID rather than
   silently replaying an arbitrary mailbox range.

Push notifications only wake the history replay loop. They are not the source
of truth.

## Runs

The durable worker prepares runs in queue order, then dispatches each LLM
decision through its endpoint/model lane. A lane uses its configured request
limit, defaulting to one; work for another idle model does not wait behind a
busy lane. A run owns a lease token only while its current preparation or
decision stage is active and moves through `queued`, `processing`,
`retry_wait`, `completed`, `needs_attention`, or `resolved_externally`.

Each run references an immutable ruleset snapshot containing:

- rules, ordering, actions, choices, and learned memories;
- the routing-policy and provider configuration used for its decisions.

Editing a rule, memory, or LLM configuration publishes a new snapshot. New
messages use it; existing runs keep their original behavior.

## Rule Chain

The worker evaluates one message through the ruleset in priority order.
Condition skips and LLM decisions are written to `workflow_steps`. A `NO_MATCH`
advances to the next rule. A match normally completes the run; a rule with
`continue_after_match` advances after its action is confirmed. Each decision
returns to the queue before its next rule, so a message moving from taxonomy to
the default model never blocks unrelated default-model work.

Labels planned by a confirmed earlier action update the local message snapshot
before the next rule runs. Lower-priority rules therefore see the result within
the same deterministic chain.

## Actions And Retries

An LLM decision creates an immutable action plan before Gmail is called. The
action plan stores resolved Gmail label IDs rather than rule text. If a Gmail
request fails after it may have reached Gmail, the worker fetches the message
and verifies the desired labels before retrying.

Decision and action failures receive at most three automatic attempts, including
the first attempt. Exhausted work moves to `needs_attention`; it never returns
to the arrival queue.

Before a manual retry resumes work, the worker fetches the current Gmail message.
Messages deleted, moved to Trash, or archived outside Post Office end as
`resolved_externally`. If Gmail already reflects a persisted action plan, the
plan is confirmed without sending the mutation again.

Queue Retry is one human-requested probe, not a new automatic retry cycle. A
failed probe returns directly to `needs_attention`.

Connection failures open a five-minute circuit for the shared LLM endpoint.
Messages routed to that endpoint then move to `needs_attention` without making
their own connection attempts. A successful human-requested probe closes the
circuit.

External services cannot provide exactly-once delivery across a process crash.
The workflow instead provides exactly-once logical progression: only one
accepted decision and one action plan can advance a run, while Gmail mutations
converge on the desired label state.

## Legacy Boundary

`history`, `inference_jobs`, timestamp polling, direct rule application, and
legacy backfill are archival only. They do not create or retry production work.
The future UI should query workflow messages, runs, steps, action plans, and
events by local message ID.
