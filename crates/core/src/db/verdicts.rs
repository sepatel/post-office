use std::collections::HashMap;

use rusqlite::{named_params, params, Connection, OptionalExtension, Result};
use serde::Serialize;

use crate::gmail::models::Message;
use crate::rules::email_view::EmailView;

pub struct VerdictRepository<'a> {
    conn: &'a Connection,
}

/// Who was right about a decision, as the user adjudicated it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Adjudication {
    /// The LLM's answer was right.
    Llm,
    /// rverdict's answer was right, and the LLM's was not.
    Rverdict,
    /// Neither was right: a case where the rule does not settle the question
    /// and the model has not learned it.
    Neither,
}

/// The kinds that answer "who was right", and their SQL form.
const ADJUDICATION_KINDS: &str = "'llm_right', 'llm_wrong', 'rverdict_right', 'both_wrong'";

impl Adjudication {
    pub fn kind(self) -> &'static str {
        match self {
            Self::Llm => "llm_right",
            Self::Rverdict => "rverdict_right",
            Self::Neither => "both_wrong",
        }
    }

    /// The wire form the UI sends, which is shorter than the stored kind.
    pub fn from_name(name: &str) -> Result<Self, String> {
        match name {
            "llm" => Ok(Self::Llm),
            "rverdict" => Ok(Self::Rverdict),
            "neither" => Ok(Self::Neither),
            other => Err(format!(
                "who was right is llm, rverdict or neither, not {other:?}"
            )),
        }
    }

    pub fn from_kind(kind: &str) -> Option<Self> {
        match kind {
            "llm_right" => Some(Self::Llm),
            "rverdict_right" => Some(Self::Rverdict),
            "both_wrong" => Some(Self::Neither),
            _ => None,
        }
    }

    /// What the user settled about a decision. Feedback about one label is more
    /// specific than a rating of the step it fanned out to, so it wins.
    pub fn from_feedback(step: &[&Feedback], row: &[&Feedback]) -> Option<Self> {
        step.iter()
            .chain(row)
            .filter_map(|f| Self::from_kind(&f.kind))
            .next_back()
    }
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

/// Which disagreements to review. Defaults to the newest of every kind.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
pub struct DisagreementQuery {
    pub account: Option<String>,
    pub rule_legacy_id: Option<i64>,
    /// A framing family (`noul`, `binary`, `menu`, `multi`), matching the
    /// `multi:<target>` rows of multiple-match rules too.
    pub framing: Option<String>,
    /// Only decisions the model was [`crate::decision::report::ACT_THRESHOLD`]
    /// confident on, which are the ones that would be acted on.
    pub confident_only: bool,
    /// Restrict to decided or still-open disagreements.
    pub adjudicated: Option<bool>,
    pub model: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

/// One recorded disagreement, with enough of the email to judge it.
/// A multiple-match question whose menu entry was never named. The framing
/// still says which entry it asked about, and the run's snapshot says what it
/// was called.
#[derive(Debug, Clone)]
pub struct UnnamedMulti {
    pub verdict_id: i64,
    pub rule_set_id: i64,
    pub rule_index: i64,
    /// The entry's position in the rule's menu, from `multi:<index>`.
    pub index: i64,
    pub account_email: String,
    pub rules_json: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Disagreement {
    #[serde(flatten)]
    pub row: VerdictRow,
    pub message_id: i64,
    pub subject: String,
    pub sender: String,
    pub snippet: String,
    /// What the user already settled, if anything.
    pub adjudication: Option<Adjudication>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DisagreementPage {
    pub rows: Vec<Disagreement>,
    /// Matches before `limit` and `offset`.
    pub total: i64,
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

    /// The verdicts where the two answers differ, newest first, for the review
    /// queue. The page and its total share one filter.
    pub fn disagreements(
        &self,
        query: &DisagreementQuery,
        threshold: f64,
    ) -> Result<DisagreementPage> {
        let filter = format!(
            "WHERE v.status = 'ok' AND v.agrees = 0
               AND (:model IS NULL OR v.model = :model)
               AND (:account IS NULL OR v.account_email = :account)
               AND (:rule IS NULL OR v.rule_legacy_id = :rule)
               AND (:framing IS NULL OR v.framing LIKE :framing || '%')
               AND (:confident = 0 OR v.confidence >= :threshold)
               AND (:adjudicated IS NULL OR EXISTS (SELECT 1 FROM workflow_feedback f
                    WHERE f.verdict_id = v.id AND f.kind IN ({ADJUDICATION_KINDS})) = :adjudicated)"
        );
        let total: i64 = self.conn.query_row(
            &format!("SELECT COUNT(*) FROM workflow_verdicts v {filter}"),
            named_params! {
                ":model": query.model,
                ":account": query.account,
                ":rule": query.rule_legacy_id,
                ":framing": query.framing,
                ":confident": query.confident_only,
                ":threshold": threshold,
                ":adjudicated": query.adjudicated,
            },
            |row| row.get(0),
        )?;

        let mut statement = self.conn.prepare(&format!(
            "SELECT {VERDICT_COLUMNS}, r.message_id FROM workflow_verdicts v
             JOIN workflow_runs r ON r.id = v.run_id
             {filter}
             ORDER BY v.id DESC LIMIT :limit OFFSET :offset"
        ))?;
        let found = statement
            .query_map(
                named_params! {
                    ":model": query.model,
                    ":account": query.account,
                    ":rule": query.rule_legacy_id,
                    ":framing": query.framing,
                    ":confident": query.confident_only,
                    ":threshold": threshold,
                    ":adjudicated": query.adjudicated,
                    ":limit": query.limit,
                    ":offset": query.offset,
                },
                |row| Ok((verdict_row(row)?, row.get::<_, i64>(29)?)),
            )?
            .collect::<Result<Vec<_>>>()?;

        let mut emails = self.emails(found.iter().map(|(_, id)| *id).collect())?;
        let mut settled = self.adjudicated_by_verdict()?;
        Ok(DisagreementPage {
            total,
            rows: found
                .into_iter()
                .map(|(row, message_id)| {
                    let (subject, sender, snippet) = emails.remove(&message_id).unwrap_or_default();
                    Disagreement {
                        adjudication: settled.remove(&row.id),
                        row,
                        message_id,
                        subject,
                        sender,
                        snippet,
                    }
                })
                .collect(),
        })
    }

    /// `(subject, sender, snippet)` by message id, parsed once per message
    /// rather than once per row.
    fn emails(&self, ids: Vec<i64>) -> Result<HashMap<i64, (String, String, String)>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let list = ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
        let mut statement = self.conn.prepare(&format!(
            "SELECT id, message_json FROM workflow_messages WHERE id IN ({list})"
        ))?;
        let rows = statement.query_map([], |row| {
            let id: i64 = row.get(0)?;
            let message = serde_json::from_str::<Message>(&row.get::<_, String>(1)?).ok();
            let view = message.as_ref().map(EmailView::from_message);
            let text = |name: &str| {
                view.as_ref()
                    .and_then(|v| v.header(name))
                    .unwrap_or_default()
                    .to_owned()
            };
            Ok((
                id,
                (
                    text("Subject"),
                    text("From"),
                    message.map(|m| m.snippet).unwrap_or_default(),
                ),
            ))
        })?;
        rows.collect()
    }

    fn adjudicated_by_verdict(&self) -> Result<HashMap<i64, Adjudication>> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT verdict_id, kind FROM workflow_feedback
             WHERE verdict_id IS NOT NULL AND kind IN ({ADJUDICATION_KINDS})"
        ))?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>>>()?;
        let mut settled = HashMap::new();
        for (id, kind) in rows {
            if let Some(right) = Adjudication::from_kind(&kind) {
                settled.insert(id, right);
            }
        }
        Ok(settled)
    }

    /// The multiple-match questions recorded before targets existed, with the
    /// snapshot needed to name what they asked about.
    pub fn unnamed_multi(&self) -> Result<Vec<UnnamedMulti>> {
        let mut statement = self.conn.prepare(
            "SELECT v.id, r.rule_set_id, v.rule_index,
                    CAST(substr(v.framing, instr(v.framing, ':') + 1) AS INTEGER),
                    v.account_email, rs.rules_json
             FROM workflow_verdicts v
             JOIN workflow_runs r ON r.id = v.run_id
             JOIN workflow_rule_sets rs ON rs.id = r.rule_set_id
             WHERE v.status = 'ok' AND v.target IS NULL AND v.framing LIKE 'multi:%'",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(UnnamedMulti {
                verdict_id: row.get(0)?,
                rule_set_id: row.get(1)?,
                rule_index: row.get(2)?,
                index: row.get(3)?,
                account_email: row.get(4)?,
                rules_json: row.get(5)?,
            })
        })?;
        rows.collect()
    }

    /// Names the menu entry a row asked about. Rows already named are left
    /// alone: a name recorded when the question was asked is better than one
    /// re-derived from a menu that may since have moved.
    pub fn set_targets(&self, named: &[(i64, String)]) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        let mut count = 0;
        {
            let mut statement = tx.prepare(
                "UPDATE workflow_verdicts SET target = ?2 WHERE id = ?1 AND target IS NULL",
            )?;
            for (id, target) in named {
                count += statement.execute(params![id, target])?;
            }
        }
        tx.commit()?;
        Ok(count)
    }

    /// The state and question of a verdict, for exports.
    pub fn export_inputs(        &self,
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

    /// Records who was right about one verdict, replacing any earlier
    /// statement about it. A rating of the whole step stays a separate signal:
    /// being wrong about one label does not condemn the other labels the step
    /// fanned out to.
    pub fn set_adjudication(&self, verdict_id: i64, right: Option<Adjudication>) -> Result<()> {
        self.conn.execute(
            &format!(
                "DELETE FROM workflow_feedback
                 WHERE verdict_id = ?1 AND kind IN ({ADJUDICATION_KINDS})"
            ),
            params![verdict_id],
        )?;
        let Some(right) = right else {
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
                right.kind(),
                format!("explicit:verdict:{verdict_id}")
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
}

#[cfg(test)]
mod tests {
    use crate::db::Database;
    use crate::decision::report::ACT_THRESHOLD;
    use crate::decision::worker::shadow_batch;
    use crate::decision::worker::tests::{seeded_multi, FirstOption};

    use super::*;

    /// The one seeded rule whose two answers differ, shadowed.
    fn shadowed() -> Database {
        let db = seeded_multi();
        shadow_batch(&db, &FirstOption::default(), 10).unwrap();
        db
    }

    fn query(over: DisagreementQuery) -> DisagreementQuery {
        DisagreementQuery { limit: 50, ..over }
    }

    #[test]
    fn disagreements_lists_only_the_ones_that_differ() {
        let db = shadowed();
        let page = db
            .with_verdicts(|r| r.disagreements(&query(DisagreementQuery::default()), ACT_THRESHOLD))
            .unwrap();
        assert_eq!(page.total, 1);
        let row = &page.rows[0];
        assert_eq!(row.row.target.as_deref(), Some("\"Work\""));
        assert_eq!(row.row.rule_name, "Sort");
        // Enough of the email to rule on it without opening the message.
        assert_eq!(row.message_id, 1);
        assert_eq!(row.subject, "Invoice 4411");
        assert_eq!(row.adjudication, None);
    }

    #[test]
    fn the_queue_filters_by_family_and_by_whether_it_was_ruled_on() {
        let db = shadowed();
        let open = DisagreementQuery {
            adjudicated: Some(false),
            ..DisagreementQuery::default()
        };
        let family = DisagreementQuery {
            framing: Some("multi".into()),
            ..open.clone()
        };
        assert_eq!(
            db.with_verdicts(|r| r.disagreements(&query(family), ACT_THRESHOLD))
                .unwrap()
                .total,
            1
        );
        assert_eq!(
            db.with_verdicts(|r| r.disagreements(
                &query(DisagreementQuery {
                    framing: Some("menu".into()),
                    ..open.clone()
                }),
                ACT_THRESHOLD
            ))
            .unwrap()
            .total,
            0
        );

        let row = db
            .with_verdicts(|r| r.disagreements(&query(open.clone()), ACT_THRESHOLD))
            .unwrap()
            .rows
            .swap_remove(0)
            .row;
        db.with_verdicts(|r| r.set_adjudication(row.id, Some(Adjudication::Rverdict)))
            .unwrap();

        let ruled = DisagreementQuery {
            adjudicated: Some(true),
            ..open.clone()
        };
        let page = db
            .with_verdicts(|r| r.disagreements(&query(ruled), ACT_THRESHOLD))
            .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.rows[0].adjudication, Some(Adjudication::Rverdict));
        assert_eq!(
            db.with_verdicts(|r| r.disagreements(&query(open.clone()), ACT_THRESHOLD))
                .unwrap()
                .total,
            0
        );
    }

    /// The count is what a queue pages against, so it has to ignore the page
    /// window rather than report the rows that fit in it.
    #[test]
    fn the_total_outranks_the_page_size() {
        let db = shadowed();
        let empty = DisagreementQuery {
            limit: 0,
            ..DisagreementQuery::default()
        };
        let page = db
            .with_verdicts(|r| r.disagreements(&empty, ACT_THRESHOLD))
            .unwrap();
        assert!((page.total, page.rows.len()) == (1, 0));
    }

    /// A database that already carries verdicts and feedback has to come
    /// through the rename intact, or every past ruling silently stops counting.
    #[test]
    fn the_rename_carries_existing_ratings_over() {
        let db = shadowed();
        let work = db
            .with_verdicts(|r| r.all(None))
            .unwrap()
            .into_iter()
            .find(|r| r.agrees == Some(false))
            .expect("a disagreement");
        db.with_verdicts(|r| {
            r.record_feedback(
                "a@x",
                1,
                Some(1),
                Some(2),
                Some(1),
                "thumbs_up",
                "",
                "explicit:2",
            )
        })
        .unwrap();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO workflow_feedback
                     (account_email, message_id, run_id, step_id, rule_index, verdict_id, kind,
                      gmail_history_id)
                 VALUES ('a@x', 1, 1, 2, 1, ?1, 'verdict_right', 'explicit:verdict:1')",
                params![work.id],
            )
        })
        .unwrap();

        db.with_conn(|conn| {
            conn.execute_batch(include_str!(
                "../../../../migrations/032_verdict_adjudication.sql"
            ))
        })
        .unwrap();

        let kinds: Vec<_> = db
            .with_verdicts(|r| r.feedback_for_steps())
            .unwrap()
            .iter()
            .map(|f| f.kind.as_str().to_owned())
            .collect();
        assert_eq!(kinds, ["llm_right", "llm_right"]);

        // The verdict itself is untouched, and the queue still finds it.
        let page = db
            .with_verdicts(|r| {
                r.disagreements(
                    &DisagreementQuery {
                        adjudicated: Some(true),
                        limit: 50,
                        ..DisagreementQuery::default()
                    },
                    ACT_THRESHOLD,
                )
            })
            .unwrap();
        assert!((page.total, page.rows[0].row.id) == (1, work.id));
    }
}
