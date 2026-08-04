use rusqlite::{params, Connection, OptionalExtension, Result};
use serde::Serialize;

pub struct SyncStateRepository<'a> {
    conn: &'a Connection,
}

impl<'a> SyncStateRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn get(&self, account_email: &str) -> Result<Option<GmailSyncState>> {
        self.conn
            .query_row(
                "SELECT account_email, last_history_id, watch_expiration, watch_status,
                        last_notification_at, last_history_pull_at, last_sync_error, updated_at
                 FROM gmail_sync_state
                 WHERE account_email = ?1",
                params![account_email],
                |row| {
                    Ok(GmailSyncState {
                        account_email: row.get(0)?,
                        last_history_id: row.get(1)?,
                        watch_expiration: row.get(2)?,
                        watch_status: row.get(3)?,
                        last_notification_at: row.get(4)?,
                        last_history_pull_at: row.get(5)?,
                        last_sync_error: row.get(6)?,
                        updated_at: row.get(7)?,
                    })
                },
            )
            .optional()
    }

    pub fn upsert_cursor(
        &self,
        account_email: &str,
        last_history_id: &str,
        watch_status: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO gmail_sync_state(account_email, last_history_id, watch_status, updated_at)
             VALUES (?1, ?2, COALESCE(?3, 'inactive'), datetime('now'))
             ON CONFLICT(account_email) DO UPDATE SET
                last_history_id = excluded.last_history_id,
                watch_status = COALESCE(?3, gmail_sync_state.watch_status),
                updated_at = datetime('now')",
            params![account_email, last_history_id, watch_status],
        )?;
        Ok(())
    }

    pub fn update_watch(
        &self,
        account_email: &str,
        expiration: Option<&str>,
        status: &str,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE gmail_sync_state
             SET watch_expiration = ?2,
                 watch_status = ?3,
                 updated_at = datetime('now')
             WHERE account_email = ?1",
            params![account_email, expiration, status],
        )?;
        Ok(())
    }

    pub fn mark_notification_received(&self, account_email: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE gmail_sync_state
             SET last_notification_at = datetime('now'),
                 updated_at = datetime('now')
             WHERE account_email = ?1",
            params![account_email],
        )?;
        Ok(())
    }

    pub fn mark_history_pull(&self, account_email: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE gmail_sync_state
             SET last_history_pull_at = datetime('now'),
                 updated_at = datetime('now')
             WHERE account_email = ?1",
            params![account_email],
        )?;
        Ok(())
    }

    pub fn set_sync_error(&self, account_email: &str, error: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE gmail_sync_state
             SET last_sync_error = ?2,
                 updated_at = datetime('now')
             WHERE account_email = ?1",
            params![account_email, error],
        )?;
        Ok(())
    }

    pub fn insert_cursor_log(
        &self,
        account_email: &str,
        from_history_id: Option<&str>,
        to_history_id: &str,
        trigger: &str,
        processed_messages: usize,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO gmail_sync_cursor_log(account_email, from_history_id, to_history_id, trigger, processed_messages)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                account_email,
                from_history_id,
                to_history_id,
                trigger,
                processed_messages as i64
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn upsert_pending_event(
        &self,
        account_email: &str,
        event_key: &str,
        min_history_id: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO gmail_pending_events(account_email, event_key, min_history_id, status)
             VALUES (?1, ?2, ?3, 'pending')
             ON CONFLICT(event_key) DO UPDATE SET
                min_history_id = CASE
                    WHEN CAST(excluded.min_history_id AS INTEGER) < CAST(gmail_pending_events.min_history_id AS INTEGER)
                        THEN excluded.min_history_id
                    ELSE gmail_pending_events.min_history_id
                END",
            params![account_email, event_key, min_history_id],
        )?;
        Ok(())
    }

    pub fn list_pending_events(
        &self,
        account_email: &str,
        limit: u32,
    ) -> Result<Vec<PendingSyncEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT event_key, min_history_id, received_at, status
             FROM gmail_pending_events
             WHERE account_email = ?1 AND status = 'pending'
             ORDER BY CAST(min_history_id AS INTEGER) ASC, received_at ASC
             LIMIT ?2",
        )?;

        let rows = stmt.query_map(params![account_email, limit], |row| {
            Ok(PendingSyncEvent {
                event_key: row.get(0)?,
                min_history_id: row.get(1)?,
                received_at: row.get(2)?,
                status: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>>>()
    }

    pub fn mark_event_status(&self, event_key: &str, status: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE gmail_pending_events SET status = ?2 WHERE event_key = ?1",
            params![event_key, status],
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct GmailSyncState {
    pub account_email: String,
    pub last_history_id: String,
    pub watch_expiration: Option<String>,
    pub watch_status: String,
    pub last_notification_at: Option<String>,
    pub last_history_pull_at: Option<String>,
    pub last_sync_error: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PendingSyncEvent {
    pub event_key: String,
    pub min_history_id: String,
    pub received_at: String,
    pub status: String,
}
