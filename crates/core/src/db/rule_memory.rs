use rusqlite::{params, Connection, Result};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct MemoryEntry {
    pub id: i64,
    pub rule_id: i64,
    pub kind: String,
    pub text: String,
    pub source: String,
    pub created_at: String,
}

pub struct RuleMemoryRepository<'a> {
    conn: &'a Connection,
}

impl<'a> RuleMemoryRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn list_for_rule(&self, rule_id: i64) -> Result<Vec<MemoryEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, rule_id, kind, text, source, created_at
             FROM rule_memory WHERE rule_id = ?1 ORDER BY created_at ASC, id ASC",
        )?;

        let rows = stmt
            .query_map(params![rule_id], |row| {
                Ok(MemoryEntry {
                    id: row.get(0)?,
                    rule_id: row.get(1)?,
                    kind: row.get(2)?,
                    text: row.get(3)?,
                    source: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>>>()?;

        Ok(rows)
    }

    pub fn insert(&self, rule_id: i64, kind: &str, text: &str, source: &str) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO rule_memory (rule_id, kind, text, source) VALUES (?1, ?2, ?3, ?4)",
            params![rule_id, kind, text, source],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn delete_for_rule(&self, rule_id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM rule_memory WHERE rule_id = ?1", params![rule_id])?;
        Ok(())
    }
}
