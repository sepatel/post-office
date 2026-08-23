use rusqlite::{Connection, OptionalExtension, Result, Transaction};
use std::path::Path;
use std::sync::{Arc, Mutex};

pub mod accounts;
pub mod config;
pub mod history;
pub mod inference;
pub mod labels;
pub mod llm_provider_status;
pub mod llm_usage;
pub mod rule_chat;
pub mod rule_memory;
pub mod rules;
pub mod sync_state;

pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Clone for Database {
    fn clone(&self) -> Self {
        Self {
            conn: Arc::clone(&self.conn),
        }
    }
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;

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

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn migrate(&self) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn.transaction()?;
        transaction.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                 version INTEGER PRIMARY KEY,
                 applied_at TEXT NOT NULL DEFAULT (datetime('now'))
             );",
        )?;
        let migrations = [
            include_str!("../../../../migrations/001_initial.sql"),
            include_str!("../../../../migrations/002_rule_memory_chat.sql"),
            include_str!("../../../../migrations/003_llm_usage.sql"),
            include_str!("../../../../migrations/004_gmail_sync.sql"),
            include_str!("../../../../migrations/005_history_email_meta.sql"),
            include_str!("../../../../migrations/006_inference_jobs.sql"),
            include_str!("../../../../migrations/007_history_inference_meta.sql"),
            include_str!("../../../../migrations/008_llm_provider_status.sql"),
        ];
        for (index, migration) in migrations.iter().enumerate() {
            let version = (index + 1) as i64;
            let applied = migration_applied(&transaction, version)?;
            if !applied {
                transaction.execute_batch(migration)?;
                transaction.execute(
                    "INSERT INTO schema_migrations (version) VALUES (?1)",
                    [version],
                )?;
            }
        }
        if !migration_applied(&transaction, 9)? {
            migrate_multi_account_schema(&transaction)?;
            transaction.execute("INSERT INTO schema_migrations (version) VALUES (9)", [])?;
        }
        if !migration_applied(&transaction, 10)? {
            transaction.execute_batch(include_str!(
                "../../../../migrations/010_rule_decision_mode.sql"
            ))?;
            transaction.execute("INSERT INTO schema_migrations (version) VALUES (10)", [])?;
        }
        if !migration_applied(&transaction, 11)? {
            transaction.execute_batch(include_str!(
                "../../../../migrations/011_rule_label_selection.sql"
            ))?;
            transaction.execute("INSERT INTO schema_migrations (version) VALUES (11)", [])?;
        }
        if !migration_applied(&transaction, 12)? {
            transaction
                .execute_batch(include_str!("../../../../migrations/012_gmail_labels.sql"))?;
            transaction.execute("INSERT INTO schema_migrations (version) VALUES (12)", [])?;
        }
        if !migration_applied(&transaction, 13)? {
            transaction.execute_batch(include_str!(
                "../../../../migrations/013_rule_continue_after_match.sql"
            ))?;
            transaction.execute("INSERT INTO schema_migrations (version) VALUES (13)", [])?;
        }
        if !migration_applied(&transaction, 14)? {
            transaction
                .execute_batch(include_str!("../../../../migrations/014_rule_choices.sql"))?;
            transaction.execute("INSERT INTO schema_migrations (version) VALUES (14)", [])?;
        }
        transaction.commit()
    }

    pub fn with_accounts<F, R>(&self, f: F) -> R
    where
        F: FnOnce(accounts::AccountRepository<'_>) -> R,
    {
        let conn = self.conn.lock().unwrap();
        f(accounts::AccountRepository::new(&conn))
    }

    pub fn with_rules<F, R>(&self, f: F) -> R
    where
        F: FnOnce(rules::RuleRepository<'_>) -> R,
    {
        let conn = self.conn.lock().unwrap();
        let repo = rules::RuleRepository::new(&conn);
        f(repo)
    }

    pub fn with_history<F, R>(&self, f: F) -> R
    where
        F: FnOnce(history::HistoryRepository<'_>) -> R,
    {
        let conn = self.conn.lock().unwrap();
        let repo = history::HistoryRepository::new(&conn);
        f(repo)
    }

    pub fn with_config<F, R>(&self, f: F) -> R
    where
        F: FnOnce(config::ConfigRepository<'_>) -> R,
    {
        let conn = self.conn.lock().unwrap();
        let repo = config::ConfigRepository::new(&conn);
        f(repo)
    }

    pub fn with_rule_memory<F, R>(&self, f: F) -> R
    where
        F: FnOnce(rule_memory::RuleMemoryRepository<'_>) -> R,
    {
        let conn = self.conn.lock().unwrap();
        let repo = rule_memory::RuleMemoryRepository::new(&conn);
        f(repo)
    }

    pub fn with_rule_chat<F, R>(&self, f: F) -> R
    where
        F: FnOnce(rule_chat::RuleChatRepository<'_>) -> R,
    {
        let conn = self.conn.lock().unwrap();
        let repo = rule_chat::RuleChatRepository::new(&conn);
        f(repo)
    }

    pub fn with_llm_usage<F, R>(&self, f: F) -> R
    where
        F: FnOnce(llm_usage::LlmUsageRepository<'_>) -> R,
    {
        let conn = self.conn.lock().unwrap();
        let repo = llm_usage::LlmUsageRepository::new(&conn);
        f(repo)
    }

    pub fn with_inference<F, R>(&self, f: F) -> R
    where
        F: FnOnce(inference::InferenceRepository<'_>) -> R,
    {
        let conn = self.conn.lock().unwrap();
        let repo = inference::InferenceRepository::new(&conn);
        f(repo)
    }

    pub fn with_llm_provider_status<F, R>(&self, f: F) -> R
    where
        F: FnOnce(llm_provider_status::LlmProviderStatusRepository<'_>) -> R,
    {
        let conn = self.conn.lock().unwrap();
        let repo = llm_provider_status::LlmProviderStatusRepository::new(&conn);
        f(repo)
    }

    pub fn with_sync_state<F, R>(&self, f: F) -> R
    where
        F: FnOnce(sync_state::SyncStateRepository<'_>) -> R,
    {
        let conn = self.conn.lock().unwrap();
        let repo = sync_state::SyncStateRepository::new(&conn);
        f(repo)
    }

    pub fn with_labels<F, R>(&self, f: F) -> R
    where
        F: FnOnce(labels::LabelRepository<'_>) -> R,
    {
        let conn = self.conn.lock().unwrap();
        let repo = labels::LabelRepository::new(&conn);
        f(repo)
    }
}

/// Timestamps reach the DB either as RFC3339 (written by Rust) or as
/// `datetime('now')`'s space-separated UTC form (written by SQL defaults).
pub fn parse_stored_utc(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    use chrono::{DateTime, Utc};
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|ts| ts.with_timezone(&Utc))
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|naive| DateTime::from_naive_utc_and_offset(naive, Utc))
        })
}

fn migration_applied(transaction: &Transaction<'_>, version: i64) -> Result<bool> {
    transaction
        .query_row(
            "SELECT 1 FROM schema_migrations WHERE version = ?1",
            [version],
            |_| Ok(()),
        )
        .optional()
        .map(|value| value.is_some())
}

fn migrate_multi_account_schema(transaction: &Transaction<'_>) -> Result<()> {
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS accounts (
             email           TEXT PRIMARY KEY,
             sort_order      INTEGER NOT NULL,
             paused          INTEGER NOT NULL DEFAULT 0,
             last_successful TEXT,
             last_error      TEXT,
             created_at      TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
         );",
    )?;
    ensure_account_columns(transaction)?;
    let account_insert = if has_column(transaction, "accounts", "display_name")? {
        "INSERT OR IGNORE INTO accounts (email, display_name, sort_order, last_successful)
         SELECT value, value, 0, datetime('now')
         FROM config
         WHERE key = 'gmail.account' AND TRIM(value) <> '';"
    } else {
        "INSERT OR IGNORE INTO accounts (email, sort_order, last_successful)
         SELECT value, 0, datetime('now')
         FROM config
         WHERE key = 'gmail.account' AND TRIM(value) <> '';"
    };
    transaction.execute_batch(&format!(
        "{account_insert}
         UPDATE accounts
         SET created_at = COALESCE(NULLIF(created_at, ''), datetime('now')),
             updated_at = COALESCE(NULLIF(updated_at, ''), datetime('now'));"
    ))?;

    for table in ["rules", "history", "llm_usage", "inference_jobs"] {
        add_account_column(transaction, table)?;
        transaction.execute(
            &format!(
                "UPDATE {table}
                 SET account_email = COALESCE((SELECT value FROM config WHERE key = 'gmail.account'), '')
                 WHERE account_email = ''"
            ),
            [],
        )?;
    }

    migrate_history_email_meta(transaction)?;
    migrate_pending_events(transaction)?;
    transaction.execute_batch(
        "DROP INDEX IF EXISTS idx_inference_jobs_active;
         CREATE UNIQUE INDEX IF NOT EXISTS idx_inference_jobs_active
         ON inference_jobs(account_email, email_id, rule_id)
         WHERE status IN ('pending', 'running', 'retrying');
         CREATE INDEX IF NOT EXISTS idx_rules_account_enabled
         ON rules(account_email, enabled, priority);
         CREATE INDEX IF NOT EXISTS idx_history_account_created
         ON history(account_email, created_at DESC);
         CREATE INDEX IF NOT EXISTS idx_history_account_email
         ON history(account_email, email_id, created_at DESC);
         CREATE INDEX IF NOT EXISTS idx_history_email_meta_account_sent_at
         ON history_email_meta(account_email, email_sent_at);
         CREATE INDEX IF NOT EXISTS idx_llm_usage_account_date
         ON llm_usage(account_email, created_at);
         CREATE INDEX IF NOT EXISTS idx_inference_jobs_account_due
         ON inference_jobs(account_email, status, next_attempt_at);
         CREATE INDEX IF NOT EXISTS idx_gmail_pending_events_account_status
         ON gmail_pending_events(account_email, status, received_at);",
    )
}

fn ensure_account_columns(transaction: &Transaction<'_>) -> Result<()> {
    for (column, definition) in [
        ("sort_order", "INTEGER NOT NULL DEFAULT 0"),
        ("paused", "INTEGER NOT NULL DEFAULT 0"),
        ("last_successful", "TEXT"),
        ("last_error", "TEXT"),
        ("created_at", "TEXT NOT NULL DEFAULT ''"),
        ("updated_at", "TEXT NOT NULL DEFAULT ''"),
    ] {
        add_column_if_missing(transaction, "accounts", column, definition)?;
    }
    Ok(())
}

fn add_account_column(transaction: &Transaction<'_>, table: &str) -> Result<()> {
    add_column_if_missing(
        transaction,
        table,
        "account_email",
        "TEXT NOT NULL DEFAULT ''",
    )
}

fn add_column_if_missing(
    transaction: &Transaction<'_>,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    if !has_column(transaction, table, column)? {
        transaction.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {definition};"
        ))?;
    }
    Ok(())
}

fn migrate_history_email_meta(transaction: &Transaction<'_>) -> Result<()> {
    const TABLE: &str = "history_email_meta";
    const LEGACY_TABLE: &str = "history_email_meta_legacy";
    if has_table(transaction, LEGACY_TABLE)? {
        create_history_email_meta(transaction)?;
        transaction.execute_batch(
            "INSERT OR IGNORE INTO history_email_meta (account_email, email_id, email_sent_at, updated_at)
             SELECT COALESCE((SELECT value FROM config WHERE key = 'gmail.account'), ''),
                    email_id, email_sent_at, updated_at
             FROM history_email_meta_legacy;
             DROP TABLE history_email_meta_legacy;",
        )?;
    } else if !has_table(transaction, TABLE)? || !has_column(transaction, TABLE, "account_email")? {
        if has_table(transaction, TABLE)? {
            transaction.execute_batch(
                "ALTER TABLE history_email_meta RENAME TO history_email_meta_legacy;",
            )?;
        }
        create_history_email_meta(transaction)?;
        if has_table(transaction, LEGACY_TABLE)? {
            transaction.execute_batch(
                "INSERT OR IGNORE INTO history_email_meta (account_email, email_id, email_sent_at, updated_at)
                 SELECT COALESCE((SELECT value FROM config WHERE key = 'gmail.account'), ''),
                        email_id, email_sent_at, updated_at
                 FROM history_email_meta_legacy;
                 DROP TABLE history_email_meta_legacy;",
            )?;
        }
    }
    Ok(())
}

fn create_history_email_meta(transaction: &Transaction<'_>) -> Result<()> {
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS history_email_meta (
             account_email TEXT NOT NULL,
             email_id      TEXT NOT NULL,
             email_sent_at TEXT,
             updated_at    TEXT NOT NULL DEFAULT (datetime('now')),
             PRIMARY KEY (account_email, email_id)
         );",
    )
}

fn migrate_pending_events(transaction: &Transaction<'_>) -> Result<()> {
    const TABLE: &str = "gmail_pending_events";
    const LEGACY_TABLE: &str = "gmail_pending_events_legacy";
    let needs_rebuild = !has_table(transaction, TABLE)?
        || !has_unique_index(transaction, TABLE, &["account_email", "event_key"])?;
    if !has_table(transaction, LEGACY_TABLE)? && needs_rebuild && has_table(transaction, TABLE)? {
        transaction.execute_batch(
            "ALTER TABLE gmail_pending_events RENAME TO gmail_pending_events_legacy;",
        )?;
    }
    create_pending_events(transaction)?;
    if has_table(transaction, LEGACY_TABLE)? {
        transaction.execute_batch(
            "INSERT OR IGNORE INTO gmail_pending_events (id, account_email, event_key, min_history_id, received_at, status)
             SELECT id, account_email, event_key, min_history_id, received_at, status
             FROM gmail_pending_events_legacy;
             DROP TABLE gmail_pending_events_legacy;",
        )?;
    }
    Ok(())
}

fn create_pending_events(transaction: &Transaction<'_>) -> Result<()> {
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS gmail_pending_events (
             id             INTEGER PRIMARY KEY,
             account_email  TEXT NOT NULL,
             event_key      TEXT NOT NULL,
             min_history_id TEXT NOT NULL,
             received_at    TEXT NOT NULL DEFAULT (datetime('now')),
             status         TEXT NOT NULL DEFAULT 'pending',
             UNIQUE(account_email, event_key)
         );",
    )
}

fn has_table(transaction: &Transaction<'_>, table: &str) -> Result<bool> {
    transaction
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |_| Ok(()),
        )
        .optional()
        .map(|value| value.is_some())
}

fn has_column(transaction: &Transaction<'_>, table: &str, column: &str) -> Result<bool> {
    let mut statement = transaction.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>>>()?;
    Ok(columns.iter().any(|name| name == column))
}

fn has_unique_index(
    transaction: &Transaction<'_>,
    table: &str,
    expected_columns: &[&str],
) -> Result<bool> {
    let mut statement = transaction.prepare(&format!("PRAGMA index_list({table})"))?;
    let indexes = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i32>(2)? != 0))
        })?
        .collect::<Result<Vec<_>>>()?;
    for (name, _) in indexes.into_iter().filter(|(_, unique)| *unique) {
        let mut columns = transaction.prepare(&format!("PRAGMA index_info({name})"))?;
        let columns = columns
            .query_map([], |row| row.get::<_, String>(2))?
            .collect::<Result<Vec<_>>>()?;
        if columns
            .iter()
            .map(String::as_str)
            .eq(expected_columns.iter().copied())
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::Database;

    #[test]
    fn migrations_are_applied_once() {
        let db = Database::open(Path::new(":memory:")).unwrap();
        db.migrate().unwrap();
        db.migrate().unwrap();

        let accounts = db.with_accounts(|repo| repo.list()).unwrap();
        assert!(accounts.is_empty());
    }

    /// The 014 backfill must preserve behavior exactly: a `configured` menu
    /// moves to `choices` while every non-label action stays in the recipe, and
    /// legacy APPLY/SKIP was already just match-or-not over that recipe.
    #[test]
    fn choices_backfill_splits_the_menu_from_the_recipe() {
        use crate::rules::models::Action;

        let db = Database::open(Path::new(":memory:")).unwrap();
        db.migrate().unwrap();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute_batch(
                r#"DELETE FROM schema_migrations WHERE version = 14;
                 ALTER TABLE rules DROP COLUMN choices;
                 ALTER TABLE rules DROP COLUMN choose_from_all_labels;
                 ALTER TABLE rules ADD COLUMN response_mode TEXT NOT NULL DEFAULT 'legacy_token';
                 ALTER TABLE rules ADD COLUMN label_selection TEXT NOT NULL DEFAULT 'none';
                 INSERT INTO rules (account_email, name, conditions, prompt, actions, priority, enabled,
                                    response_mode, label_selection)
                 VALUES ('a@example.com', 'menu', '[]', 'p',
                         '[{"type":"label","value":"L1"},{"type":"archive"}]', 0, 1, 'decision', 'configured'),
                        ('a@example.com', 'every', '[]', 'p', '[]', 1, 1, 'decision', 'all_user_labels'),
                        ('a@example.com', 'legacy', '[]', 'p', '[{"type":"trash"}]', 2, 1, 'legacy_token', 'none');"#,
            )
            .unwrap();
        }

        db.migrate().unwrap();

        let rules = db
            .with_rules(|repo| repo.list_all("a@example.com"))
            .unwrap();
        assert!(matches!(rules[0].choices[..], [Action::Label { .. }]));
        assert!(matches!(rules[0].actions[..], [Action::Archive]));
        assert!(rules[1].choose_from_all_labels);
        assert!(rules[2].choices.is_empty());
        assert!(matches!(rules[2].actions[..], [Action::Trash]));
    }

    #[test]
    fn recovers_when_multi_account_schema_exists_without_its_marker() {
        let db = Database::open(Path::new(":memory:")).unwrap();
        db.migrate().unwrap();
        db.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM schema_migrations WHERE version = 9", [])
            .unwrap();

        db.migrate().unwrap();
    }

    #[test]
    fn recovers_a_partial_accounts_table() {
        let db = Database::open(Path::new(":memory:")).unwrap();
        db.migrate().unwrap();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute_batch(
                "DELETE FROM schema_migrations WHERE version = 9;
                 DROP TABLE accounts;
                 CREATE TABLE accounts (email TEXT PRIMARY KEY, display_name TEXT NOT NULL);
                 INSERT INTO config (key, value) VALUES ('gmail.account', 'recover@example.com');",
            )
            .unwrap();
        }

        db.migrate().unwrap();

        let accounts = db.with_accounts(|repo| repo.list()).unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].email, "recover@example.com");
        db.with_accounts(|repo| repo.add("new@example.com"))
            .unwrap();
    }
}
