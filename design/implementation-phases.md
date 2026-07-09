# Implementation Phases

## Phase 1: Foundation (Week 1-2)

**Goal:** Working Gmail authentication + basic email reading + project structure

### Tasks

- [ ] Initialize Cargo workspace with `crates/core`
- [ ] Initialize Tauri + React project in `src-tauri/` and `src/`
- [ ] Set up SQLite database with migrations
- [ ] Implement Gmail OAuth2 flow:
  - [ ] Local HTTP server for redirect
  - [ ] Browser-based consent screen
  - [ ] Token exchange
  - [ ] Token storage in keyring
  - [ ] Automatic token refresh
- [ ] Implement Gmail REST API client:
  - [ ] `list_messages` (with query)
  - [ ] `get_message` (full format)
  - [ ] `list_labels`
  - [ ] Error handling + retry
- [ ] Basic Tauri setup:
  - [ ] System tray with show/hide/quit
  - [ ] Window minimize-to-tray
  - [ ] IPC command for gmail_authenticate
- [ ] React frontend skeleton:
  - [ ] Layout with sidebar
  - [ ] Settings page (placeholder)
  - [ ] Tauri invoke wrappers

### Deliverable

A user can authenticate with Gmail, see their inbox emails listed in the app, and the app sits in the system tray.

---

## Phase 2: Core Engine (Week 3-4)

**Goal:** Rules engine + LLM integration + background processing

### Tasks

- [ ] Implement rule engine:
  - [ ] Condition types + evaluation
  - [ ] Action types + execution
  - [ ] Rule priority ordering
  - [ ] CRUD operations in SQLite
- [ ] Implement LLM client:
  - [ ] async-openai wrapper
  - [ ] Configurable base URL + model
  - [ ] Prompt construction from email + rule
  - [ ] Response parsing
  - [ ] Error handling
- [ ] Implement background polling:
  - [ ] Configurable interval
  - [ ] Fetch unread emails
  - [ ] Evaluate rules per email
  - [ ] Call LLM
  - [ ] Execute Gmail actions
  - [ ] Log to history + audit
- [ ] Implement history + audit logging
- [ ] IPC commands:
  - [ ] `rules_list`, `rules_create`, `rules_update`, `rules_delete`
  - [ ] `history_list`, `history_search`
  - [ ] `processing_pause`, `processing_resume`, `processing_status`

### Deliverable

A user can create rules, the app polls Gmail periodically, processes emails through the LLM, applies actions, and logs everything.

---

## Phase 3: GUI (Week 5-6)

**Goal:** Complete desktop UI for all features

### Tasks

- [ ] Dashboard page:
  - [ ] Processing status display
  - [ ] Today's stats
  - [ ] Recent activity list
  - [ ] Quick action buttons
- [ ] Rules page:
  - [ ] Rule list with enable/disable toggles
  - [ ] Drag-and-drop reordering
  - [ ] Delete with confirmation
- [ ] Rule Editor page:
  - [ ] Name + description fields
  - [ ] Condition builder (visual)
  - [ ] Prompt editor (large textarea)
  - [ ] Action checkboxes
  - [ ] Priority slider
  - [ ] Test/dry-run button
- [ ] History page:
  - [ ] Paginated list
  - [ ] Search bar (full-text)
  - [ ] Filter dropdowns (date, rule, action, status)
  - [ ] Expandable row with email preview + LLM response
- [ ] Settings page:
  - [ ] Gmail connection status + re-auth
  - [ ] LLM endpoint form (URL, key, model)
  - [ ] Polling interval slider
  - [ ] Import/export rules
- [ ] Desktop notifications:
  - [ ] On important email processed
  - [ ] On error
- [ ] Tray updates:
  - [ ] Dynamic tooltip with status
  - [ ] Pause/resume from tray menu

### Deliverable

A polished desktop application with full UI for managing rules, viewing history, and configuring settings.

---

## Phase 4: Polish (Week 7-8)

**Goal:** Production-ready release

### Tasks

- [ ] Error handling review:
  - [ ] Graceful degradation
  - [ ] User-friendly error messages
  - [ ] Recovery from transient failures
- [ ] Testing:
  - [ ] Unit tests for rule engine
  - [ ] Unit tests for LLM response parsing
  - [ ] Integration tests for Gmail client (mock)
  - [ ] E2E test for processing pipeline
- [ ] Packaging:
  - [ ] macOS: DMG
  - [ ] Windows: MSI
  - [ ] Linux: AppImage, deb, rpm
- [ ] Documentation:
  - [ ] README with setup instructions
  - [ ] User guide for rules
  - [ ] Configuration reference
- [ ] CI/CD:
  - [ ] GitHub Actions for build + test
  - [ ] Release automation
- [ ] Code review + cleanup:
  - [ ] Remove dead code
  - [ ] Consistent error handling
  - [ ] License headers

### Deliverable

A v0.1.0 release ready for early adopters.

---

## Future Considerations (Post-v0.1.0)

These are NOT in scope for initial release but inform architecture decisions:

| Feature | Impact on Current Design |
|---------|--------------------------|
| Multiple accounts | DB schema uses `account_id` column (future-proofed) |
| IMAP support | Gmail client behind a trait (swappable) |
| Custom LLM prompts library | Prompt field is free-form (no constraint) |
| Email templates for notifications | Tauri notification plugin already included |
| Auto-update | Tauri has built-in updater plugin |
| System-wide hotkeys | Can add `tauri-plugin-global-shortcut` |
| CLI mode | Core crate has no GUI deps (reusable) |
| Server mode | Core crate could be used in a web server |
