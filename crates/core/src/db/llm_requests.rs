use rusqlite::{params, Connection, Result};
use serde::Serialize;

pub struct LlmRequestRepository<'a> {
    conn: &'a Connection,
}

impl<'a> LlmRequestRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn insert(&self, account_email: &str, request: &NewLlmRequest) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO llm_requests (
                account_email, rule_id, request_key, source, kind, provider_id, model,
                policy_id, email_count, prompt_tokens, completion_tokens, total_tokens, duration_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                account_email,
                request.rule_id,
                request.request_key,
                request.source,
                request.kind,
                request.provider_id,
                request.model,
                request.policy_id,
                request.email_count,
                request.prompt_tokens,
                request.completion_tokens,
                request.total_tokens,
                request.duration_ms,
            ],
        )?;
        Ok(())
    }

    pub fn rule_metrics(&self, account_email: &str) -> Result<Vec<RuleRequestMetrics>> {
        let mut statement = self.conn.prepare(
            "SELECT
                rule_id,
                SUM(CASE WHEN created_at >= datetime('now', '-24 hours') THEN 1 ELSE 0 END),
                SUM(CASE WHEN created_at >= datetime('now', '-24 hours') THEN email_count ELSE 0 END),
                SUM(CASE WHEN created_at >= datetime('now', '-24 hours') THEN COALESCE(prompt_tokens, 0) ELSE 0 END),
                SUM(CASE WHEN created_at >= datetime('now', '-24 hours') THEN COALESCE(completion_tokens, 0) ELSE 0 END),
                SUM(CASE WHEN created_at >= datetime('now', '-24 hours') THEN COALESCE(total_tokens, 0) ELSE 0 END),
                CAST(COALESCE(ROUND(AVG(CASE WHEN created_at >= datetime('now', '-24 hours') THEN duration_ms END)), 0) AS INTEGER),
                COUNT(*),
                SUM(email_count),
                SUM(COALESCE(prompt_tokens, 0)),
                SUM(COALESCE(completion_tokens, 0)),
                SUM(COALESCE(total_tokens, 0)),
                CAST(COALESCE(ROUND(AVG(duration_ms)), 0) AS INTEGER)
             FROM llm_requests
             WHERE account_email = ?1 AND created_at >= datetime('now', '-7 days')
             GROUP BY rule_id",
        )?;
        let metrics = statement
            .query_map(params![account_email], |row| {
                Ok(RuleRequestMetrics {
                    rule_id: row.get(0)?,
                    requests_24h: row.get(1)?,
                    emails_24h: row.get(2)?,
                    prompt_tokens_24h: row.get(3)?,
                    completion_tokens_24h: row.get(4)?,
                    total_tokens_24h: row.get(5)?,
                    avg_duration_24h_ms: row.get(6)?,
                    requests_7d: row.get(7)?,
                    emails_7d: row.get(8)?,
                    prompt_tokens_7d: row.get(9)?,
                    completion_tokens_7d: row.get(10)?,
                    total_tokens_7d: row.get(11)?,
                    avg_duration_7d_ms: row.get(12)?,
                })
            })?
            .collect();
        metrics
    }
}

#[derive(Debug, Clone)]
pub struct NewLlmRequest {
    pub rule_id: i64,
    pub request_key: String,
    pub source: String,
    pub kind: String,
    pub provider_id: Option<String>,
    pub model: String,
    pub policy_id: Option<String>,
    pub email_count: i64,
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub duration_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuleRequestMetrics {
    pub rule_id: i64,
    pub requests_24h: i64,
    pub emails_24h: i64,
    pub prompt_tokens_24h: i64,
    pub completion_tokens_24h: i64,
    pub total_tokens_24h: i64,
    pub avg_duration_24h_ms: i64,
    pub requests_7d: i64,
    pub emails_7d: i64,
    pub prompt_tokens_7d: i64,
    pub completion_tokens_7d: i64,
    pub total_tokens_7d: i64,
    pub avg_duration_7d_ms: i64,
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::db::rules::CreateRuleRequest;
    use crate::db::Database;
    use crate::rules::models::Action;

    #[test]
    fn deduplicates_a_batched_request_and_keeps_its_email_count() {
        let db = Database::open(Path::new(":memory:")).unwrap();
        db.migrate().unwrap();
        let rule = db
            .with_rules(|repo| {
                repo.create(
                    "test@example.com",
                    &CreateRuleRequest {
                        name: "Receipts".into(),
                        description: None,
                        conditions: vec![],
                        prompt: "Classify receipts".into(),
                        choices: vec![],
                        choose_from_all_labels: false,
                        actions: vec![Action::Archive],
                        priority: 0,
                        enabled: true,
                        inference_policy: "default".into(),
                        continue_after_match: false,
                    },
                )
            })
            .unwrap();
        let request = NewLlmRequest {
            rule_id: rule.id,
            request_key: "local:request-1".into(),
            source: "cycle".into(),
            kind: "decision".into(),
            provider_id: Some("local".into()),
            model: "test".into(),
            policy_id: Some("default".into()),
            email_count: 3,
            prompt_tokens: Some(120),
            completion_tokens: Some(30),
            total_tokens: Some(150),
            duration_ms: 500,
        };

        db.with_llm_requests(|repo| {
            repo.insert("test@example.com", &request)?;
            repo.insert("test@example.com", &request)
        })
        .unwrap();

        let metrics = db
            .with_llm_requests(|repo| repo.rule_metrics("test@example.com"))
            .unwrap();
        assert_eq!(metrics[0].requests_24h, 1);
        assert_eq!(metrics[0].emails_24h, 3);
        assert_eq!(metrics[0].total_tokens_24h, 150);
    }
}
