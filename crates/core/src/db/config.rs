use rusqlite::{params, Connection, Result};
use serde::{Deserialize, Serialize};

pub struct ConfigRepository<'a> {
    conn: &'a Connection,
}

impl<'a> ConfigRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn get(&self, key: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT value FROM config WHERE key = ?1")?;
        let mut rows = stmt.query_map(params![key], |row| row.get::<_, String>(0))?;
        rows.next().transpose()
    }

    pub fn set(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO config (key, value, updated_at) VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(key) DO UPDATE SET value = ?2, updated_at = datetime('now')",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_all(&self) -> Result<Vec<ConfigEntry>> {
        let mut stmt = self
            .conn
            .prepare("SELECT key, value, updated_at FROM config ORDER BY key")?;
        let entries = stmt
            .query_map([], |row| {
                Ok(ConfigEntry {
                    key: row.get(0)?,
                    value: row.get(1)?,
                    updated_at: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>>>()?;
        Ok(entries)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigEntry {
    pub key: String,
    pub value: String,
    pub updated_at: String,
}
