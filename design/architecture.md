# Architecture

## System Overview

Post Office is a single-user desktop application that applies user-defined
rules to Gmail with AI-powered classification via a local LLM and executes
actions (label, archive, trash, mark spam).

Current ingestion is interval polling. Target ingestion is Gmail push
notifications (`users.watch`) plus durable cursor replay (`users.history.list`).

```
┌─────────────────────────────────────────────────────────────────┐
│                       Desktop GUI (Tauri v2)                    │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────┐ │
│  │ System Tray  │  │  Rule Editor │  │  Processing History    │ │
│  │ (icon, menu) │  │  (CRUD UI)   │  │  (search, filter)      │ │
│  └──────────────┘  └──────────────┘  └────────────────────────┘ │
└────────────────────────────┬────────────────────────────────────┘
                             │ IPC (Tauri commands)
┌────────────────────────────┴────────────────────────────────────┐
│                        Core Engine (Rust)                        │
│                                                                 │
│  ┌─────────────────┐  ┌─────────────────┐  ┌────────────────┐  │
│  │  Gmail Client    │  │  LLM Client     │  │  Rule Engine   │  │
│  │  (REST + OAuth2) │  │  (async-openai) │  │  (match + act) │  │
│  └────────┬────────┘  └────────┬────────┘  └───────┬────────┘  │
│           │                    │                    │            │
│  ┌────────┴────────────────────┴────────────────────┴────────┐  │
│  │                    SQLite (rusqlite)                       │  │
│  │  rules | history | audit | config                         │  │
│  └───────────────────────────────────────────────────────────┘  │
│                                                                 │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │              Background Polling Service                    │  │
│  │  (tokio::spawn, configurable interval)                    │  │
│  └───────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

## Push Target State

```
┌───────────────────────────── Google Cloud ─────────────────────────────┐
│ Gmail -> Pub/Sub topic -> Push subscription -> Relay webhook/API/WS    │
└───────────────────────────────────┬─────────────────────────────────────┘
                                    │ outbound WS
┌───────────────────────────────────▼─────────────────────────────────────┐
│                      Desktop App (Tauri + Core)                        │
│                                                                         │
│  Sync Agent -> users.history.list replay -> Rule Engine -> Gmail ops   │
│        |                                                                │
│        v                                                                │
│    sqlite cursor state (`last_history_id`) + processing history         │
└─────────────────────────────────────────────────────────────────────────┘
```

See `design/gmail-push-sync.md` for protocol and rollout details.

## Data Flow

```
1. Sync trigger fires (push notification, startup replay, or safety sweep)
   │
2. Sync agent reads persisted `last_history_id`
   │
3. GmailClient::history_list(startHistoryId=...)
   │
4. Collect changed message IDs (deduped)
   │
5. For each email:
   │
   ├── 5a. GmailClient::get_message(id, Format::Full)
   │       → Extract subject, sender, body (plain text), labels, date
   │
   ├── 5b. RuleEngine::evaluate(email, rules)
   │       → Check conditions in priority order
   │       → Return first matching rule (lowest priority number)
   │
   ├── 5c. If rule matched:
   │       │
   │       ├── If rule.prompt is empty:
   │       │     → Resolve configured structured actions locally (no LLM call)
   │       │
   │       ├── Else LlmClient::process(rule.prompt + email_content)
   │       │   → Parse first non-empty token line (APPLY, SKIP, explicit action)
   │       │   → Hybrid resolution:
   │       │       APPLY => run configured structured actions
   │       │       SKIP / invalid => no action
   │       │       ARCHIVE/TRASH/SPAM/MARK_READ/MARK_UNREAD/STAR/LABEL:<name> => execute token directly
   │       │
   │       └── GmailClient::modify_labels(email_id, add, remove)
   │           → Execute resolved Gmail label mutations
   │
   ├── 5d. Log to history table
   │       (email_id, rule_id, action, status, llm_response, duration)
   │
   ├── 5e. Persist highest replayed history id
   │
   └── 5f. Emit Tauri event to update GUI
```

## Workspace Structure

```
post-office/
├── Cargo.toml                    # Workspace root
├── crates/
│   └── core/                     # Core logic (no GUI dependencies)
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs
│           ├── gmail/            # Gmail REST API wrapper
│           ├── llm/              # LLM client
│           ├── rules/            # Rule engine
│           ├── db/               # SQLite layer
│           └── config.rs         # Configuration
├── src-tauri/                    # Tauri application shell
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   └── src/
│       ├── main.rs
│       ├── lib.rs               # Builder + tray
│       ├── commands.rs          # IPC commands
│       └── tray.rs              # System tray
├── src/                          # React frontend
│   ├── App.tsx
│   ├── pages/
│   └── components/
└── migrations/
    └── 001_initial.sql
```

## Crate Separation Rationale

The `crates/core` crate contains all business logic with zero GUI dependencies. This enables:

- **Unit testing** without Tauri/WebView runtime
- **CLI tool** in the future (same core, different frontend)
- **Library reuse** if we add a server mode later
- **Faster compilation** when editing core logic

The `src-tauri` crate is thin: it wires Tauri IPC commands to core functions and manages the system tray.

## Concurrency Model

```
┌─────────────────────────────────────────────────┐
│                 Tokio Runtime                    │
│                                                  │
│  ┌──────────────┐  ┌──────────────────────────┐ │
│  │ Sync Task    │  │ GUI Event Loop (Tauri)   │ │
│  │ (spawn)      │  │                          │ │
│  └──────┬───────┘  └────────────┬─────────────┘ │
│         │                       │                │
│         └───────────┬───────────┘                │
│                     │                            │
│              ┌──────┴──────┐                     │
│              │ Shared State │                     │
│              │ (Arc<Mutex>) │                     │
│              └─────────────┘                     │
└─────────────────────────────────────────────────┘
```

- **Sync task**: Runs in background tokio task, reacts to push + replay timers
- **GUI**: Tauri's event loop, communicates via IPC commands
- **Shared state**: Processing status, current email count, error state
- **SQLite**: Single connection with WAL mode (concurrent reads, serialized writes)

## Error Strategy

| Error Type | Handling |
|------------|----------|
| Gmail API transient (429, 503) | Exponential backoff with jitter, 5 retries |
| Gmail API auth (401) | Refresh token, retry once; if refresh fails, notify user |
| Gmail API permanent (400, 404) | Log error, skip email, continue |
| LLM endpoint unreachable | Log error, skip email, mark rule as failed |
| LLM malformed response | Log response, skip email (no retry) |
| SQLite write failure | Retry once; if persistent, notify user |
| Config parse failure | Use defaults, log warning |
