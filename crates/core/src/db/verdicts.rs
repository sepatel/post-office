use rusqlite::{params, Connection, OptionalExtension, Result};
use serde::Serialize;

pub struct VerdictRepository<'a> {
    conn: &'a Connection,
}

/// A step the LLM decided that `model` has not yet been asked about, with
/// everything needed to ask it.
#[derive(Debug, Clone)]
pub struct PendingStep {
    pub step_id: i64,
    pub run_id: i64,
    pub rule_index: i64,
    pub rule_legacy_id: i64,
    pub rule_name: String,
    pub outcome: String,
    pub reply: Option<String>,
    pub account_email: String,
    pub rules_json: String,
    pub message_json: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct NewVerdict<'a> {
    pub step_id: i64,
    pub run_id: i64,
    pub account_email: &'a str,
    pub rule_index: i64,
    pub rule_legacy_id: i64,
    pub rule_name: &'a str,
    pub framing: &'a str,
    pub model: &'a str,
    pub backend: Option<&'a str>,
    pub precision: Option<&'a str>,
    pub status: &'a str,
    pub detail: Option<&'a str>,
    pub question_json: Option<String>,
    pub options_json: Option<String>,
    pub kind_json: Option<String>,
    pub logits_json: Option<String>,
    pub probabilities_json: Option<String>,
    pub verdict_matched: Option<bool>,
    pub verdict_choice: Option<&'a str>,
    /// The menu entry a multiple-match question asked about, named whether or
    /// not rverdict matched it. Null for other framings and for rows recorded
    /// before the column existed.
    pub target: Option<&'a str>,
    pub confidence: Option<f64>,
    pub llm_matched: Option<bool>,
    pub llm_choices_json: Option<String>,
    pub llm_option: Option<i64>,
    pub agrees: Option<bool>,
    pub state_tokens: Option<i64>,
    pub truncated: Option<bool>,
    pub duration_ms: Option<i64>,
}

/// A recorded verdict, as reports and the UI read it.
#[derive(Debug, Clone, Serialize)]
pub struct VerdictRow {
    pub id: i64,
    pub step_id: i64,
    pub run_id: i64,
    pub account_email: String,
    pub rule_index: i64,
    pub rule_legacy_id: i64,
    pub rule_name: String,
    pub framing: String,
    pub model: String,
    pub backend: Option<String>,
    pub precision: Option<String>,
    pub status: String,
    pub detail: Option<String>,
    pub options_json: Option<String>,
    #[serde(skip)]
    pub kind_json: Option<String>,
    #[serde(skip)]
    pub logits_json: Option<String>,
    pub probabilities_json: Option<String>,
    pub verdict_matched: Option<bool>,
    pub verdict_choice: Option<String>,
    pub target: Option<String>,
    pub confidence: Option<f64>,
    pub llm_matched: Option<bool>,
    pub llm_choices_json: Option<String>,
    pub llm_option: Option<i64>,
    pub agrees: Option<bool>,
    pub state_tokens: Option<i64>,
    pub truncated: Option<bool>,
    pub duration_ms: Option<i64>,
    pub created_at: String,
}

/// A completed action plan for a message, to match Gmail changes against.
#[derive(Debug, Clone)]
pub struct AppliedPlan {
    pub message_id: i64,
    pub run_id: i64,
    pub rule_index: i64,
    pub step_id: Option<i64>,
    pub add_label_ids: Vec<String>,
    pub remove_label_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Feedback {
    pub step_id: Option<i64>,
    pub verdict_id: Option<i64>,
    pub kind: String,
    pub label_id: String,
    pub created_at: String,
}

const VERDICT_COLUMNS: &str =
    "v.id, v.step_id, v.run_id, v.account_email, v.rule_index, v.rule_legacy_id,
    v.rule_name, v.framing, v.model, v.backend, v.precision, v.status, v.detail, v.options_json,
    v.kind_json, v.logits_json, v.probabilities_json, v.verdict_matched, v.verdict_choice,
    v.target, v.confidence, v.llm_matched, v.llm_choices_json, v.llm_option, v.agrees,
    v.state_tokens, v.truncated, v.duration_ms, v.created_at";

fn verdict_row(row: &rusqlite::Row<'_>) -> Result<VerdictRow> {
    Ok(VerdictRow {
        id: row.get(0)?,
        step_id: row.get(1)?,
        run_id: row.get(2)?,
        account_email: row.get(3)?,
        rule_index: row.get(4)?,
        rule_legacy_id: row.get(5)?,
        rule_name: row.get(6)?,
        framing: row.get(7)?,
        model: row.get(8)?,
        backend: row.get(9)?,
        precision: row.get(10)?,
        status: row.get(11)?,
        detail: row.get(12)?,
        options_json: row.get(13)?,
        kind_json: row.get(14)?,
        logits_json: row.get(15)?,
        probabilities_json: row.get(16)?,
        verdict_matched: row.get(17)?,
        verdict_choice: row.get(18)?,
        target: row.get(19)?,
        confidence: row.get(20)?,
        llm_matched: row.get(21)?,
        llm_choices_json: row.get(22)?,
        llm_option: row.get(23)?,
        agrees: row.get(24)?,
        state_tokens: row.get(25)?,
        truncated: row.get(26)?,
        duration_ms: row.get(27)?,
        created_at: row.get(28)?,
    })
}

/// Steps the LLM decided: a successful LLM attempt for the same rule.
const LLM_DECIDED: &str = "s.outcome IN ('matched', 'no_match', 'matched_no_action')
    AND EXISTS (SELECT 1 FROM workflow_llm_attempts a
                WHERE a.run_id = s.run_id AND a.rule_index = s.rule_index AND a.status = 'succeeded')";

impl<'a> VerdictRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Newest first, so live mail is shadowed before the backfill.
    pub fn pending_steps(&self, model: &str, limit: usize) -> Result<Vec<PendingStep>> {
        let sql = format!(
            "SELECT s.id, s.run_id, s.rule_index, s.rule_legacy_id, s.rule_name, s.outcome,
                    s.decision_json, r.account_email, rs.rules_json, m.message_json
             FROM workflow_steps s
             JOIN workflow_runs r ON r.id = s.run_id
             JOIN workflow_rule_sets rs ON rs.id = r.rule_set_id
             JOIN workflow_messages m ON m.id = r.message_id
             WHERE {LLM_DECIDED}
               AND NOT EXISTS (SELECT 1 FROM workflow_verdicts v WHERE v.step_id = s.id AND v.model = ?1)
             ORDER BY s.id DESC
             LIMIT ?2"
        );
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map(params![model, limit as i64], |row| {
            Ok(PendingStep {
                step_id: row.get(0)?,
                run_id: row.get(1)?,
                rule_index: row.get(2)?,
                rule_legacy_id: row.get(3)?,
                rule_name: row.get(4)?,
                outcome: row.get(5)?,
                reply: row.get(6)?,
                account_email: row.get(7)?,
                rules_json: row.get(8)?,
                message_json: row.get(9)?,
            })
        })?;
        rows.collect()
    }

    /// `(pending, done)` step counts for `model`.
    pub fn progress(&self, model: &str) -> Result<(i64, i64)> {
        let sql = format!(
            "SELECT
                 COUNT(*) FILTER (WHERE NOT EXISTS
                     (SELECT 1 FROM workflow_verdicts v WHERE v.step_id = s.id AND v.model = ?1)),
                 COUNT(*) FILTER (WHERE EXISTS
                     (SELECT 1 FROM workflow_verdicts v WHERE v.step_id = s.id AND v.model = ?1))
             FROM workflow_steps s WHERE {LLM_DECIDED}"
        );
        self.conn
            .query_row(&sql, params![model], |row| Ok((row.get(0)?, row.get(1)?)))
    }

    pub fn insert(&self, v: &NewVerdict<'_>) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO workflow_verdicts (
                 step_id, run_id, account_email, rule_index, rule_legacy_id, rule_name, framing,
                 model, backend, precision, status, detail, question_json, options_json, kind_json,
                 logits_json, probabilities_json, verdict_matched, verdict_choice, target,
                 confidence, llm_matched, llm_choices_json, llm_option, agrees, state_tokens,
                 truncated, duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                     ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28)",
            params![
                v.step_id,
                v.run_id,
                v.account_email,
                v.rule_index,
                v.rule_legacy_id,
                v.rule_name,
                v.framing,
                v.model,
                v.backend,
                v.precision,
                v.status,
                v.detail,
                v.question_json,
                v.options_json,
                v.kind_json,
                v.logits_json,
                v.probabilities_json,
                v.verdict_matched,
                v.verdict_choice,
                v.target,
                v.confidence,
                v.llm_matched,
                v.llm_choices_json,
                v.llm_option,
                v.agrees,
                v.state_tokens,
                v.truncated,
                v.duration_ms,
            ],
        )?;
        Ok(())
    }

    /// Every verdict, oldest first, optionally for one model.
    pub fn all(&self, model: Option<&str>) -> Result<Vec<VerdictRow>> {
        let sql = format!(
            "SELECT {VERDICT_COLUMNS} FROM workflow_verdicts v
             WHERE ?1 IS NULL OR v.model = ?1 ORDER BY v.id"
        );
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map(params![model], verdict_row)?;
        rows.collect()
    }

    pub fn for_message(&self, message_id: i64) -> Result<Vec<VerdictRow>> {
        let sql = format!(
            "SELECT {VERDICT_COLUMNS} FROM workflow_verdicts v
             JOIN workflow_runs r ON r.id = v.run_id
             WHERE r.message_id = ?1 ORDER BY v.rule_index, v.framing"
        );
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map(params![message_id], verdict_row)?;
        rows.collect()
    }

    /// The state and question of a verdict, for exports.
    pub fn export_inputs(
        &self,
        verdict_id: i64,
    ) -> Result<Option<(Option<String>, Option<String>)>> {
        self.conn
            .query_row(
                "SELECT m.message_json, v.question_json FROM workflow_verdicts v
                 JOIN workflow_runs r ON r.id = v.run_id
                 JOIN workflow_messages m ON m.id = r.message_id
                 WHERE v.id = ?1",
                params![verdict_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
    }

    /// Completed action plans for a Gmail message, newest first.
    pub fn applied_plans(
        &self,
        account_email: &str,
        gmail_message_id: &str,
    ) -> Result<Vec<AppliedPlan>> {
        let mut statement = self.conn.prepare(
            "SELECT m.id, p.run_id, p.rule_index, s.id, p.add_label_ids, p.remove_label_ids
             FROM workflow_action_plans p
             JOIN workflow_runs r ON r.id = p.run_id
             JOIN workflow_messages m ON m.id = r.message_id
             LEFT JOIN workflow_steps s
                 ON s.run_id = p.run_id AND s.rule_index = p.rule_index AND s.outcome = 'matched'
             WHERE m.account_email = ?1 AND m.gmail_message_id = ?2 AND p.state = 'succeeded'
             ORDER BY p.id DESC",
        )?;
        let rows = statement.query_map(params![account_email, gmail_message_id], |row| {
            let ids = |text: Option<String>| -> Vec<String> {
                text.and_then(|t| serde_json::from_str(&t).ok())
                    .unwrap_or_default()
            };
            Ok(AppliedPlan {
                message_id: row.get(0)?,
                run_id: row.get(1)?,
                rule_index: row.get(2)?,
                step_id: row.get(3)?,
                add_label_ids: ids(row.get(4)?),
                remove_label_ids: ids(row.get(5)?),
            })
        })?;
        rows.collect()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_feedback(
        &self,
        account_email: &str,
        message_id: i64,
        run_id: Option<i64>,
        step_id: Option<i64>,
        rule_index: Option<i64>,
        kind: &str,
        label_id: &str,
        gmail_history_id: &str,
    ) -> Result<bool> {
        let inserted = self.conn.execute(
            "INSERT OR IGNORE INTO workflow_feedback
                 (account_email, message_id, run_id, step_id, rule_index, kind, label_id, gmail_history_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![account_email, message_id, run_id, step_id, rule_index, kind, label_id, gmail_history_id],
        )?;
        Ok(inserted > 0)
    }

    /// Sets or clears the user's thumbs up/down on a step.
    pub fn set_rating(&self, step_id: i64, up: Option<bool>) -> Result<()> {
        let explicit = format!("explicit:{step_id}");
        self.conn.execute(
            "DELETE FROM workflow_feedback
             WHERE step_id = ?1 AND verdict_id IS NULL
               AND kind IN ('thumbs_up', 'thumbs_down')",
            params![step_id],
        )?;
        let Some(up) = up else {
            return Ok(());
        };
        self.conn.execute(
            "INSERT INTO workflow_feedback
                 (account_email, message_id, run_id, step_id, rule_index, kind, gmail_history_id)
             SELECT r.account_email, r.message_id, s.run_id, s.id, s.rule_index, ?2, ?3
             FROM workflow_steps s JOIN workflow_runs r ON r.id = s.run_id WHERE s.id = ?1",
            params![
                step_id,
                if up { "thumbs_up" } else { "thumbs_down" },
                explicit
            ],
        )?;
        Ok(())
    }

    pub fn feedback_for_steps(&self) -> Result<Vec<Feedback>> {
        let mut statement = self.conn.prepare(
            "SELECT step_id, verdict_id, kind, label_id, created_at
             FROM workflow_feedback ORDER BY id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(Feedback {
                step_id: row.get(0)?,
                verdict_id: row.get(1)?,
                kind: row.get(2)?,
                label_id: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    pub fn feedback_for_message(&self, message_id: i64) -> Result<Vec<Feedback>> {
        let mut statement = self.conn.prepare(
            "SELECT step_id, verdict_id, kind, label_id, created_at FROM workflow_feedback
             WHERE message_id = ?1 ORDER BY id",
        )?;
        let rows = statement.query_map(params![message_id], |row| {
            Ok(Feedback {
                step_id: row.get(0)?,
                verdict_id: row.get(1)?,
                kind: row.get(2)?,
                label_id: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    /// Sets or clears the user's thumbs on one verdict row, so being wrong
    /// about one label does not mark the step's other labels wrong too.
    pub fn set_verdict_rating(&self, verdict_id: i64, up: Option<bool>) -> Result<()> {
        let explicit = format!("explicit:verdict:{verdict_id}");
        self.conn.execute(
            "DELETE FROM workflow_feedback
             WHERE verdict_id = ?1 AND kind IN ('verdict_right', 'verdict_wrong')",
            params![verdict_id],
        )?;
        let Some(up) = up else {
            return Ok(());
        };
        self.conn.execute(
            "INSERT INTO workflow_feedback
                 (account_email, message_id, run_id, step_id, rule_index, verdict_id, kind,
                  gmail_history_id)
             SELECT r.account_email, r.message_id, v.run_id, v.step_id, v.rule_index, v.id, ?2, ?3
             FROM workflow_verdicts v JOIN workflow_runs r ON r.id = v.run_id
             WHERE v.id = ?1",
            params![
                verdict_id,
                if up { "verdict_right" } else { "verdict_wrong" },
                explicit
            ],
        )?;
        Ok(())
    }
}
