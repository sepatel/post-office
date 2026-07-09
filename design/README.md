# Post Office — Design Documents

A Rust desktop application for applying AI filters over Gmail emails. Runs locally with OpenAI-compatible LLM endpoints to process, classify, and take action on emails.

## Quick Links

| Document | Description |
|----------|-------------|
| [Architecture](architecture.md) | System architecture, tech stack, project structure |
| [Database](database.md) | SQLite schema, migrations, optimization |
| [Gmail API](gmail-api.md) | REST API wrapper, OAuth2, rate limiting |
| [LLM Integration](llm-integration.md) | OpenAI-compatible client, prompt handling |
| [Rules Engine](rules-engine.md) | Condition matching, action execution |
| [Desktop GUI](desktop-gui.md) | Tauri v2, system tray, window management |
| [Dependencies](dependencies.md) | Crate choices and rationale |
| [Implementation Phases](implementation-phases.md) | Development roadmap |

## Project Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Name | `post-office` | Clear, memorable, describes the domain |
| GUI Framework | Tauri v2 | Built-in system tray, best desktop packaging |
| Frontend | React (via Vite) | Largest ecosystem, Tauri first-class support |
| Gmail Integration | Thin wrapper over REST API with `reqwest` | Full control, no dependency risk |
| LLM Client | `async-openai` | Best local endpoint support, streaming |
| Database | `rusqlite` with `bundled` | Embedded, zero deps, handles millions of rows |
| Auth Storage | `keyring` crate | OS-native credential storage |
| License | MIT | Aligns with all dependencies |
| Multi-account | Single account per instance | Simpler architecture, users run multiple instances |

## Decisions (Resolved)

| Question | Decision |
|----------|----------|
| Email content for LLM | Plain text if available, full HTML as fallback |
| Rule evaluation | First-match-wins (lowest priority number wins) |
| Processing scope | Configurable per-instance (default: `is:unread`) |
| Starter prompt templates | Not for MVP |
| Malformed LLM responses | Strict prompt format, skip email if invalid (no retry) |
| Frontend framework | React (largest ecosystem, Tauri first-class support, least maintenance) |
