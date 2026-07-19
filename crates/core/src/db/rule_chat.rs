use rusqlite::{params, Connection, Result};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ChatMessageRow {
    pub id: i64,
    pub rule_id: i64,
    pub role: String,
    pub content: String,
    pub proposal_json: Option<String>,
    pub created_at: String,
}

pub struct RuleChatRepository<'a> {
    conn: &'a Connection,
}

impl<'a> RuleChatRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn list(&self, rule_id: i64) -> Result<Vec<ChatMessageRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, rule_id, role, content, proposal_json, created_at
             FROM rule_chat WHERE rule_id = ?1 ORDER BY created_at ASC, id ASC",
        )?;

        let rows = stmt
            .query_map(params![rule_id], |row| {
                Ok(ChatMessageRow {
                    id: row.get(0)?,
                    rule_id: row.get(1)?,
                    role: row.get(2)?,
                    content: row.get(3)?,
                    proposal_json: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>>>()?;

        Ok(rows)
    }

    pub fn insert(
        &self,
        rule_id: i64,
        role: &str,
        content: &str,
        proposal_json: Option<&str>,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO rule_chat (rule_id, role, content, proposal_json) VALUES (?1, ?2, ?3, ?4)",
            params![rule_id, role, content, proposal_json],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn delete_for_rule(&self, rule_id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM rule_chat WHERE rule_id = ?1", params![rule_id])?;
        Ok(())
    }
}
