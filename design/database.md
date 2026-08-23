# Database Design

## Why SQLite

SQLite is the correct choice for this workload. Concerns about large prompts, millions of history rows, and complex queries are addressed by SQLite's capabilities:

| Concern | SQLite's Answer |
|---------|----------------|
| Large prompts (10K+ tokens) | TEXT columns store up to 1GB. 10K tokens ≈ 40KB — trivial |
| Millions of history rows | Proven at billions. WAL mode handles 12K+ writes/sec |
| Complex rule queries | Full SQL: JOINs, CTEs, JSON1 extension |
| Full-text search | FTS5 built-in: BM25 ranking, prefix search |
| Deployment | Single file, zero dependencies with `bundled` feature |
| Backup | Copy one `.db` file |

## Setup

```rust
// crates/core/src/db/mod.rs

use rusqlite::{Connection, Result};
use std::path::Path;

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;

        // WAL allows concurrent reads while poller writes
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA cache_size = -64000;
            PRAGMA mmap_size = 268435456;
            PRAGMA temp_store = MEMORY;
            PRAGMA page_size = 8192;
            PRAGMA foreign_keys = ON;
        ",
        )?;

        Ok(Self { conn })
    }

    pub fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(include_str!("../migrations/001_initial.sql"))
    }
}
```

## Schema

### Config Table

Stores application configuration as key-value JSON pairs.

```sql
CREATE TABLE config (
    key         TEXT PRIMARY KEY,
    value       TEXT NOT NULL,         -- JSON-serialized value
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Example rows:
-- ('gmail.account_email', '"user@gmail.com"')
-- ('llm.base_url', '"http://localhost:11434/v1"')
-- ('llm.default_model', '"llama3"')
-- ('polling.interval_minutes', '5')
-- ('polling.enabled', 'true')
```

### Rules Table

Hierarchical rules with nested JSON conditions and actions.

```sql
CREATE TABLE rules (
    id          INTEGER PRIMARY KEY,
    parent_id   INTEGER REFERENCES rules(id) ON DELETE SET NULL,
    name        TEXT NOT NULL,
    description TEXT,
    conditions  TEXT NOT NULL,         -- JSON array of conditions
    prompt      TEXT NOT NULL,         -- Free-form LLM prompt
    actions     TEXT NOT NULL,         -- JSON array of actions
    priority    INTEGER DEFAULT 0,
    enabled     INTEGER DEFAULT 1,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_rules_parent ON rules(parent_id);
CREATE INDEX idx_rules_enabled ON rules(enabled) WHERE enabled = 1;
```

Later migrations add `account_email` (009), `continue_after_match` (013), and
`choices` + `choose_from_all_labels` (014, collapsing the `response_mode` and
`label_selection` columns that 010 and 011 introduced). See `migrations/` for
the authoritative schema; the sections below describe the initial tables only.

#### Conditions JSON Format

```json
[
  {
    "type": "from",
    "operator": "contains",
    "value": "newsletter@example.com"
  },
  {
    "type": "and",
    "conditions": [
      { "type": "subject", "operator": "regex", "value": "(?i)weekly digest" },
      { "type": "has_attachment", "value": false }
    ]
  }
]
```

Supported condition types:
- `from` — Sender email/name
- `to` — Recipient
- `subject` — Subject line
- `body` — Email body text
- `has_attachment` — Boolean
- `is_unread` — Boolean
- `label` — Current label
- `date_after` — ISO date
- `date_before` — ISO date
- `and` — Logical AND of sub-conditions
- `or` — Logical OR of sub-conditions
- `not` — Logical NOT of sub-condition

Supported operators:
- `contains` — Substring match
- `equals` — Exact match
- `regex` — Regular expression
- `not_contains` — Negated substring

#### Actions JSON Format

```json
[
  { "type": "label", "value": " newsletters" },
  { "type": "archive" },
  { "type": "mark_read" }
]
```

Supported action types:
- `label` — Apply a Gmail label (created on demand if missing)
- `archive` — Remove from INBOX
- `trash` — Move to trash
- `spam` — Mark as spam
- `mark_read` — Remove UNREAD label
- `mark_unread` — Add UNREAD label
- `star` — Add STARRED label

### History Table

Append-heavy log of every email processed.

```sql
CREATE TABLE history (
    id          INTEGER PRIMARY KEY,
    email_id    TEXT NOT NULL,
    email_from  TEXT,
    email_subject TEXT,
    rule_id     INTEGER REFERENCES rules(id) ON DELETE SET NULL,
    rule_name   TEXT,                   -- Denormalized for display
    action      TEXT NOT NULL,          -- 'labeled', 'archived', 'trashed', 'spammed', 'skipped'
    status      TEXT NOT NULL,          -- 'success', 'failed', 'dry_run'
    llm_model   TEXT,                   -- Which model processed this
    llm_response TEXT,                  -- Full LLM response
    error       TEXT,                   -- Error message if failed
    duration_ms INTEGER,                -- Processing time
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_history_email ON history(email_id, created_at);
CREATE INDEX idx_history_rule ON history(rule_id, created_at);
CREATE INDEX idx_history_date ON history(created_at);
CREATE INDEX idx_history_status ON history(status, created_at);
```

### Audit Table

Tracks all state changes for debugging.

```sql
CREATE TABLE audit (
    id          INTEGER PRIMARY KEY,
    action      TEXT NOT NULL,          -- 'rule.created', 'config.updated', 'gmail.authenticated'
    entity_type TEXT NOT NULL,          -- 'rule', 'config', 'gmail', 'system'
    entity_id   TEXT,
    details     TEXT,                   -- JSON: what changed
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_audit_action ON audit(action, created_at);
CREATE INDEX idx_audit_entity ON audit(entity_type, entity_id, created_at);
```

### Full-Text Search

```sql
CREATE VIRTUAL TABLE history_fts USING fts5(
    email_id,
    email_from,
    email_subject,
    llm_response,
    content=history,
    content_rowid=id
);

-- Triggers to keep FTS in sync
CREATE TRIGGER history_ai AFTER INSERT ON history BEGIN
    INSERT INTO history_fts(rowid, email_id, email_from, email_subject, llm_response)
    VALUES (new.id, new.email_id, new.email_from, new.email_subject, new.llm_response);
END;

CREATE TRIGGER history_ad AFTER DELETE ON history BEGIN
    INSERT INTO history_fts(history_fts, rowid, email_id, email_from, email_subject, llm_response)
    VALUES ('delete', old.id, old.email_id, old.email_from, old.email_subject, old.llm_response);
END;

CREATE TRIGGER history_au AFTER UPDATE ON history BEGIN
    INSERT INTO history_fts(history_fts, rowid, email_id, email_from, email_subject, llm_response)
    VALUES ('delete', old.id, old.email_id, old.email_from, old.email_subject, old.llm_response);
    INSERT INTO history_fts(rowid, email_id, email_from, email_subject, llm_response)
    VALUES (new.id, new.email_id, new.email_from, new.email_subject, new.llm_response);
END;
```

## Query Examples

### Search history by keyword

```sql
SELECT h.*, rank
FROM history h
JOIN history_fts f ON h.id = f.rowid
WHERE history_fts MATCH 'invoice'
ORDER BY rank;
```

### Get recent activity

```sql
SELECT * FROM history
ORDER BY created_at DESC
LIMIT 50;
```

### Get rule statistics

```sql
SELECT
    r.name,
    COUNT(h.id) as total_processed,
    SUM(CASE WHEN h.status = 'success' THEN 1 ELSE 0 END) as successes,
    SUM(CASE WHEN h.status = 'failed' THEN 1 ELSE 0 END) as failures,
    AVG(h.duration_ms) as avg_duration_ms
FROM rules r
LEFT JOIN history h ON r.id = h.rule_id
GROUP BY r.id;
```

### Get emails processed today

```sql
SELECT COUNT(DISTINCT email_id)
FROM history
WHERE created_at >= date('now');
```

## Performance Notes

- **WAL mode** allows concurrent reads while the poller writes history entries
- **Composite indexes** on `(email_id, created_at)` and `(rule_id, created_at)` make date-range queries fast with millions of rows
- **FTS5 triggers** keep the search index in sync automatically
- **Page size 8192** is better for large text (prompts, LLM responses)
- **Memory-mapped I/O** (256MB) speeds up repeated reads
- **Bundled SQLite** means zero system dependencies — the library compiles into the binary

## Backup

The entire database is a single file. Users can:
1. Copy the `.db` file manually
2. Use the SQLite backup API for consistent snapshots without stopping writes
3. Export rules as JSON for portability
