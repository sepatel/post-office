use rusqlite::{params, Connection, OptionalExtension, Result};
use serde::Serialize;

pub struct AccountRepository<'a> {
    conn: &'a Connection,
}

impl<'a> AccountRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn list(&self) -> Result<Vec<Account>> {
        let mut statement = self.conn.prepare(
            "SELECT email, sort_order, paused, last_successful, last_error
             FROM accounts
             ORDER BY sort_order ASC, email ASC",
        )?;
        let accounts = statement.query_map([], map_account)?.collect();
        accounts
    }

    pub fn get(&self, email: &str) -> Result<Option<Account>> {
        self.conn
            .query_row(
                "SELECT email, sort_order, paused, last_successful, last_error
                 FROM accounts WHERE email = ?1",
                params![email],
                map_account,
            )
            .optional()
    }

    pub fn add(&self, email: &str) -> Result<Account> {
        let next_order = self.conn.query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM accounts",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        if self.has_column("display_name")? {
            self.conn.execute(
                "INSERT INTO accounts (email, display_name, sort_order, last_successful, last_error, updated_at)
                 VALUES (?1, ?2, ?3, datetime('now'), NULL, datetime('now'))
                 ON CONFLICT(email) DO UPDATE SET last_error = NULL, updated_at = datetime('now')",
                params![email, email, next_order],
            )?;
        } else {
            self.conn.execute(
                "INSERT INTO accounts (email, sort_order, last_successful, last_error, updated_at)
                 VALUES (?1, ?2, datetime('now'), NULL, datetime('now'))
                 ON CONFLICT(email) DO UPDATE SET last_error = NULL, updated_at = datetime('now')",
                params![email, next_order],
            )?;
        }
        self.get(email)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
    }

    pub fn set_paused(&self, email: &str, paused: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE accounts SET paused = ?2, updated_at = datetime('now') WHERE email = ?1",
            params![email, paused as i32],
        )?;
        Ok(())
    }

    pub fn reorder(&self, emails: &[String]) -> Result<()> {
        let current = self.list()?;
        if current.len() != emails.len()
            || current
                .iter()
                .any(|account| !emails.contains(&account.email))
        {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let transaction = self.conn.unchecked_transaction()?;
        for (position, email) in emails.iter().enumerate() {
            transaction.execute(
                "UPDATE accounts SET sort_order = ?2, updated_at = datetime('now') WHERE email = ?1",
                params![email, position as i64],
            )?;
        }
        transaction.commit()
    }

    pub fn record_success(&self, email: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE accounts SET last_successful = datetime('now'), last_error = NULL,
             updated_at = datetime('now') WHERE email = ?1",
            params![email],
        )?;
        Ok(())
    }

    pub fn record_error(&self, email: &str, error: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE accounts SET last_error = ?2, updated_at = datetime('now')
             WHERE email = ?1 AND last_error IS NOT ?2",
            params![email, error],
        )?;
        Ok(())
    }

    pub fn delete_with_data(&self, email: &str) -> Result<()> {
        let transaction = self.conn.unchecked_transaction()?;
        transaction.execute(
            "DELETE FROM inference_jobs WHERE account_email = ?1",
            params![email],
        )?;
        transaction.execute(
            "DELETE FROM llm_usage WHERE account_email = ?1",
            params![email],
        )?;
        transaction.execute(
            "DELETE FROM history WHERE account_email = ?1",
            params![email],
        )?;
        transaction.execute(
            "DELETE FROM history_email_meta WHERE account_email = ?1",
            params![email],
        )?;
        transaction.execute("DELETE FROM rules WHERE account_email = ?1", params![email])?;
        transaction.execute(
            "DELETE FROM gmail_sync_state WHERE account_email = ?1",
            params![email],
        )?;
        transaction.execute(
            "DELETE FROM gmail_sync_cursor_log WHERE account_email = ?1",
            params![email],
        )?;
        transaction.execute(
            "DELETE FROM gmail_pending_events WHERE account_email = ?1",
            params![email],
        )?;
        transaction.execute(
            "DELETE FROM gmail_labels WHERE account_email = ?1",
            params![email],
        )?;
        transaction.execute(
            "DELETE FROM workflow_events WHERE account_email = ?1",
            params![email],
        )?;
        transaction.execute(
            "DELETE FROM workflow_runs WHERE account_email = ?1",
            params![email],
        )?;
        transaction.execute(
            "DELETE FROM workflow_messages WHERE account_email = ?1",
            params![email],
        )?;
        transaction.execute(
            "DELETE FROM workflow_rule_sets WHERE account_email = ?1",
            params![email],
        )?;
        transaction.execute(
            "DELETE FROM workflow_mailboxes WHERE account_email = ?1",
            params![email],
        )?;
        transaction.execute("DELETE FROM accounts WHERE email = ?1", params![email])?;
        transaction.commit()
    }

    fn has_column(&self, column: &str) -> Result<bool> {
        let mut statement = self.conn.prepare("PRAGMA table_info(accounts)")?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>>>()?;
        Ok(columns.iter().any(|name| name == column))
    }
}

fn map_account(row: &rusqlite::Row<'_>) -> rusqlite::Result<Account> {
    let paused = row.get::<_, i32>(2)? != 0;
    let last_error: Option<String> = row.get(4)?;
    Ok(Account {
        email: row.get(0)?,
        sort_order: row.get(1)?,
        paused,
        status: if paused {
            "paused".into()
        } else if last_error.is_some() {
            "error".into()
        } else {
            "healthy".into()
        },
        last_successful: row.get(3)?,
        last_error,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct Account {
    pub email: String,
    pub sort_order: i64,
    pub paused: bool,
    pub status: String,
    pub last_successful: Option<String>,
    pub last_error: Option<String>,
}
