# Gmail Push Sync Design

## Problem

Polling with a moving time checkpoint can skip messages when:

- fetch results exceed one page/cycle cap,
- Gmail indexing lags a little behind real arrival,
- per-message failures are treated as cycle success,
- checkpoint advances by wall clock instead of mailbox cursor.

The current app architecture is desktop-only, so the machine cannot expose a
public webhook endpoint directly. Gmail push requires Cloud Pub/Sub delivery,
which means we need a small cloud relay.

## Goals

- Near-real-time inbox processing (seconds, not minutes).
- No missed messages during normal operation.
- Deterministic recovery from dropped notifications/restarts.
- Keep OAuth refresh/access tokens local on the desktop app.
- Keep relay stateless about message content (metadata only).

## Non-Goals

- Multi-account orchestration in one app instance.
- Server-side rule execution (LLM and actions stay on desktop).
- Replacing local SQLite history with cloud storage.

## Target Architecture

```
Gmail -> Pub/Sub topic -> Push webhook (Relay) -> WebSocket -> Desktop Agent
                                                          |
                                                          v
                                                   users.history.list
                                                          |
                                                          v
                                                  local rule pipeline
```

### Components

1. **Desktop app (existing app + new sync agent)**
   - Maintains a persistent outbound WebSocket to relay.
   - Stores `last_history_id` and sync state in SQLite.
   - Calls Gmail `users.history.list` with `startHistoryId`.
   - Fetches full messages for changed IDs and runs existing rule pipeline.

2. **Relay service (new tiny backend)**
   - Public HTTPS endpoint for Pub/Sub push.
   - Verifies Pub/Sub JWT and normalizes notifications.
   - Maps Gmail account -> active desktop session(s).
   - Forwards signal to desktop via WebSocket (or buffers if offline).
   - Exposes small authenticated control API for watch setup/renew.

3. **Google Cloud resources**
   - Pub/Sub topic + push subscription.
   - IAM permission allowing Gmail push publisher account.

## Gmail API Model

Use Gmail push the way it is intended:

1. Call `users.watch` with topic and optional label filter.
2. Gmail sends notification containing `emailAddress` + `historyId`.
3. Desktop receives signal (through relay), then calls
   `users.history.list(startHistoryId=<local last_history_id>)`.
4. Process returned history events until caught up.
5. Persist newest history id after successful local commit.
6. Renew watch before expiration (max 7 days from Gmail).

Important: notifications are hints, not source of truth. `history.list` is the
source of truth and must be replayable.

## Local Data Model (SQLite)

Add tables:

```sql
CREATE TABLE IF NOT EXISTS gmail_sync_state (
    account_email TEXT PRIMARY KEY,
    last_history_id TEXT NOT NULL,
    watch_expiration TEXT,
    watch_status TEXT NOT NULL DEFAULT 'inactive',
    last_notification_at TEXT,
    last_history_pull_at TEXT,
    last_sync_error TEXT,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS gmail_sync_cursor_log (
    id INTEGER PRIMARY KEY,
    account_email TEXT NOT NULL,
    from_history_id TEXT,
    to_history_id TEXT NOT NULL,
    trigger TEXT NOT NULL, -- websocket|startup|sweep|manual
    processed_messages INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS gmail_pending_events (
    id INTEGER PRIMARY KEY,
    account_email TEXT NOT NULL,
    event_key TEXT NOT NULL UNIQUE,
    min_history_id TEXT NOT NULL,
    received_at TEXT NOT NULL DEFAULT (datetime('now')),
    status TEXT NOT NULL DEFAULT 'pending'
);
```

Notes:

- `last_history_id` is the durable mailbox watermark.
- `gmail_pending_events` dedupes repeated push notifications.
- Keep existing `history` table for rule/action logs.

## Relay Data Model (server-side)

Minimal metadata only:

- `sessions(device_id, account_email, ws_connected_at, last_seen_at)`
- `watches(account_email, expiration, status, updated_at)`
- `pending_notifications(account_email, history_id, received_at, delivered_at)`

No email bodies, no OAuth tokens, no rule data.

## Protocol: Desktop <-> Relay

### Desktop -> Relay

- `HELLO { device_id, account_email, app_version, auth_token }`
- `ACK { notification_id, applied_history_id }`
- `WATCH_RENEW_REQUEST { account_email }`
- `HEARTBEAT`

### Relay -> Desktop

- `NOTIFY { notification_id, account_email, history_id }`
- `WATCH_STATUS { expiration, status }`
- `RESYNC_REQUIRED { reason }`

Auth:

- Desktop uses a short-lived relay session token minted via explicit user login
  or device bootstrap flow.
- Token is scoped to one account email.

## Sync Algorithm (Desktop)

On any trigger (`NOTIFY`, startup, periodic sweep):

1. Acquire per-account sync mutex.
2. Read local `last_history_id`.
3. Call `users.history.list(startHistoryId=last_history_id)` with pagination.
4. Collect changed message IDs (dedupe set).
5. For each message ID:
   - fetch full message,
   - evaluate rules,
   - execute actions,
   - write local history/usage logs.
6. Commit highest returned `historyId` to `gmail_sync_state.last_history_id` in
   same DB transaction that records sync cursor log entry.
7. ACK relay notification only after local commit succeeds.

Idempotency:

- Reprocessing the same message is acceptable; actions should be naturally
  idempotent for labels (add/remove operations are safe on repeated calls).
- Keep a short-lived processed cache keyed by `(email_id, history_id)` to reduce
  churn during reconnect storms.

## Recovery and Failure Handling

### Dropped WebSocket / App offline

- Relay buffers last N notifications per account.
- Desktop runs startup reconciliation (`history.list` from last cursor) and does
  not rely on relay replay for correctness.

### `history.list` returns 404 (`startHistoryId` too old)

- Mark state `resync_required`.
- Run bounded backfill query (for example 30 days) to reconstruct watermark.
- Set new `last_history_id` from profile/current history, resume watch mode.

### Watch expired / invalid

- Relay emits `WATCH_STATUS` and attempts renew.
- Desktop also schedules local renew checks as backup.
- Fallback reconciliation sweep runs every X minutes until watch is healthy.

### Relay unavailable

- Desktop degrades to cursor-based reconciliation sweep (still using
  `history.list` if possible).
- Keep legacy polling query path as emergency fallback flag only.

## Security

- OAuth access/refresh tokens remain local in OS keyring (unchanged).
- Relay stores no Gmail tokens and no message content.
- Pub/Sub webhook verifies Google-signed JWT and audience.
- WebSocket requires bearer auth and account/device binding.
- All links TLS-only.

## Observability

Track metrics:

- notification receive -> desktop apply latency (p50/p95),
- history replay batch size and duration,
- watch renew success rate,
- number of 404 resync events,
- skipped/failed message processing counts,
- relay offline duration.

Expose in app diagnostics panel and relay logs.

## Rollout Plan

### Stage 0: Polling hardening first (already needed)

- Add `messages.list` pagination.
- Move checkpointing from wall-clock to safe cursor semantics.

### Stage 1: Cursor sync without push

- Implement `history.list` local replay loop and `last_history_id` persistence.
- Run by periodic reconciliation timer.

### Stage 2: Relay + WebSocket push

- Build relay service with Pub/Sub webhook and desktop sessions.
- Forward notifications to desktop; desktop triggers immediate replay.

### Stage 3: Watch lifecycle automation

- Automate `users.watch` create/renew/stop and health reporting.

### Stage 4: Decommission primary polling

- Keep polling only as emergency feature flag.
- Default mode becomes push + history replay.

## Implementation Notes for This Repo

- Extend `crates/core/src/gmail/client.rs` with:
  - `watch(...)`, `stop_watch(...)`, `history_list(...)`.
- Extend `crates/core/src/gmail/models.rs` with:
  - watch request/response types,
  - history list response + history record models.
- Add new sync module in `crates/core/src/sync/` for cursor replay logic.
- Keep current rule engine and action execution unchanged.
- Add Tauri commands for diagnostics and manual resync.

## Why This Solves "Falling Through the Cracks"

- Cursor (`historyId`) replaces fragile wall-clock checkpointing.
- Replay from cursor tolerates delayed notifications and temporary outages.
- Push shortens reaction time; replay preserves correctness.
- Local durability + idempotent apply removes one-shot processing risk.
