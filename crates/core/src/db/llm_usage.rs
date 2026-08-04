use rusqlite::{params, Connection, Result};
use serde::Serialize;

pub struct LlmUsageRepository<'a> {
    conn: &'a Connection,
}

impl<'a> LlmUsageRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn insert(&self, entry: &NewLlmUsageEntry) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO llm_usage (history_id, email_id, rule_id, model, prompt_tokens, completion_tokens, total_tokens, duration_ms, estimated_cost_usd, source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                entry.history_id,
                entry.email_id,
                entry.rule_id,
                entry.model,
                entry.prompt_tokens,
                entry.completion_tokens,
                entry.total_tokens,
                entry.duration_ms,
                entry.estimated_cost_usd,
                entry.source,
            ],
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    pub fn rule_roi_metrics(&self) -> Result<Vec<RuleRoiMetrics>> {
        let mut stmt = self.conn.prepare(
            "SELECT
                u.rule_id,
                SUM(CASE WHEN u.created_at >= datetime('now', '-24 hours') THEN 1 ELSE 0 END) AS llm_checks_24h,
                SUM(CASE WHEN u.created_at >= datetime('now', '-24 hours') AND h.status = 'success' THEN 1 ELSE 0 END) AS llm_successes_24h,
                SUM(CASE WHEN u.created_at >= datetime('now', '-24 hours') THEN COALESCE(u.prompt_tokens, 0) ELSE 0 END) AS prompt_tokens_24h,
                SUM(CASE WHEN u.created_at >= datetime('now', '-24 hours') THEN COALESCE(u.completion_tokens, 0) ELSE 0 END) AS completion_tokens_24h,
                SUM(CASE WHEN u.created_at >= datetime('now', '-24 hours') THEN COALESCE(u.total_tokens, 0) ELSE 0 END) AS total_tokens_24h,
                SUM(CASE WHEN u.created_at >= datetime('now', '-24 hours') THEN COALESCE(u.estimated_cost_usd, 0) ELSE 0 END) AS estimated_cost_24h_usd,
                CAST(COALESCE(ROUND(AVG(CASE WHEN u.created_at >= datetime('now', '-24 hours') THEN u.duration_ms END)), 0) AS INTEGER) AS avg_duration_24h_ms,
                COUNT(*) AS llm_checks_7d,
                SUM(CASE WHEN h.status = 'success' THEN 1 ELSE 0 END) AS llm_successes_7d,
                SUM(COALESCE(u.prompt_tokens, 0)) AS prompt_tokens_7d,
                SUM(COALESCE(u.completion_tokens, 0)) AS completion_tokens_7d,
                SUM(COALESCE(u.total_tokens, 0)) AS total_tokens_7d,
                SUM(COALESCE(u.estimated_cost_usd, 0)) AS estimated_cost_7d_usd,
                CAST(COALESCE(ROUND(AVG(u.duration_ms)), 0) AS INTEGER) AS avg_duration_7d_ms
             FROM llm_usage u
             LEFT JOIN history h ON h.id = u.history_id
             WHERE u.source = 'cycle'
               AND u.created_at >= datetime('now', '-7 days')
             GROUP BY u.rule_id",
        )?;

        let entries = stmt
            .query_map([], |row| {
                Ok(RuleRoiMetrics {
                    rule_id: row.get(0)?,
                    llm_checks_24h: row.get(1)?,
                    llm_successes_24h: row.get(2)?,
                    prompt_tokens_24h: row.get(3)?,
                    completion_tokens_24h: row.get(4)?,
                    total_tokens_24h: row.get(5)?,
                    estimated_cost_24h_usd: row.get(6)?,
                    avg_duration_24h_ms: row.get(7)?,
                    llm_checks_7d: row.get(8)?,
                    llm_successes_7d: row.get(9)?,
                    prompt_tokens_7d: row.get(10)?,
                    completion_tokens_7d: row.get(11)?,
                    total_tokens_7d: row.get(12)?,
                    estimated_cost_7d_usd: row.get(13)?,
                    avg_duration_7d_ms: row.get(14)?,
                })
            })?
            .collect::<Result<Vec<_>>>()?;

        Ok(entries)
    }
}

#[derive(Debug, Clone)]
pub struct NewLlmUsageEntry {
    pub history_id: Option<i64>,
    pub email_id: String,
    pub rule_id: i64,
    pub model: Option<String>,
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub duration_ms: Option<i64>,
    pub estimated_cost_usd: Option<f64>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuleRoiMetrics {
    pub rule_id: i64,
    pub llm_checks_24h: i64,
    pub llm_successes_24h: i64,
    pub prompt_tokens_24h: i64,
    pub completion_tokens_24h: i64,
    pub total_tokens_24h: i64,
    pub estimated_cost_24h_usd: f64,
    pub avg_duration_24h_ms: i64,
    pub llm_checks_7d: i64,
    pub llm_successes_7d: i64,
    pub prompt_tokens_7d: i64,
    pub completion_tokens_7d: i64,
    pub total_tokens_7d: i64,
    pub estimated_cost_7d_usd: f64,
    pub avg_duration_7d_ms: i64,
}
