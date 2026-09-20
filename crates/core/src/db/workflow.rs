use rusqlite::{params, Connection, OptionalExtension, Result};
use serde::Serialize;

const MAX_ATTEMPTS: i64 = 7;

pub struct WorkflowRepository<'a> {
    conn: &'a Connection,
}

impl<'a> WorkflowRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn initialize_mailbox(&self, account_email: &str, history_cursor: &str) -> Result<bool> {
        let inserted = self.conn.execute(
            "INSERT OR IGNORE INTO workflow_mailboxes (account_email, history_cursor)
             VALUES (?1, ?2)",
            params![account_email, history_cursor],
        )?;
        Ok(inserted > 0)
    }

    pub fn reset_mailbox_cursor(&self, account_email: &str, history_cursor: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE workflow_mailboxes
             SET history_cursor = ?2, updated_at = datetime('now')
             WHERE account_email = ?1",
            params![account_email, history_cursor],
        )?;
        Ok(())
    }

    pub fn mailbox_cursor(&self, account_email: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT history_cursor FROM workflow_mailboxes WHERE account_email = ?1",
                [account_email],
                |row| row.get(0),
            )
            .optional()
    }

    pub fn active_rule_set(&self, account_email: &str) -> Result<Option<RuleSet>> {
        self.conn
            .query_row(
                "SELECT id, account_email, version, rules_json, created_at
                 FROM workflow_rule_sets
                 WHERE account_email = ?1 AND active = 1",
                [account_email],
                map_rule_set,
            )
            .optional()
    }

    pub fn activate_rule_set(&self, account_email: &str, rules_json: &str) -> Result<RuleSet> {
        let tx = self.conn.unchecked_transaction()?;
        let version = tx.query_row(
            "SELECT COALESCE(MAX(version), 0) + 1 FROM workflow_rule_sets WHERE account_email = ?1",
            [account_email],
            |row| row.get::<_, i64>(0),
        )?;
        tx.execute(
            "UPDATE workflow_rule_sets SET active = 0 WHERE account_email = ?1 AND active = 1",
            [account_email],
        )?;
        tx.execute(
            "INSERT INTO workflow_rule_sets (account_email, version, rules_json, active)
             VALUES (?1, ?2, ?3, 1)",
            params![account_email, version, rules_json],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        self.conn.query_row(
            "SELECT id, account_email, version, rules_json, created_at
             FROM workflow_rule_sets WHERE id = ?1",
            [id],
            map_rule_set,
        )
    }

    /// Records arrivals before moving the Gmail cursor. Replaying a page is safe:
    /// message identity and the run identity both have unique constraints.
    pub fn enqueue_arrivals(
        &self,
        account_email: &str,
        history_cursor: &str,
        message_ids: &[String],
    ) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        let rule_set_id = tx.query_row(
            "SELECT id FROM workflow_rule_sets WHERE account_email = ?1 AND active = 1",
            [account_email],
            |row| row.get::<_, i64>(0),
        )?;
        let mut created = 0;
        for gmail_message_id in message_ids {
            tx.execute(
                "INSERT OR IGNORE INTO workflow_messages (
                     account_email, gmail_message_id, gmail_history_id
                 ) VALUES (?1, ?2, ?3)",
                params![account_email, gmail_message_id, history_cursor],
            )?;
            let message_id = tx.query_row(
                "SELECT id FROM workflow_messages
                 WHERE account_email = ?1 AND gmail_message_id = ?2",
                params![account_email, gmail_message_id],
                |row| row.get::<_, i64>(0),
            )?;
            let inserted = tx.execute(
                "INSERT OR IGNORE INTO workflow_runs (account_email, message_id, rule_set_id)
                 VALUES (?1, ?2, ?3)",
                params![account_email, message_id, rule_set_id],
            )?;
            if inserted > 0 {
                created += 1;
                let run_id = tx.last_insert_rowid();
                tx.execute(
                    "INSERT INTO workflow_events (account_email, message_id, run_id, kind, detail)
                     VALUES (?1, ?2, ?3, 'queued', ?4)",
                    params![account_email, message_id, run_id, history_cursor],
                )?;
            }
        }
        tx.execute(
            "UPDATE workflow_mailboxes
             SET history_cursor = ?2, updated_at = datetime('now')
             WHERE account_email = ?1",
            params![account_email, history_cursor],
        )?;
        tx.commit()?;
        Ok(created)
    }

    pub fn claim_next(&self, account_email: &str, lease_token: &str) -> Result<Option<ClaimedRun>> {
        self.conn.execute(
            "UPDATE workflow_runs
             SET attempt_count = attempt_count + 1,
                 state = CASE WHEN attempt_count + 1 >= ?2 THEN 'needs_attention' ELSE 'retry_wait' END,
                 lease_token = NULL, lease_until = NULL, next_attempt_at = datetime('now'),
                 last_error = 'Worker lease expired before completion', updated_at = datetime('now')
             WHERE account_email = ?1 AND state = 'processing'
               AND lease_until < datetime('now')",
            params![account_email, MAX_ATTEMPTS],
        )?;
        let id = self
            .conn
            .query_row(
                "UPDATE workflow_runs
                 SET state = 'processing', lease_token = ?2,
                     lease_until = datetime('now', '+15 minutes'), updated_at = datetime('now')
                 WHERE id = (
                     SELECT id FROM workflow_runs
                     WHERE account_email = ?1
                       AND state IN ('queued', 'retry_wait')
                       AND next_attempt_at <= datetime('now')
                     ORDER BY next_attempt_at ASC, id ASC
                     LIMIT 1
                 )
                 RETURNING id",
                params![account_email, lease_token],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        let Some(id) = id else {
            return Ok(None);
        };
        self.conn
            .query_row(
                "SELECT r.id, r.account_email, r.message_id, m.gmail_message_id, m.message_json,
                        m.labels_json, r.rule_set_id, s.rules_json, r.next_rule_index,
                        r.lease_token, r.lease_until
                 FROM workflow_runs r
                 JOIN workflow_messages m ON m.id = r.message_id
                 JOIN workflow_rule_sets s ON s.id = r.rule_set_id
                 WHERE r.id = ?1",
                [id],
                map_claimed_run,
            )
            .map(Some)
    }

    pub fn store_message_snapshot(
        &self,
        run_id: i64,
        lease_token: &str,
        gmail_thread_id: &str,
        message_json: &str,
        labels_json: &str,
    ) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE workflow_messages
             SET gmail_thread_id = ?3, message_json = ?4, labels_json = ?5, updated_at = datetime('now')
             WHERE id = (
                 SELECT message_id FROM workflow_runs
                 WHERE id = ?1 AND state = 'processing' AND lease_token = ?2
             )",
            params![run_id, lease_token, gmail_thread_id, message_json, labels_json],
        )?;
        Ok(changed > 0)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "A step is written atomically with the run cursor it advances."
    )]
    pub fn record_step_and_advance(
        &self,
        run_id: i64,
        lease_token: &str,
        rule_index: i64,
        rule_legacy_id: i64,
        rule_name: &str,
        outcome: &str,
        decision_json: Option<&str>,
        error: Option<&str>,
    ) -> Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        let owned = tx.query_row(
            "SELECT 1 FROM workflow_runs WHERE id = ?1 AND state = 'processing' AND lease_token = ?2",
            params![run_id, lease_token],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
        if !owned {
            return Ok(false);
        }
        tx.execute(
            "INSERT OR IGNORE INTO workflow_steps (
                 run_id, rule_index, rule_legacy_id, rule_name, outcome, decision_json, error
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                run_id,
                rule_index,
                rule_legacy_id,
                rule_name,
                outcome,
                decision_json,
                error
            ],
        )?;
        tx.execute(
            "UPDATE workflow_runs
             SET next_rule_index = ?3, updated_at = datetime('now')
             WHERE id = ?1 AND lease_token = ?2",
            params![run_id, lease_token, rule_index + 1],
        )?;
        tx.commit()?;
        Ok(true)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "The persisted step mirrors the complete decision audit record."
    )]
    pub fn record_step(
        &self,
        run_id: i64,
        lease_token: &str,
        rule_index: i64,
        rule_legacy_id: i64,
        rule_name: &str,
        outcome: &str,
        decision_json: Option<&str>,
        error: Option<&str>,
    ) -> Result<bool> {
        let owned = self
            .conn
            .query_row(
                "SELECT 1 FROM workflow_runs WHERE id = ?1 AND state = 'processing' AND lease_token = ?2",
                params![run_id, lease_token],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !owned {
            return Ok(false);
        }
        self.conn.execute(
            "INSERT OR IGNORE INTO workflow_steps (
                 run_id, rule_index, rule_legacy_id, rule_name, outcome, decision_json, error
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                run_id,
                rule_index,
                rule_legacy_id,
                rule_name,
                outcome,
                decision_json,
                error
            ],
        )?;
        Ok(true)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Attempt telemetry is stored together to preserve the decision audit trail."
    )]
    pub fn record_llm_attempt(
        &self,
        run_id: i64,
        rule_index: i64,
        provider_id: Option<&str>,
        model: Option<&str>,
        status: &str,
        error: Option<&str>,
        duration_ms: Option<i64>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO workflow_llm_attempts (
                 run_id, rule_index, provider_id, model, status, error, duration_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                run_id,
                rule_index,
                provider_id,
                model,
                status,
                error,
                duration_ms
            ],
        )?;
        Ok(())
    }

    pub fn retry_run(
        &self,
        run_id: i64,
        lease_token: &str,
        error: &str,
        delay_seconds: u64,
    ) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE workflow_runs
             SET attempt_count = attempt_count + 1,
                 state = CASE WHEN attempt_count + 1 >= ?4 THEN 'needs_attention' ELSE 'retry_wait' END,
                 next_attempt_at = CASE
                     WHEN attempt_count + 1 >= ?4 THEN next_attempt_at
                     ELSE datetime('now', '+' || ?3 || ' seconds')
                 END,
                 lease_token = NULL, lease_until = NULL, last_error = ?5, updated_at = datetime('now')
             WHERE id = ?1 AND state = 'processing' AND lease_token = ?2",
            params![
                run_id,
                lease_token,
                delay_seconds as i64,
                MAX_ATTEMPTS,
                error
            ],
        )?;
        Ok(changed > 0)
    }

    pub fn plan_action(
        &self,
        run_id: i64,
        lease_token: &str,
        rule_index: i64,
        add_label_ids: &[String],
        remove_label_ids: &[String],
    ) -> Result<Option<ActionPlan>> {
        let tx = self.conn.unchecked_transaction()?;
        let owned = tx.query_row(
            "SELECT 1 FROM workflow_runs WHERE id = ?1 AND state = 'processing' AND lease_token = ?2",
            params![run_id, lease_token],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
        if !owned {
            return Ok(None);
        }
        let add = serde_json::to_string(add_label_ids).unwrap_or_else(|_| "[]".into());
        let remove = serde_json::to_string(remove_label_ids).unwrap_or_else(|_| "[]".into());
        tx.execute(
            "INSERT OR IGNORE INTO workflow_action_plans (
                 run_id, rule_index, add_label_ids, remove_label_ids
             ) VALUES (?1, ?2, ?3, ?4)",
            params![run_id, rule_index, add, remove],
        )?;
        let plan = tx.query_row(
            "SELECT id, run_id, rule_index, add_label_ids, remove_label_ids, state, attempt_count, last_error
             FROM workflow_action_plans WHERE run_id = ?1 AND rule_index = ?2",
            params![run_id, rule_index],
            map_action_plan,
        )?;
        tx.commit()?;
        Ok(Some(plan))
    }

    pub fn action_for_rule(&self, run_id: i64, rule_index: i64) -> Result<Option<ActionPlan>> {
        self.conn
            .query_row(
                "SELECT id, run_id, rule_index, add_label_ids, remove_label_ids, state, attempt_count, last_error
                 FROM workflow_action_plans
                 WHERE run_id = ?1 AND rule_index = ?2 AND state <> 'succeeded'",
                params![run_id, rule_index],
                map_action_plan,
            )
            .optional()
    }

    pub fn retry_action(
        &self,
        action_id: i64,
        run_id: i64,
        lease_token: &str,
        error: &str,
        delay_seconds: u64,
    ) -> Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        let owned = tx.query_row(
            "SELECT 1 FROM workflow_runs WHERE id = ?1 AND state = 'processing' AND lease_token = ?2",
            params![run_id, lease_token],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
        if !owned {
            return Ok(false);
        }
        tx.execute(
            "UPDATE workflow_action_plans
             SET attempt_count = attempt_count + 1, last_error = ?2, updated_at = datetime('now')
             WHERE id = ?1",
            params![action_id, error],
        )?;
        let exhausted = tx.query_row(
            "SELECT attempt_count >= ?2 FROM workflow_action_plans WHERE id = ?1",
            params![action_id, MAX_ATTEMPTS],
            |row| row.get::<_, bool>(0),
        )?;
        tx.execute(
            "UPDATE workflow_runs
             SET state = CASE WHEN ?5 THEN 'needs_attention' ELSE 'retry_wait' END,
                 next_attempt_at = CASE
                     WHEN ?5 THEN next_attempt_at
                     ELSE datetime('now', '+' || ?3 || ' seconds')
                 END,
                 lease_token = NULL, lease_until = NULL, last_error = ?4, updated_at = datetime('now')
             WHERE id = ?1 AND lease_token = ?2",
            params![run_id, lease_token, delay_seconds as i64, error, exhausted],
        )?;
        tx.commit()?;
        Ok(true)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Action confirmation atomically advances both the message state and its run."
    )]
    pub fn confirm_action(
        &self,
        action_id: i64,
        run_id: i64,
        lease_token: &str,
        message_json: &str,
        labels_json: &str,
        next_rule_index: i64,
        continue_chain: bool,
    ) -> Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        let owned = tx
            .query_row(
                "SELECT message_id FROM workflow_runs
             WHERE id = ?1 AND state = 'processing' AND lease_token = ?2",
                params![run_id, lease_token],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        let Some(message_id) = owned else {
            return Ok(false);
        };
        tx.execute(
            "UPDATE workflow_action_plans
             SET state = 'succeeded', last_error = NULL, completed_at = datetime('now'), updated_at = datetime('now')
             WHERE id = ?1",
            [action_id],
        )?;
        tx.execute(
            "UPDATE workflow_messages
             SET message_json = ?2, labels_json = ?3, updated_at = datetime('now')
             WHERE id = ?1",
            params![message_id, message_json, labels_json],
        )?;
        if continue_chain {
            tx.execute(
                "UPDATE workflow_runs
                 SET next_rule_index = ?3, updated_at = datetime('now')
                 WHERE id = ?1 AND lease_token = ?2",
                params![run_id, lease_token, next_rule_index],
            )?;
        } else {
            tx.execute(
                "UPDATE workflow_runs
                 SET state = 'completed', completed_at = datetime('now'), lease_token = NULL,
                     lease_until = NULL, updated_at = datetime('now')
                 WHERE id = ?1 AND lease_token = ?2",
                params![run_id, lease_token],
            )?;
        }
        tx.execute(
            "INSERT INTO workflow_events (account_email, message_id, run_id, kind)
             SELECT account_email, message_id, id, 'action_confirmed'
             FROM workflow_runs WHERE id = ?1",
            [run_id],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub fn complete_run(&self, run_id: i64, lease_token: &str, detail: &str) -> Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        let changed = tx.execute(
            "UPDATE workflow_runs
             SET state = 'completed', completed_at = datetime('now'), lease_token = NULL,
                 lease_until = NULL, updated_at = datetime('now')
             WHERE id = ?1 AND state = 'processing' AND lease_token = ?2",
            params![run_id, lease_token],
        )?;
        if changed > 0 {
            tx.execute(
                "INSERT INTO workflow_events (account_email, message_id, run_id, kind, detail)
                 SELECT account_email, message_id, id, 'completed', ?2
                 FROM workflow_runs WHERE id = ?1",
                params![run_id, detail],
            )?;
        }
        tx.commit()?;
        Ok(changed > 0)
    }

    pub fn mark_needs_attention(
        &self,
        run_id: i64,
        lease_token: &str,
        error: &str,
    ) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE workflow_runs
             SET state = 'needs_attention', lease_token = NULL, lease_until = NULL,
                 last_error = ?3, updated_at = datetime('now')
             WHERE id = ?1 AND state = 'processing' AND lease_token = ?2",
            params![run_id, lease_token, error],
        )?;
        Ok(changed > 0)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RuleSet {
    pub id: i64,
    pub account_email: String,
    pub version: i64,
    pub rules_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct ClaimedRun {
    pub id: i64,
    pub account_email: String,
    pub message_id: i64,
    pub gmail_message_id: String,
    pub message_json: Option<String>,
    pub labels_json: Option<String>,
    pub rule_set_id: i64,
    pub rules_json: String,
    pub next_rule_index: i64,
    pub lease_token: String,
    pub lease_until: String,
}

#[derive(Debug, Clone)]
pub struct ActionPlan {
    pub id: i64,
    pub run_id: i64,
    pub rule_index: i64,
    pub add_label_ids: Vec<String>,
    pub remove_label_ids: Vec<String>,
    pub state: String,
    pub attempt_count: i64,
    pub last_error: Option<String>,
}

fn map_rule_set(row: &rusqlite::Row<'_>) -> rusqlite::Result<RuleSet> {
    Ok(RuleSet {
        id: row.get(0)?,
        account_email: row.get(1)?,
        version: row.get(2)?,
        rules_json: row.get(3)?,
        created_at: row.get(4)?,
    })
}

fn map_claimed_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<ClaimedRun> {
    Ok(ClaimedRun {
        id: row.get(0)?,
        account_email: row.get(1)?,
        message_id: row.get(2)?,
        gmail_message_id: row.get(3)?,
        message_json: row.get(4)?,
        labels_json: row.get(5)?,
        rule_set_id: row.get(6)?,
        rules_json: row.get(7)?,
        next_rule_index: row.get(8)?,
        lease_token: row.get(9)?,
        lease_until: row.get(10)?,
    })
}

fn map_action_plan(row: &rusqlite::Row<'_>) -> rusqlite::Result<ActionPlan> {
    let add_label_ids = serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or_default();
    let remove_label_ids = serde_json::from_str(&row.get::<_, String>(4)?).unwrap_or_default();
    Ok(ActionPlan {
        id: row.get(0)?,
        run_id: row.get(1)?,
        rule_index: row.get(2)?,
        add_label_ids,
        remove_label_ids,
        state: row.get(5)?,
        attempt_count: row.get(6)?,
        last_error: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::db::Database;

    #[test]
    fn repeated_arrival_does_not_create_another_run() {
        let db = Database::open(Path::new(":memory:")).unwrap();
        db.migrate().unwrap();
        db.with_workflow(|repo| {
            repo.initialize_mailbox("test@example.com", "100")?;
            repo.activate_rule_set("test@example.com", r#"{"rules":[]}"#)?;
            assert_eq!(
                repo.enqueue_arrivals("test@example.com", "101", &["message-1".into()])?,
                1
            );
            assert_eq!(
                repo.enqueue_arrivals("test@example.com", "101", &["message-1".into()])?,
                0
            );
            assert_eq!(
                repo.mailbox_cursor("test@example.com")?.as_deref(),
                Some("101")
            );
            let run = repo.claim_next("test@example.com", "lease-1")?.unwrap();
            assert!(repo.complete_run(run.id, "lease-1", "test")?);
            assert!(repo.claim_next("test@example.com", "lease-2")?.is_none());
            Ok::<(), rusqlite::Error>(())
        })
        .unwrap();
    }

    #[test]
    fn action_plan_survives_a_worker_restart() {
        let db = Database::open(Path::new(":memory:")).unwrap();
        db.migrate().unwrap();
        db.with_workflow(|repo| {
            repo.initialize_mailbox("test@example.com", "100")?;
            repo.activate_rule_set("test@example.com", r#"{"rules":[]}"#)?;
            repo.enqueue_arrivals("test@example.com", "101", &["message-1".into()])?;
            let run = repo.claim_next("test@example.com", "lease-1")?.unwrap();
            let plan = repo
                .plan_action(run.id, "lease-1", 0, &["Label_1".into()], &["INBOX".into()])?
                .unwrap();
            assert_eq!(plan.add_label_ids, ["Label_1"]);
            assert_eq!(
                repo.action_for_rule(run.id, 0)?.unwrap().remove_label_ids,
                ["INBOX"]
            );
            Ok::<(), rusqlite::Error>(())
        })
        .unwrap();
    }
}
