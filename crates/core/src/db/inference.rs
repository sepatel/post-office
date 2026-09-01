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
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT OR IGNORE INTO inference_jobs (account_email, email_id, rule_id, source)
              VALUES (?1, ?2, ?3, ?4)",
            params![account_email, email_id, rule_id, source],
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

        let id = self
            .conn
            .query_row(
                "SELECT id FROM inference_jobs
                  WHERE account_email = ?1
                    AND status IN ('pending', 'retrying')
                    AND next_attempt_at <= datetime('now')
                  ORDER BY next_attempt_at ASC, id ASC LIMIT 1",
                params![account_email],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;

        let Some(id) = id else {
            return Ok(None);
        };

        self.conn.execute(
            "UPDATE inference_jobs
             SET status = 'running', lease_until = datetime('now', '+5 minutes'),
                 updated_at = datetime('now')
              WHERE id = ?1 AND account_email = ?2",
            params![id, account_email],
        )?;
        self.get(account_email, id)
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
            "SELECT a.id, a.job_id, a.provider_id, a.model, a.status, a.error, a.created_at
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
                    created_at: row.get(6)?,
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
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO inference_attempts (job_id, provider_id, model, status, error)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![job_id, provider_id, model, status, error],
        )?;
        Ok(())
    }

    pub fn mark_success(&self, account_email: &str, job_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE inference_jobs
             SET status = 'succeeded', lease_until = NULL, last_error = NULL,
                 updated_at = datetime('now')
              WHERE id = ?1 AND account_email = ?2",
            params![job_id, account_email],
        )?;
        Ok(())
    }

    pub fn mark_skipped(&self, account_email: &str, job_id: i64, reason: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE inference_jobs
             SET status = 'skipped', lease_until = NULL, last_error = ?2,
                 updated_at = datetime('now')
              WHERE id = ?1 AND account_email = ?3",
            params![job_id, reason, account_email],
        )?;
        Ok(())
    }

    pub fn mark_retry(
        &self,
        account_email: &str,
        job_id: i64,
        error: &str,
        delay_seconds: u64,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE inference_jobs
             SET status = 'retrying', attempt_count = attempt_count + 1,
                 next_attempt_at = datetime('now', '+' || ?2 || ' seconds'),
                 lease_until = NULL, last_error = ?3, updated_at = datetime('now')
              WHERE id = ?1 AND account_email = ?4",
            params![job_id, delay_seconds as i64, error, account_email],
        )?;
        Ok(())
    }

    pub fn mark_dead_letter(&self, account_email: &str, job_id: i64, error: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE inference_jobs
             SET status = 'dead_letter', lease_until = NULL, last_error = ?2,
                 updated_at = datetime('now')
              WHERE id = ?1 AND account_email = ?3",
            params![job_id, error, account_email],
        )?;
        Ok(())
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
    pub created_at: String,
}
