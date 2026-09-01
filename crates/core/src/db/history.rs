use rusqlite::{params, Connection, Result};
use serde::Serialize;

pub struct HistoryRepository<'a> {
    conn: &'a Connection,
}

impl<'a> HistoryRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn insert(&self, account_email: &str, entry: &NewHistoryEntry) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO history (account_email, email_id, email_from, email_subject, rule_id, rule_name, action, status, llm_model, llm_response, error, duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                account_email,
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

        let id = self.conn.last_insert_rowid();
        if entry.llm_provider.is_some() || entry.policy_id.is_some() {
            self.conn.execute(
                "INSERT INTO history_inference_meta (history_id, provider_id, policy_id)
                 VALUES (?1, ?2, ?3)",
                params![id, entry.llm_provider, entry.policy_id],
            )?;
        }
        Ok(id)
    }

    pub fn upsert_email_sent_at(
        &self,
        account_email: &str,
        email_id: &str,
        email_sent_at: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO history_email_meta (account_email, email_id, email_sent_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(account_email, email_id) DO UPDATE SET
                 email_sent_at = COALESCE(excluded.email_sent_at, history_email_meta.email_sent_at),
                 updated_at = datetime('now')",
            params![account_email, email_id, email_sent_at],
        )?;
        Ok(())
    }

    pub fn list(&self, account_email: &str, page: u32, per_page: u32) -> Result<Vec<HistoryEntry>> {
        let offset = page * per_page;
        let mut stmt = self.conn.prepare(
            "SELECT h.id, h.email_id, h.email_from, h.email_subject, h.rule_id, h.rule_name, h.action, h.status,
                    h.llm_model, h.llm_response, h.error, h.duration_ms, h.created_at,
                    m.email_sent_at, im.provider_id, im.policy_id
             FROM history h
              LEFT JOIN history_email_meta m ON m.account_email = h.account_email AND m.email_id = h.email_id
              LEFT JOIN history_inference_meta im ON im.history_id = h.id
              WHERE h.account_email = ?1
              ORDER BY h.created_at DESC
              LIMIT ?2 OFFSET ?3",
        )?;

        let entries = stmt
            .query_map(params![account_email, per_page, offset], |row| {
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
                    email_sent_at: row.get(13)?,
                    llm_provider: row.get(14)?,
                    policy_id: row.get(15)?,
                })
            })?
            .collect::<Result<Vec<_>>>()?;

        Ok(entries)
    }

    pub fn by_email(&self, account_email: &str, email_id: &str) -> Result<Vec<HistoryEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT h.id, h.email_id, h.email_from, h.email_subject, h.rule_id, h.rule_name, h.action, h.status,
                    h.llm_model, h.llm_response, h.error, h.duration_ms, h.created_at,
                    m.email_sent_at, im.provider_id, im.policy_id
             FROM history h
              LEFT JOIN history_email_meta m ON m.account_email = h.account_email AND m.email_id = h.email_id
              LEFT JOIN history_inference_meta im ON im.history_id = h.id
              WHERE h.account_email = ?1 AND h.email_id = ?2
              ORDER BY h.created_at DESC",
        )?;

        let entries = stmt
            .query_map(params![account_email, email_id], |row| {
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
                    email_sent_at: row.get(13)?,
                    llm_provider: row.get(14)?,
                    policy_id: row.get(15)?,
                })
            })?
            .collect::<Result<Vec<_>>>()?;

        Ok(entries)
    }

    pub fn by_rule(
        &self,
        account_email: &str,
        rule_id: i64,
        page: u32,
        per_page: u32,
    ) -> Result<Vec<HistoryEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT h.id, h.email_id, h.email_from, h.email_subject, h.rule_id, h.rule_name, h.action, h.status,
                    h.llm_model, h.llm_response, h.error, h.duration_ms, h.created_at,
                    m.email_sent_at, im.provider_id, im.policy_id
             FROM history h
              LEFT JOIN history_email_meta m ON m.account_email = h.account_email AND m.email_id = h.email_id
              LEFT JOIN history_inference_meta im ON im.history_id = h.id
             WHERE h.account_email = ?1 AND h.rule_id = ?2
             ORDER BY h.created_at DESC
             LIMIT ?3 OFFSET ?4",
        )?;
        let entries = stmt
            .query_map(
                params![
                    account_email,
                    rule_id,
                    per_page,
                    page.saturating_mul(per_page)
                ],
                |row| {
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
                        email_sent_at: row.get(13)?,
                        llm_provider: row.get(14)?,
                        policy_id: row.get(15)?,
                    })
                },
            )?
            .collect();
        entries
    }

    pub fn latest_created_at(&self, account_email: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT created_at FROM history WHERE account_email = ?1 ORDER BY created_at DESC LIMIT 1")?;
        let mut rows = stmt.query_map(params![account_email], |row| row.get::<_, String>(0))?;
        rows.next().transpose()
    }

    pub fn rules_metrics(&self, account_email: &str) -> Result<Vec<RuleMetrics>> {
        let mut stmt = self.conn.prepare(
            "SELECT
                rule_id,
                SUM(CASE WHEN created_at >= datetime('now', '-24 hours') THEN 1 ELSE 0 END) AS checked_24h,
                SUM(CASE WHEN created_at >= datetime('now', '-24 hours') AND status = 'success' THEN 1 ELSE 0 END) AS succeeded_24h,
                SUM(CASE WHEN created_at >= datetime('now', '-24 hours') AND (action = 'RESOLVE_RULE' OR COALESCE(TRIM(llm_response), '') <> '') THEN 1 ELSE 0 END) AS llm_calls_24h,
                COUNT(*) AS checked_7d,
                SUM(CASE WHEN status = 'success' THEN 1 ELSE 0 END) AS succeeded_7d,
                SUM(CASE WHEN action = 'RESOLVE_RULE' OR COALESCE(TRIM(llm_response), '') <> '' THEN 1 ELSE 0 END) AS llm_calls_7d
             FROM history
              WHERE account_email = ?1
                AND rule_id IS NOT NULL
                AND created_at >= datetime('now', '-7 days')
             GROUP BY rule_id",
        )?;

        let entries = stmt
            .query_map(params![account_email], |row| {
                Ok(RuleMetrics {
                    rule_id: row.get(0)?,
                    checked_24h: row.get(1)?,
                    succeeded_24h: row.get(2)?,
                    llm_calls_24h: row.get(3)?,
                    checked_7d: row.get(4)?,
                    succeeded_7d: row.get(5)?,
                    llm_calls_7d: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>>>()?;

        Ok(entries)
    }

    pub fn search(&self, account_email: &str, query: &str) -> Result<Vec<HistoryEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT h.id, h.email_id, h.email_from, h.email_subject, h.rule_id, h.rule_name,
                    h.action, h.status, h.llm_model, h.llm_response, h.error, h.duration_ms, h.created_at,
                     m.email_sent_at, im.provider_id, im.policy_id
             FROM history h
             JOIN history_fts f ON h.id = f.rowid
              LEFT JOIN history_email_meta m ON m.account_email = h.account_email AND m.email_id = h.email_id
              LEFT JOIN history_inference_meta im ON im.history_id = h.id
              WHERE history_fts MATCH ?1 AND h.account_email = ?2
              ORDER BY rank",
        )?;

        let entries = stmt
            .query_map(params![query, account_email], |row| {
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
                    email_sent_at: row.get(13)?,
                    llm_provider: row.get(14)?,
                    policy_id: row.get(15)?,
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
    pub llm_provider: Option<String>,
    pub policy_id: Option<String>,
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
    pub email_sent_at: Option<String>,
    pub llm_provider: Option<String>,
    pub policy_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuleMetrics {
    pub rule_id: i64,
    pub checked_24h: i64,
    pub succeeded_24h: i64,
    pub llm_calls_24h: i64,
    pub checked_7d: i64,
    pub succeeded_7d: i64,
    pub llm_calls_7d: i64,
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::NewHistoryEntry;
    use crate::db::Database;

    fn entry() -> NewHistoryEntry {
        NewHistoryEntry {
            email_id: "shared-message-id".into(),
            email_from: None,
            email_subject: None,
            rule_id: None,
            rule_name: None,
            action: "SKIP".into(),
            status: "skipped".into(),
            llm_model: None,
            llm_response: None,
            error: None,
            duration_ms: None,
            llm_provider: None,
            policy_id: None,
        }
    }

    #[test]
    fn scopes_same_message_id_to_its_account() {
        let db = Database::open(Path::new(":memory:")).unwrap();
        db.migrate().unwrap();
        db.with_history(|repo| {
            repo.insert("first@example.com", &entry())?;
            repo.insert("second@example.com", &entry())?;
            repo.upsert_email_sent_at(
                "first@example.com",
                "shared-message-id",
                "2026-01-01T00:00:00Z",
            )?;
            repo.upsert_email_sent_at(
                "second@example.com",
                "shared-message-id",
                "2026-02-01T00:00:00Z",
            )?;
            Ok::<(), rusqlite::Error>(())
        })
        .unwrap();

        let first = db
            .with_history(|repo| repo.by_email("first@example.com", "shared-message-id"))
            .unwrap();
        let second = db
            .with_history(|repo| repo.by_email("second@example.com", "shared-message-id"))
            .unwrap();

        assert_eq!(first.len(), 1);
        assert_eq!(second.len(), 1);
        assert_eq!(
            first[0].email_sent_at.as_deref(),
            Some("2026-01-01T00:00:00Z")
        );
        assert_eq!(
            second[0].email_sent_at.as_deref(),
            Some("2026-02-01T00:00:00Z")
        );
    }
}
