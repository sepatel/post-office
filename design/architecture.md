# Architecture

## System Overview

Post Office is a single-user desktop application that polls Gmail for new emails, applies user-defined rules with AI-powered classification via a local LLM, and executes actions (label, archive, trash, mark spam).

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

## Data Flow

```
1. Poller wakes up (every N minutes, configurable)
   │
2. GmailClient::list_messages("is:unread", max=100)
   │
3. For each email:
   │
   ├── 3a. GmailClient::get_message(id, Format::Full)
   │       → Extract subject, sender, body (plain text), labels, date
   │
   ├── 3b. RuleEngine::evaluate(email, rules)
   │       → Check conditions in priority order
   │       → Return first matching rule (or all matches, configurable)
   │
   ├── 3c. If rule matched:
   │       │
   │       ├── LlmClient::process(rule.prompt + email_content)
   │       │   → Send to local OpenAI-compatible endpoint
   │       │   → Parse response for action instructions
   │       │
   │       └── GmailClient::modify_labels(email_id, add, remove)
   │           → Execute actions (label, archive, trash, spam)
   │
   ├── 3d. Log to history table
   │       (email_id, rule_id, action, status, llm_response, duration)
   │
   └── 3e. Emit Tauri event to update GUI
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
│  │ Polling Task │  │ GUI Event Loop (Tauri)   │ │
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

- **Polling task**: Runs in background tokio task, wakes on interval
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
| LLM malformed response | Retry with stricter prompt, then skip |
| SQLite write failure | Retry once; if persistent, notify user |
| Config parse failure | Use defaults, log warning |
