use rusqlite::{params, Connection, OptionalExtension, Result};
use serde::Serialize;

pub struct LlmProviderStatusRepository<'a> {
    conn: &'a Connection,
}

impl<'a> LlmProviderStatusRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn get(&self, provider_id: &str) -> Result<Option<LlmProviderStatus>> {
        self.conn
            .query_row(
                "SELECT provider_id, rate_limited_until, last_error, updated_at
                 FROM llm_provider_status WHERE provider_id = ?1",
                params![provider_id],
                map_status,
            )
            .optional()
    }

    pub fn list(&self) -> Result<Vec<LlmProviderStatus>> {
        let mut stmt = self.conn.prepare(
            "SELECT provider_id, rate_limited_until, last_error, updated_at
             FROM llm_provider_status
             ORDER BY provider_id",
        )?;
        let rows = stmt.query_map([], map_status)?;
        rows.collect()
    }

    pub fn mark_rate_limited(&self, provider_id: &str, until: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO llm_provider_status (provider_id, rate_limited_until, last_error, updated_at)
             VALUES (?1, ?2, 'Rate limit exceeded', datetime('now'))
             ON CONFLICT(provider_id) DO UPDATE SET
                rate_limited_until = excluded.rate_limited_until,
                last_error = excluded.last_error,
                updated_at = datetime('now')",
            params![provider_id, until],
        )?;
        Ok(())
    }

    pub fn clear_rate_limit(&self, provider_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE llm_provider_status
             SET rate_limited_until = NULL, last_error = NULL, updated_at = datetime('now')
             WHERE provider_id = ?1",
            params![provider_id],
        )?;
        Ok(())
    }
}

fn map_status(row: &rusqlite::Row<'_>) -> rusqlite::Result<LlmProviderStatus> {
    Ok(LlmProviderStatus {
        provider_id: row.get(0)?,
        rate_limited_until: row.get(1)?,
        last_error: row.get(2)?,
        updated_at: row.get(3)?,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct LlmProviderStatus {
    pub provider_id: String,
    pub rate_limited_until: Option<String>,
    pub last_error: Option<String>,
    pub updated_at: String,
}
