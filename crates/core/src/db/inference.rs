use rusqlite::{params, Connection, OptionalExtension, Result};
use serde::Serialize;

pub struct InferenceRepository<'a> {
    conn: &'a Connection,
}

impl<'a> InferenceRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn enqueue(
        &self,
        account_email: &str,
        email_id: &str,
        rule_id: i64,
        source: &str,
        error: Option<&str>,
        delay_seconds: u64,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT OR IGNORE INTO inference_jobs (
                account_email, email_id, rule_id, source, status, attempt_count,
                next_attempt_at, last_error
             ) VALUES (
                ?1, ?2, ?3, ?4, 'retrying', 1,
                datetime('now', '+' || ?5 || ' seconds'), ?6
             )",
            params![
                account_email,
                email_id,
                rule_id,
                source,
                delay_seconds as i64,
                error,
            ],
        )?;

        self.conn.query_row(
            "SELECT id FROM inference_jobs
              WHERE account_email = ?1 AND email_id = ?2 AND rule_id = ?3
                AND status IN ('pending', 'running', 'retrying')
              ORDER BY id DESC LIMIT 1",
            params![account_email, email_id, rule_id],
            |row| row.get(0),
        )
    }

    pub fn claim_next(&self, account_email: &str) -> Result<Option<InferenceJob>> {
        self.conn.execute(
            "UPDATE inference_jobs
             SET status = 'retrying', lease_until = NULL, updated_at = datetime('now')
              WHERE account_email = ?1 AND status = 'running' AND lease_until < datetime('now')",
            params![account_email],
        )?;

        self.conn
            .query_row(
                "UPDATE inference_jobs
                  SET status = 'running', lease_until = datetime('now', '+15 minutes'),
                      updated_at = datetime('now')
                WHERE id = (
                    SELECT id FROM inference_jobs
                    WHERE account_email = ?1
                      AND status IN ('pending', 'retrying')
                      AND next_attempt_at <= datetime('now')
                    ORDER BY next_attempt_at ASC, id ASC LIMIT 1
                )
                  AND account_email = ?1
                  AND status IN ('pending', 'retrying')
                  AND next_attempt_at <= datetime('now')
                RETURNING id, email_id, rule_id, source, status, attempt_count,
                          next_attempt_at, lease_until, last_error, created_at, updated_at",
                params![account_email],
                map_job,
            )
            .optional()
    }

    pub fn get(&self, account_email: &str, id: i64) -> Result<Option<InferenceJob>> {
        self.conn
            .query_row(
                "SELECT id, email_id, rule_id, source, status, attempt_count,
                        next_attempt_at, lease_until, last_error, created_at, updated_at
                  FROM inference_jobs WHERE id = ?1 AND account_email = ?2",
                params![id, account_email],
                map_job,
            )
            .optional()
    }

    pub fn has_active(&self, account_email: &str, email_id: &str, rule_id: i64) -> Result<bool> {
        self.conn
            .query_row(
                "SELECT 1 FROM inference_jobs
                 WHERE account_email = ?1 AND email_id = ?2 AND rule_id = ?3
                   AND status IN ('pending', 'running', 'retrying')",
                params![account_email, email_id, rule_id],
                |_| Ok(()),
            )
            .optional()
            .map(|result| result.is_some())
    }

    pub fn list(&self, account_email: &str, page: u32, per_page: u32) -> Result<Vec<InferenceJob>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, email_id, rule_id, source, status, attempt_count,
                    next_attempt_at, lease_until, last_error, created_at, updated_at
              FROM inference_jobs WHERE account_email = ?1
              ORDER BY updated_at DESC, id DESC
              LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt.query_map(
            params![account_email, per_page, page.saturating_mul(per_page)],
            map_job,
        )?;
        rows.collect()
    }

    pub fn list_for_rule(
        &self,
        account_email: &str,
        rule_id: i64,
        page: u32,
        per_page: u32,
    ) -> Result<Vec<InferenceJob>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, email_id, rule_id, source, status, attempt_count,
                    next_attempt_at, lease_until, last_error, created_at, updated_at
               FROM inference_jobs
              WHERE account_email = ?1 AND rule_id = ?2 AND status <> 'succeeded'
              ORDER BY updated_at DESC, id DESC
              LIMIT ?3 OFFSET ?4",
        )?;
        let jobs = stmt
            .query_map(
                params![
                    account_email,
                    rule_id,
                    per_page,
                    page.saturating_mul(per_page)
                ],
                map_job,
            )?
            .collect();
        jobs
    }

    pub fn attempts(&self, account_email: &str, job_id: i64) -> Result<Vec<InferenceAttempt>> {
        let mut stmt = self.conn.prepare(
            "SELECT a.id, a.job_id, a.provider_id, a.model, a.status, a.error, a.duration_ms, a.created_at
               FROM inference_attempts a
               JOIN inference_jobs j ON j.id = a.job_id
              WHERE j.account_email = ?1 AND a.job_id = ?2
              ORDER BY a.created_at DESC, a.id DESC",
        )?;
        let attempts = stmt
            .query_map(params![account_email, job_id], |row| {
                Ok(InferenceAttempt {
                    id: row.get(0)?,
                    job_id: row.get(1)?,
                    provider_id: row.get(2)?,
                    model: row.get(3)?,
                    status: row.get(4)?,
                    error: row.get(5)?,
                    duration_ms: row.get(6)?,
                    created_at: row.get(7)?,
                })
            })?
            .collect();
        attempts
    }

    pub fn record_attempt(
        &self,
        job_id: i64,
        provider_id: Option<&str>,
        model: Option<&str>,
        status: &str,
        error: Option<&str>,
        duration_ms: Option<i64>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO inference_attempts (job_id, provider_id, model, status, error, duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![job_id, provider_id, model, status, error, duration_ms],
        )?;
        Ok(())
    }

    pub fn mark_success(
        &self,
        account_email: &str,
        job_id: i64,
        lease_until: Option<&str>,
    ) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE inference_jobs
              SET status = 'succeeded', lease_until = NULL, last_error = NULL,
                  updated_at = datetime('now')
               WHERE id = ?1 AND account_email = ?2 AND status = 'running'
                 AND lease_until = ?3",
            params![job_id, account_email, lease_until],
        )?;
        Ok(changed > 0)
    }

    pub fn mark_skipped(
        &self,
        account_email: &str,
        job_id: i64,
        reason: &str,
        lease_until: Option<&str>,
    ) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE inference_jobs
              SET status = 'skipped', lease_until = NULL, last_error = ?2,
                  updated_at = datetime('now')
               WHERE id = ?1 AND account_email = ?3 AND status = 'running'
                 AND lease_until = ?4",
            params![job_id, reason, account_email, lease_until],
        )?;
        Ok(changed > 0)
    }

    pub fn mark_retry(
        &self,
        account_email: &str,
        job_id: i64,
        error: &str,
        delay_seconds: u64,
        lease_until: Option<&str>,
    ) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE inference_jobs
              SET status = 'retrying', attempt_count = attempt_count + 1,
                  next_attempt_at = datetime('now', '+' || ?2 || ' seconds'),
                  lease_until = NULL, last_error = ?3, updated_at = datetime('now')
               WHERE id = ?1 AND account_email = ?4 AND status = 'running'
                 AND lease_until = ?5",
            params![
                job_id,
                delay_seconds as i64,
                error,
                account_email,
                lease_until
            ],
        )?;
        Ok(changed > 0)
    }

    pub fn mark_dead_letter(
        &self,
        account_email: &str,
        job_id: i64,
        error: &str,
        lease_until: Option<&str>,
    ) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE inference_jobs
              SET status = 'dead_letter', lease_until = NULL, last_error = ?2,
                  updated_at = datetime('now')
               WHERE id = ?1 AND account_email = ?3 AND status = 'running'
                 AND lease_until = ?4",
            params![job_id, error, account_email, lease_until],
        )?;
        Ok(changed > 0)
    }

    pub fn retry(&self, account_email: &str, job_id: i64) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE inference_jobs
             SET status = 'pending', next_attempt_at = datetime('now'), lease_until = NULL,
                  attempt_count = 0, last_error = NULL, updated_at = datetime('now')
              WHERE id = ?1 AND account_email = ?2
                AND status IN ('retrying', 'dead_letter', 'blocked', 'skipped')",
            params![job_id, account_email],
        )?;
        Ok(changed > 0)
    }
}

fn map_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<InferenceJob> {
    Ok(InferenceJob {
        id: row.get(0)?,
        email_id: row.get(1)?,
        rule_id: row.get(2)?,
        source: row.get(3)?,
        status: row.get(4)?,
        attempt_count: row.get(5)?,
        next_attempt_at: row.get(6)?,
        lease_until: row.get(7)?,
        last_error: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct InferenceJob {
    pub id: i64,
    pub email_id: String,
    pub rule_id: Option<i64>,
    pub source: String,
    pub status: String,
    pub attempt_count: i64,
    pub next_attempt_at: String,
    pub lease_until: Option<String>,
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct InferenceAttempt {
    pub id: i64,
    pub job_id: i64,
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub status: String,
    pub error: Option<String>,
    pub duration_ms: Option<i64>,
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::db::Database;

    #[test]
    fn records_attempt_duration() {
        let db = Database::open(Path::new(":memory:")).unwrap();
        db.migrate().unwrap();
        let job_id = {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO inference_jobs (account_email, email_id) VALUES (?1, ?2)",
                params!["test@example.com", "message-1"],
            )
            .unwrap();
            conn.last_insert_rowid()
        };

        db.with_inference(|repo| {
            repo.record_attempt(
                job_id,
                Some("provider"),
                Some("model"),
                "error",
                Some("timed out"),
                Some(900_000),
            )
        })
        .unwrap();

        let attempts = db
            .with_inference(|repo| repo.attempts("test@example.com", job_id))
            .unwrap();

        assert_eq!(attempts[0].duration_ms, Some(900_000));
    }

    #[test]
    fn queues_the_first_retry_with_a_delay() {
        let db = Database::open(Path::new(":memory:")).unwrap();
        db.migrate().unwrap();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO rules (account_email, name, conditions, prompt, actions)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params!["test@example.com", "rule", "[]", "prompt", "[]"],
            )
            .unwrap();
        }

        let job_id = db
            .with_inference(|repo| {
                repo.enqueue(
                    "test@example.com",
                    "message-1",
                    1,
                    "cycle",
                    Some("transport failed"),
                    60,
                )
            })
            .unwrap();
        let job = db
            .with_inference(|repo| repo.get("test@example.com", job_id))
            .unwrap()
            .unwrap();

        assert_eq!(job.status, "retrying");
        assert_eq!(job.attempt_count, 1);
        assert_eq!(job.last_error.as_deref(), Some("transport failed"));
    }

    #[test]
    fn completion_requires_the_claimed_lease() {
        let db = Database::open(Path::new(":memory:")).unwrap();
        db.migrate().unwrap();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO inference_jobs (account_email, email_id) VALUES (?1, ?2)",
                params!["test@example.com", "message-1"],
            )
            .unwrap();
        }

        let job = db
            .with_inference(|repo| repo.claim_next("test@example.com"))
            .unwrap()
            .unwrap();

        assert_eq!(job.status, "running");
        assert!(job.lease_until.is_some());
        assert!(!db
            .with_inference(|repo| repo.mark_success("test@example.com", job.id, Some("wrong")))
            .unwrap());
        assert!(db
            .with_inference(|repo| {
                repo.mark_success("test@example.com", job.id, job.lease_until.as_deref())
            })
            .unwrap());
    }

    #[test]
    fn recognizes_active_recovery_work() {
        let db = Database::open(Path::new(":memory:")).unwrap();
        db.migrate().unwrap();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO rules (account_email, name, conditions, prompt, actions)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params!["test@example.com", "rule", "[]", "prompt", "[]"],
            )
            .unwrap();
        }
        db.with_inference(|repo| {
            repo.enqueue(
                "test@example.com",
                "message-1",
                1,
                "cycle",
                Some("transport failed"),
                60,
            )
        })
        .unwrap();

        assert!(db
            .with_inference(|repo| repo.has_active("test@example.com", "message-1", 1))
            .unwrap());
        assert!(!db
            .with_inference(|repo| repo.has_active("test@example.com", "message-2", 1))
            .unwrap());
    }
}
