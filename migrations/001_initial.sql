CREATE TABLE IF NOT EXISTS config (
    key         TEXT PRIMARY KEY,
    value       TEXT NOT NULL,
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS rules (
    id          INTEGER PRIMARY KEY,
    parent_id   INTEGER REFERENCES rules(id) ON DELETE SET NULL,
    name        TEXT NOT NULL,
    description TEXT,
    conditions  TEXT NOT NULL,
    prompt      TEXT NOT NULL,
    actions     TEXT NOT NULL,
    priority    INTEGER DEFAULT 0,
    enabled     INTEGER DEFAULT 1,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_rules_parent ON rules(parent_id);
CREATE INDEX IF NOT EXISTS idx_rules_enabled ON rules(enabled) WHERE enabled = 1;

CREATE TABLE IF NOT EXISTS history (
    id            INTEGER PRIMARY KEY,
    email_id      TEXT NOT NULL,
    email_from    TEXT,
    email_subject TEXT,
    rule_id       INTEGER REFERENCES rules(id) ON DELETE SET NULL,
    rule_name     TEXT,
    action        TEXT NOT NULL,
    status        TEXT NOT NULL,
    llm_model     TEXT,
    llm_response  TEXT,
    error         TEXT,
    duration_ms   INTEGER,
    created_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_history_email ON history(email_id, created_at);
CREATE INDEX IF NOT EXISTS idx_history_rule ON history(rule_id, created_at);
CREATE INDEX IF NOT EXISTS idx_history_date ON history(created_at);
CREATE INDEX IF NOT EXISTS idx_history_status ON history(status, created_at);

CREATE TABLE IF NOT EXISTS audit (
    id          INTEGER PRIMARY KEY,
    action      TEXT NOT NULL,
    entity_type TEXT NOT NULL,
    entity_id   TEXT,
    details     TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_audit_action ON audit(action, created_at);
CREATE INDEX IF NOT EXISTS idx_audit_entity ON audit(entity_type, entity_id, created_at);

CREATE VIRTUAL TABLE IF NOT EXISTS history_fts USING fts5(
    email_id,
    email_from,
    email_subject,
    llm_response,
    content=history,
    content_rowid=id
);

CREATE TRIGGER IF NOT EXISTS history_ai AFTER INSERT ON history BEGIN
    INSERT INTO history_fts(rowid, email_id, email_from, email_subject, llm_response)
    VALUES (new.id, new.email_id, new.email_from, new.email_subject, new.llm_response);
END;

CREATE TRIGGER IF NOT EXISTS history_ad AFTER DELETE ON history BEGIN
    INSERT INTO history_fts(history_fts, rowid, email_id, email_from, email_subject, llm_response)
    VALUES ('delete', old.id, old.email_id, old.email_from, old.email_subject, old.llm_response);
END;

CREATE TRIGGER IF NOT EXISTS history_au AFTER UPDATE ON history BEGIN
    INSERT INTO history_fts(history_fts, rowid, email_id, email_from, email_subject, llm_response)
    VALUES ('delete', old.id, old.email_id, old.email_from, old.email_subject, old.llm_response);
    INSERT INTO history_fts(rowid, email_id, email_from, email_subject, llm_response)
    VALUES (new.id, new.email_id, new.email_from, new.email_subject, new.llm_response);
END;
