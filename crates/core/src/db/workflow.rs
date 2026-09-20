use rusqlite::{params, Connection, OptionalExtension, Result};
use serde::Serialize;

const MAX_ATTEMPTS: i64 = 3;

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
                 state = CASE WHEN manual_retry = 1 OR attempt_count + 1 >= ?2 THEN 'needs_attention' ELSE 'retry_wait' END,
                 lease_token = NULL, lease_until = NULL, next_attempt_at = datetime('now'),
                 manual_retry = 0, last_error = 'Worker lease expired before completion', updated_at = datetime('now')
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
                     ORDER BY manual_retry DESC, next_attempt_at ASC, id ASC
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
                        m.labels_json, r.rule_set_id, s.rules_json, r.next_rule_index, r.manual_retry,
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
        provider_name: Option<&str>,
        model: Option<&str>,
        status: &str,
        error: Option<&str>,
        duration_ms: Option<i64>,
        endpoint: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO workflow_llm_attempts (
                 run_id, rule_index, provider_id, provider_name, model, status, error, duration_ms, endpoint
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                run_id,
                rule_index,
                provider_id,
                provider_name,
                model,
                status,
                error,
                duration_ms,
                endpoint
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
        let tx = self.conn.unchecked_transaction()?;
        let changed = tx.execute(
            "UPDATE workflow_runs
             SET attempt_count = CASE WHEN manual_retry = 1 THEN attempt_count ELSE attempt_count + 1 END,
                 state = CASE WHEN manual_retry = 1 OR attempt_count + 1 >= ?4 THEN 'needs_attention' ELSE 'retry_wait' END,
                 next_attempt_at = CASE
                     WHEN manual_retry = 1 OR attempt_count + 1 >= ?4 THEN next_attempt_at
                     ELSE datetime('now', '+' || ?3 || ' seconds')
                 END,
                 lease_token = NULL, lease_until = NULL, manual_retry = 0, last_error = ?5, updated_at = datetime('now')
             WHERE id = ?1 AND state = 'processing' AND lease_token = ?2",
            params![
                run_id,
                lease_token,
                delay_seconds as i64,
                MAX_ATTEMPTS,
                error
            ],
        )?;
        if changed > 0 {
            let state = tx.query_row(
                "SELECT state FROM workflow_runs WHERE id = ?1",
                [run_id],
                |row| row.get::<_, String>(0),
            )?;
            let kind = if state == "needs_attention" {
                "needs_attention"
            } else {
                "retry_scheduled"
            };
            tx.execute(
                "INSERT INTO workflow_events (account_email, message_id, run_id, kind, detail)
                 SELECT account_email, message_id, id, ?2, ?3
                 FROM workflow_runs WHERE id = ?1",
                params![run_id, kind, error],
            )?;
        }
        tx.commit()?;
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
        let manual_retry = tx.query_row(
            "SELECT manual_retry != 0 FROM workflow_runs WHERE id = ?1",
            [run_id],
            |row| row.get::<_, bool>(0),
        )?;
        tx.execute(
            "UPDATE workflow_runs
             SET state = CASE WHEN ?5 OR ?6 THEN 'needs_attention' ELSE 'retry_wait' END,
                 next_attempt_at = CASE
                     WHEN ?5 OR ?6 THEN next_attempt_at
                     ELSE datetime('now', '+' || ?3 || ' seconds')
                 END,
                 lease_token = NULL, lease_until = NULL, manual_retry = 0, last_error = ?4, updated_at = datetime('now')
             WHERE id = ?1 AND lease_token = ?2",
            params![run_id, lease_token, delay_seconds as i64, error, exhausted, manual_retry],
        )?;
        let state = tx.query_row(
            "SELECT state FROM workflow_runs WHERE id = ?1",
            [run_id],
            |row| row.get::<_, String>(0),
        )?;
        let kind = if state == "needs_attention" {
            "needs_attention"
        } else {
            "action_retry_scheduled"
        };
        tx.execute(
            "INSERT INTO workflow_events (account_email, message_id, run_id, kind, detail)
             SELECT account_email, message_id, id, ?2, ?3
             FROM workflow_runs WHERE id = ?1",
            params![run_id, kind, error],
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
                     lease_until = NULL, last_error = NULL, updated_at = datetime('now')
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
                 lease_until = NULL, last_error = NULL, updated_at = datetime('now')
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
        let tx = self.conn.unchecked_transaction()?;
        let changed = tx.execute(
            "UPDATE workflow_runs
             SET state = 'needs_attention', lease_token = NULL, lease_until = NULL,
                 last_error = ?3, updated_at = datetime('now')
             WHERE id = ?1 AND state = 'processing' AND lease_token = ?2",
            params![run_id, lease_token, error],
        )?;
        if changed > 0 {
            tx.execute(
                "INSERT INTO workflow_events (account_email, message_id, run_id, kind, detail)
                 SELECT account_email, message_id, id, 'needs_attention', ?2
                 FROM workflow_runs WHERE id = ?1",
                params![run_id, error],
            )?;
        }
        tx.commit()?;
        Ok(changed > 0)
    }

    pub fn mark_endpoint_outage(
        &self,
        run_id: i64,
        lease_token: &str,
        error: &str,
    ) -> Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        let changed = tx.execute(
            "UPDATE workflow_runs
             SET attempt_count = CASE WHEN manual_retry = 1 THEN attempt_count ELSE attempt_count + 1 END,
                 state = 'needs_attention', lease_token = NULL, lease_until = NULL,
                 manual_retry = 0, last_error = ?3, updated_at = datetime('now')
             WHERE id = ?1 AND state = 'processing' AND lease_token = ?2",
            params![run_id, lease_token, error],
        )?;
        if changed > 0 {
            tx.execute(
                "INSERT INTO workflow_events (account_email, message_id, run_id, kind, detail)
                 SELECT account_email, message_id, id, 'endpoint_unavailable', ?2
                 FROM workflow_runs WHERE id = ?1",
                params![run_id, error],
            )?;
        }
        tx.commit()?;
        Ok(changed > 0)
    }

    pub fn resolve_externally(&self, run_id: i64, lease_token: &str, reason: &str) -> Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        let changed = tx.execute(
            "UPDATE workflow_runs
             SET state = 'resolved_externally', completed_at = datetime('now'),
                 lease_token = NULL, lease_until = NULL, last_error = ?3, updated_at = datetime('now')
             WHERE id = ?1 AND state = 'processing' AND lease_token = ?2",
            params![run_id, lease_token, reason],
        )?;
        if changed > 0 {
            tx.execute(
                "INSERT INTO workflow_events (account_email, message_id, run_id, kind, detail)
                 SELECT account_email, message_id, id, 'resolved_externally', ?2
                 FROM workflow_runs WHERE id = ?1",
                params![run_id, reason],
            )?;
        }
        tx.commit()?;
        Ok(changed > 0)
    }

    pub fn retry_now(&self, account_email: &str, run_id: i64) -> Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        let changed = tx.execute(
            "UPDATE workflow_runs
             SET state = 'queued', next_attempt_at = datetime('now'), lease_token = NULL,
                 lease_until = NULL, manual_retry = 1, updated_at = datetime('now')
             WHERE id = ?1 AND account_email = ?2
               AND state IN ('retry_wait', 'needs_attention')",
            params![run_id, account_email],
        )?;
        if changed > 0 {
            tx.execute(
                "INSERT INTO workflow_events (account_email, message_id, run_id, kind, detail)
                 SELECT account_email, message_id, id, 'retry_requested', 'Manual retry requested'
                 FROM workflow_runs WHERE id = ?1",
                [run_id],
            )?;
        }
        tx.commit()?;
        Ok(changed > 0)
    }

    pub fn list_runs(
        &self,
        account_email: &str,
        state: Option<&str>,
        page: u32,
        per_page: u32,
    ) -> Result<Vec<WorkflowRun>> {
        let offset = page.saturating_mul(per_page);
        let query = "SELECT r.id, m.id, m.gmail_message_id, m.gmail_thread_id, m.message_json,
                            m.labels_json, r.state, r.next_rule_index, r.attempt_count,
                            r.next_attempt_at, r.last_error, r.created_at, r.updated_at,
                            r.completed_at, s.version, s.rules_json
                     FROM workflow_runs r
                     JOIN workflow_messages m ON m.id = r.message_id
                     JOIN workflow_rule_sets s ON s.id = r.rule_set_id
                     WHERE r.account_email = ?1";
        let order = " ORDER BY CASE r.state
                         WHEN 'needs_attention' THEN 0
                         WHEN 'processing' THEN 1
                         WHEN 'retry_wait' THEN 2
                         WHEN 'queued' THEN 3
                         ELSE 4
                       END, r.updated_at DESC, r.id DESC";
        let mut statement = if state.is_some() {
            self.conn.prepare(&format!(
                "{query} AND r.state = ?2{order} LIMIT ?3 OFFSET ?4"
            ))?
        } else {
            self.conn
                .prepare(&format!("{query}{order} LIMIT ?2 OFFSET ?3"))?
        };
        let rows = match state {
            Some(state) => statement.query_map(
                params![account_email, state, per_page, offset],
                map_workflow_run,
            )?,
            None => {
                statement.query_map(params![account_email, per_page, offset], map_workflow_run)?
            }
        };
        rows.collect()
    }

    pub fn run_for_message(
        &self,
        account_email: &str,
        message_id: i64,
    ) -> Result<Option<WorkflowRun>> {
        self.conn
            .query_row(
                "SELECT r.id, m.id, m.gmail_message_id, m.gmail_thread_id, m.message_json,
                        m.labels_json, r.state, r.next_rule_index, r.attempt_count,
                        r.next_attempt_at, r.last_error, r.created_at, r.updated_at,
                        r.completed_at, s.version, s.rules_json
                 FROM workflow_runs r
                 JOIN workflow_messages m ON m.id = r.message_id
                 JOIN workflow_rule_sets s ON s.id = r.rule_set_id
                 WHERE r.account_email = ?1 AND m.id = ?2",
                params![account_email, message_id],
                map_workflow_run,
            )
            .optional()
    }

    pub fn state_counts(&self, account_email: &str) -> Result<Vec<WorkflowStateCount>> {
        let mut statement = self.conn.prepare(
            "SELECT state, COUNT(*) FROM workflow_runs
             WHERE account_email = ?1 GROUP BY state",
        )?;
        let rows = statement.query_map([account_email], |row| {
            Ok(WorkflowStateCount {
                state: row.get(0)?,
                count: row.get(1)?,
            })
        })?;
        rows.collect()
    }

    pub fn mailbox(&self, account_email: &str) -> Result<Option<WorkflowMailbox>> {
        self.conn
            .query_row(
                "SELECT account_email, history_cursor, initialized_at, updated_at
                 FROM workflow_mailboxes WHERE account_email = ?1",
                [account_email],
                |row| {
                    Ok(WorkflowMailbox {
                        account_email: row.get(0)?,
                        history_cursor: row.get(1)?,
                        initialized_at: row.get(2)?,
                        updated_at: row.get(3)?,
                    })
                },
            )
            .optional()
    }

    pub fn steps(&self, run_id: i64) -> Result<Vec<WorkflowStep>> {
        let mut statement = self.conn.prepare(
            "SELECT id, rule_index, rule_legacy_id, rule_name, outcome, decision_json, error, created_at
             FROM workflow_steps WHERE run_id = ?1 ORDER BY rule_index ASC, id ASC",
        )?;
        let rows = statement.query_map([run_id], |row| {
            Ok(WorkflowStep {
                id: row.get(0)?,
                rule_index: row.get(1)?,
                rule_legacy_id: row.get(2)?,
                rule_name: row.get(3)?,
                outcome: row.get(4)?,
                decision: row.get(5)?,
                error: row.get(6)?,
                created_at: row.get(7)?,
            })
        })?;
        rows.collect()
    }

    pub fn llm_attempts(&self, run_id: i64) -> Result<Vec<WorkflowLlmAttempt>> {
        let mut statement = self.conn.prepare(
            "SELECT id, rule_index, provider_id, provider_name, model, status, error, duration_ms, endpoint, created_at
             FROM workflow_llm_attempts WHERE run_id = ?1 ORDER BY id ASC",
        )?;
        let rows = statement.query_map([run_id], |row| {
            Ok(WorkflowLlmAttempt {
                id: row.get(0)?,
                rule_index: row.get(1)?,
                provider_id: row.get(2)?,
                provider_name: row.get(3)?,
                model: row.get(4)?,
                status: row.get(5)?,
                error: row.get(6)?,
                duration_ms: row.get(7)?,
                endpoint: row.get(8)?,
                created_at: row.get(9)?,
            })
        })?;
        rows.collect()
    }

    pub fn action_plans(&self, run_id: i64) -> Result<Vec<WorkflowActionPlan>> {
        let mut statement = self.conn.prepare(
            "SELECT id, rule_index, add_label_ids, remove_label_ids, state, attempt_count,
                    last_error, created_at, updated_at, completed_at
             FROM workflow_action_plans WHERE run_id = ?1 ORDER BY rule_index ASC, id ASC",
        )?;
        let rows = statement.query_map([run_id], |row| {
            Ok(WorkflowActionPlan {
                id: row.get(0)?,
                rule_index: row.get(1)?,
                add_label_ids: serde_json::from_str(&row.get::<_, String>(2)?).unwrap_or_default(),
                remove_label_ids: serde_json::from_str(&row.get::<_, String>(3)?)
                    .unwrap_or_default(),
                add_label_names: vec![],
                remove_label_names: vec![],
                state: row.get(4)?,
                attempt_count: row.get(5)?,
                last_error: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
                completed_at: row.get(9)?,
            })
        })?;
        rows.collect()
    }

    pub fn events(&self, run_id: i64) -> Result<Vec<WorkflowEvent>> {
        let mut statement = self.conn.prepare(
            "SELECT id, kind, detail, created_at
             FROM workflow_events WHERE run_id = ?1 ORDER BY id ASC",
        )?;
        let rows = statement.query_map([run_id], |row| {
            Ok(WorkflowEvent {
                id: row.get(0)?,
                kind: row.get(1)?,
                detail: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        rows.collect()
    }

    pub fn rule_set_status(&self, account_email: &str) -> Result<Vec<WorkflowRuleSetStatus>> {
        let mut statement = self.conn.prepare(
            "SELECT s.version, s.active,
                    SUM(CASE WHEN r.state IN ('queued', 'processing', 'retry_wait', 'needs_attention') THEN 1 ELSE 0 END)
             FROM workflow_rule_sets s
             LEFT JOIN workflow_runs r ON r.rule_set_id = s.id
             WHERE s.account_email = ?1
             GROUP BY s.id, s.version, s.active
             ORDER BY s.version DESC",
        )?;
        let rows = statement.query_map([account_email], |row| {
            Ok(WorkflowRuleSetStatus {
                version: row.get(0)?,
                active: row.get::<_, i32>(1)? != 0,
                active_run_count: row.get(2)?,
            })
        })?;
        rows.collect()
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
    pub manual_retry: bool,
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

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowRun {
    pub run_id: i64,
    pub message_id: i64,
    pub gmail_message_id: String,
    pub gmail_thread_id: Option<String>,
    pub message_json: Option<String>,
    pub labels_json: Option<String>,
    pub state: String,
    pub next_rule_index: i64,
    pub attempt_count: i64,
    pub next_attempt_at: String,
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
    pub rule_set_version: i64,
    pub rules_json: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowStateCount {
    pub state: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowMailbox {
    pub account_email: String,
    pub history_cursor: String,
    pub initialized_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowStep {
    pub id: i64,
    pub rule_index: i64,
    pub rule_legacy_id: i64,
    pub rule_name: String,
    pub outcome: String,
    pub decision: Option<String>,
    pub error: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowLlmAttempt {
    pub id: i64,
    pub rule_index: i64,
    pub provider_id: Option<String>,
    pub provider_name: Option<String>,
    pub model: Option<String>,
    pub status: String,
    pub error: Option<String>,
    pub duration_ms: Option<i64>,
    pub endpoint: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowActionPlan {
    pub id: i64,
    pub rule_index: i64,
    pub add_label_ids: Vec<String>,
    pub remove_label_ids: Vec<String>,
    pub add_label_names: Vec<String>,
    pub remove_label_names: Vec<String>,
    pub state: String,
    pub attempt_count: i64,
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowEvent {
    pub id: i64,
    pub kind: String,
    pub detail: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowRuleSetStatus {
    pub version: i64,
    pub active: bool,
    pub active_run_count: i64,
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
        manual_retry: row.get::<_, i32>(9)? != 0,
        lease_token: row.get(10)?,
        lease_until: row.get(11)?,
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

fn map_workflow_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkflowRun> {
    Ok(WorkflowRun {
        run_id: row.get(0)?,
        message_id: row.get(1)?,
        gmail_message_id: row.get(2)?,
        gmail_thread_id: row.get(3)?,
        message_json: row.get(4)?,
        labels_json: row.get(5)?,
        state: row.get(6)?,
        next_rule_index: row.get(7)?,
        attempt_count: row.get(8)?,
        next_attempt_at: row.get(9)?,
        last_error: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
        completed_at: row.get(13)?,
        rule_set_version: row.get(14)?,
        rules_json: row.get(15)?,
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
            assert_eq!(repo.list_runs("test@example.com", None, 0, 20)?.len(), 1);
            assert_eq!(
                repo.list_runs("test@example.com", Some("queued"), 0, 20)?
                    .len(),
                1
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

    #[test]
    fn external_resolution_is_terminal() {
        let db = Database::open(Path::new(":memory:")).unwrap();
        db.migrate().unwrap();
        db.with_workflow(|repo| {
            repo.initialize_mailbox("test@example.com", "100")?;
            repo.activate_rule_set("test@example.com", r#"{"rules":[]}"#)?;
            repo.enqueue_arrivals("test@example.com", "101", &["message-1".into()])?;
            let run = repo.claim_next("test@example.com", "lease-1")?.unwrap();
            assert!(repo.resolve_externally(run.id, "lease-1", "Archived manually")?);
            let stored = repo
                .run_for_message("test@example.com", run.message_id)?
                .unwrap();
            assert_eq!(stored.state, "resolved_externally");
            assert!(!repo.retry_now("test@example.com", run.id)?);
            Ok::<(), rusqlite::Error>(())
        })
        .unwrap();
    }
}
