use rusqlite::{params, Connection, OptionalExtension, Result};
use serde::Serialize;

pub struct LlmEndpointStatusRepository<'a> {
    conn: &'a Connection,
}

impl<'a> LlmEndpointStatusRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn get(&self, endpoint: &str) -> Result<Option<LlmEndpointStatus>> {
        self.conn
            .query_row(
                "SELECT endpoint, unavailable_until, last_error, updated_at
                 FROM llm_endpoint_status WHERE endpoint = ?1",
                [endpoint],
                map_status,
            )
            .optional()
    }

    pub fn list(&self) -> Result<Vec<LlmEndpointStatus>> {
        let mut statement = self.conn.prepare(
            "SELECT endpoint, unavailable_until, last_error, updated_at
             FROM llm_endpoint_status
             WHERE unavailable_until IS NOT NULL
             ORDER BY unavailable_until ASC",
        )?;
        let rows = statement.query_map([], map_status)?;
        let statuses = rows.collect::<Result<Vec<_>>>()?;
        Ok(statuses
            .into_iter()
            .filter(|status| {
                status
                    .unavailable_until
                    .as_deref()
                    .and_then(crate::db::parse_stored_utc)
                    .is_some_and(|until| until > chrono::Utc::now())
            })
            .collect())
    }

    pub fn mark_unavailable(&self, endpoint: &str, until: &str, error: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO llm_endpoint_status (endpoint, unavailable_until, last_error, updated_at)
             VALUES (?1, ?2, ?3, datetime('now'))
             ON CONFLICT(endpoint) DO UPDATE SET
                 unavailable_until = excluded.unavailable_until,
                 last_error = excluded.last_error,
                 updated_at = excluded.updated_at",
            params![endpoint, until, error],
        )?;
        Ok(())
    }

    pub fn clear(&self, endpoint: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE llm_endpoint_status
             SET unavailable_until = NULL, last_error = NULL, updated_at = datetime('now')
             WHERE endpoint = ?1",
            [endpoint],
        )?;
        Ok(())
    }
}

fn map_status(row: &rusqlite::Row<'_>) -> rusqlite::Result<LlmEndpointStatus> {
    Ok(LlmEndpointStatus {
        endpoint: row.get(0)?,
        unavailable_until: row.get(1)?,
        last_error: row.get(2)?,
        updated_at: row.get(3)?,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct LlmEndpointStatus {
    pub endpoint: String,
    pub unavailable_until: Option<String>,
    pub last_error: Option<String>,
    pub updated_at: String,
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use chrono::{Duration, Utc};

    use crate::db::Database;

    #[test]
    fn lists_only_open_endpoint_circuits() {
        let db = Database::open(Path::new(":memory:")).unwrap();
        db.migrate().unwrap();
        db.with_llm_endpoint_status(|repo| {
            repo.mark_unavailable(
                "http://shade:4000/v1",
                &(Utc::now() + Duration::minutes(5)).to_rfc3339(),
                "connection refused",
            )?;
            assert_eq!(repo.list()?.len(), 1);
            repo.clear("http://shade:4000/v1")?;
            assert!(repo.list()?.is_empty());
            Ok::<(), rusqlite::Error>(())
        })
        .unwrap();
    }
}
