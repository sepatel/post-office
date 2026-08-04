use rusqlite::{Connection, Result};
use std::path::Path;
use std::sync::{Arc, Mutex};

pub mod config;
pub mod history;
pub mod inference;
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
        let conn = self.conn.lock().unwrap();
        let migrations = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            include_str!("../../../../migrations/001_initial.sql"),
            include_str!("../../../../migrations/002_rule_memory_chat.sql"),
            include_str!("../../../../migrations/003_llm_usage.sql"),
            include_str!("../../../../migrations/004_gmail_sync.sql"),
            include_str!("../../../../migrations/005_history_email_meta.sql"),
            include_str!("../../../../migrations/006_inference_jobs.sql"),
            include_str!("../../../../migrations/007_history_inference_meta.sql"),
            include_str!("../../../../migrations/008_llm_provider_status.sql"),
        );
        conn.execute_batch(&migrations)
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
}
