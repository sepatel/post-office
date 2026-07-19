use rusqlite::{params, Connection, Result};
use serde::Serialize;

pub struct HistoryRepository<'a> {
    conn: &'a Connection,
}

impl<'a> HistoryRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn insert(&self, entry: &NewHistoryEntry) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO history (email_id, email_from, email_subject, rule_id, rule_name, action, status, llm_model, llm_response, error, duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                entry.email_id,
                entry.email_from,
                entry.email_subject,
                entry.rule_id,
                entry.rule_name,
                entry.action,
                entry.status,
                entry.llm_model,
                entry.llm_response,
                entry.error,
                entry.duration_ms,
            ],
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    pub fn list(&self, page: u32, per_page: u32) -> Result<Vec<HistoryEntry>> {
        let offset = page * per_page;
        let mut stmt = self.conn.prepare(
            "SELECT id, email_id, email_from, email_subject, rule_id, rule_name, action, status,
                    llm_model, llm_response, error, duration_ms, created_at
             FROM history
             ORDER BY created_at DESC
             LIMIT ?1 OFFSET ?2",
        )?;

        let entries = stmt
            .query_map(params![per_page, offset], |row| {
                Ok(HistoryEntry {
                    id: row.get(0)?,
                    email_id: row.get(1)?,
                    email_from: row.get(2)?,
                    email_subject: row.get(3)?,
                    rule_id: row.get(4)?,
                    rule_name: row.get(5)?,
                    action: row.get(6)?,
                    status: row.get(7)?,
                    llm_model: row.get(8)?,
                    llm_response: row.get(9)?,
                    error: row.get(10)?,
                    duration_ms: row.get(11)?,
                    created_at: row.get(12)?,
                })
            })?
            .collect::<Result<Vec<_>>>()?;

        Ok(entries)
    }

    pub fn by_email(&self, email_id: &str) -> Result<Vec<HistoryEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, email_id, email_from, email_subject, rule_id, rule_name, action, status,
                    llm_model, llm_response, error, duration_ms, created_at
             FROM history
             WHERE email_id = ?1
             ORDER BY created_at DESC",
        )?;

        let entries = stmt
            .query_map(params![email_id], |row| {
                Ok(HistoryEntry {
                    id: row.get(0)?,
                    email_id: row.get(1)?,
                    email_from: row.get(2)?,
                    email_subject: row.get(3)?,
                    rule_id: row.get(4)?,
                    rule_name: row.get(5)?,
                    action: row.get(6)?,
                    status: row.get(7)?,
                    llm_model: row.get(8)?,
                    llm_response: row.get(9)?,
                    error: row.get(10)?,
                    duration_ms: row.get(11)?,
                    created_at: row.get(12)?,
                })
            })?
            .collect::<Result<Vec<_>>>()?;

        Ok(entries)
    }

    pub fn latest_created_at(&self) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT created_at FROM history ORDER BY created_at DESC LIMIT 1")?;
        let mut rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.next().transpose()
    }

    pub fn search(&self, query: &str) -> Result<Vec<HistoryEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT h.id, h.email_id, h.email_from, h.email_subject, h.rule_id, h.rule_name,
                    h.action, h.status, h.llm_model, h.llm_response, h.error, h.duration_ms, h.created_at
             FROM history h
             JOIN history_fts f ON h.id = f.rowid
             WHERE history_fts MATCH ?1
             ORDER BY rank",
        )?;

        let entries = stmt
            .query_map(params![query], |row| {
                Ok(HistoryEntry {
                    id: row.get(0)?,
                    email_id: row.get(1)?,
                    email_from: row.get(2)?,
                    email_subject: row.get(3)?,
                    rule_id: row.get(4)?,
                    rule_name: row.get(5)?,
                    action: row.get(6)?,
                    status: row.get(7)?,
                    llm_model: row.get(8)?,
                    llm_response: row.get(9)?,
                    error: row.get(10)?,
                    duration_ms: row.get(11)?,
                    created_at: row.get(12)?,
                })
            })?
            .collect::<Result<Vec<_>>>()?;

        Ok(entries)
    }
}

#[derive(Debug, Clone)]
pub struct NewHistoryEntry {
    pub email_id: String,
    pub email_from: Option<String>,
    pub email_subject: Option<String>,
    pub rule_id: Option<i64>,
    pub rule_name: Option<String>,
    pub action: String,
    pub status: String,
    pub llm_model: Option<String>,
    pub llm_response: Option<String>,
    pub error: Option<String>,
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryEntry {
    pub id: i64,
    pub email_id: String,
    pub email_from: Option<String>,
    pub email_subject: Option<String>,
    pub rule_id: Option<i64>,
    pub rule_name: Option<String>,
    pub action: String,
    pub status: String,
    pub llm_model: Option<String>,
    pub llm_response: Option<String>,
    pub error: Option<String>,
    pub duration_ms: Option<i64>,
    pub created_at: String,
}
