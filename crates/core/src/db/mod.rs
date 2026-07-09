use rusqlite::{Connection, Result};
use std::path::Path;
use std::sync::{Arc, Mutex};

pub mod config;
pub mod history;
pub mod rules;

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
        conn.execute_batch(include_str!("../../../../migrations/001_initial.sql"))
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
}
