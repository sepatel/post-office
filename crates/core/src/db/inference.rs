use rusqlite::{params, Connection, OptionalExtension, Result};
use serde::Serialize;

pub struct InferenceRepository<'a> {
    conn: &'a Connection,
}

impl<'a> InferenceRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn enqueue(&self, email_id: &str, rule_id: i64, source: &str) -> Result<i64> {
        self.conn.execute(
            "INSERT OR IGNORE INTO inference_jobs (email_id, rule_id, source)
             VALUES (?1, ?2, ?3)",
            params![email_id, rule_id, source],
        )?;

        self.conn.query_row(
            "SELECT id FROM inference_jobs
             WHERE email_id = ?1 AND rule_id = ?2
               AND status IN ('pending', 'running', 'retrying')
             ORDER BY id DESC LIMIT 1",
            params![email_id, rule_id],
            |row| row.get(0),
        )
    }

    pub fn claim_next(&self) -> Result<Option<InferenceJob>> {
        self.conn.execute(
            "UPDATE inference_jobs
             SET status = 'retrying', lease_until = NULL, updated_at = datetime('now')
             WHERE status = 'running' AND lease_until < datetime('now')",
            [],
        )?;

        let id = self
            .conn
            .query_row(
                "SELECT id FROM inference_jobs
                 WHERE status IN ('pending', 'retrying')
                   AND next_attempt_at <= datetime('now')
                 ORDER BY next_attempt_at ASC, id ASC LIMIT 1",
                [],
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
             WHERE id = ?1",
            params![id],
        )?;
        self.get(id)
    }

    pub fn get(&self, id: i64) -> Result<Option<InferenceJob>> {
        self.conn
            .query_row(
                "SELECT id, email_id, rule_id, source, status, attempt_count,
                        next_attempt_at, lease_until, last_error, created_at, updated_at
                 FROM inference_jobs WHERE id = ?1",
                params![id],
                map_job,
            )
            .optional()
    }

    pub fn list(&self, page: u32, per_page: u32) -> Result<Vec<InferenceJob>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, email_id, rule_id, source, status, attempt_count,
                    next_attempt_at, lease_until, last_error, created_at, updated_at
             FROM inference_jobs
             ORDER BY updated_at DESC, id DESC
             LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt.query_map(params![per_page, page.saturating_mul(per_page)], map_job)?;
        rows.collect()
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

    pub fn mark_success(&self, job_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE inference_jobs
             SET status = 'succeeded', lease_until = NULL, last_error = NULL,
                 updated_at = datetime('now')
             WHERE id = ?1",
            params![job_id],
        )?;
        Ok(())
    }

    pub fn mark_skipped(&self, job_id: i64, reason: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE inference_jobs
             SET status = 'skipped', lease_until = NULL, last_error = ?2,
                 updated_at = datetime('now')
             WHERE id = ?1",
            params![job_id, reason],
        )?;
        Ok(())
    }

    pub fn mark_retry(&self, job_id: i64, error: &str, delay_seconds: u64) -> Result<()> {
        self.conn.execute(
            "UPDATE inference_jobs
             SET status = 'retrying', attempt_count = attempt_count + 1,
                 next_attempt_at = datetime('now', '+' || ?2 || ' seconds'),
                 lease_until = NULL, last_error = ?3, updated_at = datetime('now')
             WHERE id = ?1",
            params![job_id, delay_seconds as i64, error],
        )?;
        Ok(())
    }

    pub fn mark_dead_letter(&self, job_id: i64, error: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE inference_jobs
             SET status = 'dead_letter', lease_until = NULL, last_error = ?2,
                 updated_at = datetime('now')
             WHERE id = ?1",
            params![job_id, error],
        )?;
        Ok(())
    }

    pub fn retry(&self, job_id: i64) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE inference_jobs
             SET status = 'pending', next_attempt_at = datetime('now'), lease_until = NULL,
                 last_error = NULL, updated_at = datetime('now')
             WHERE id = ?1 AND status IN ('retrying', 'dead_letter', 'blocked', 'skipped')",
            params![job_id],
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
