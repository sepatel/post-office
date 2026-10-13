//! Names the menu entries behind multiple-match questions recorded before
//! targets existed.
//!
//! Migration 030 added `workflow_verdicts.target` so a "no" row could be
//! attributed to the label it asked about, and left the rows already written
//! with a null. Without it those rows are only addressable by position, which
//! no one can read. The question is recoverable without running the model: the
//! framing names the position, and the run's snapshot names the menu that
//! position was in.

use std::collections::HashMap;

use serde::Serialize;

use crate::db::verdicts::UnnamedMulti;
use crate::db::Database;
use crate::gmail::models::Label;
use crate::rules::engine::choice_catalog;
use crate::workflow::snapshot_rule;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct RepairSummary {
    /// Rows now naming the entry they asked about.
    pub named: usize,
    /// Rows whose snapshot is gone or whose framing points past the menu.
    pub unresolved: usize,
}

/// Names every multiple-match row that can be named. Idempotent, and safe to
/// re-run: a row that already carries a name keeps the one it was recorded
/// with.
pub fn repair_targets(db: &Database) -> rusqlite::Result<RepairSummary> {
    let rows = db.with_verdicts(|repo| repo.unnamed_multi())?;
    let mut labels: HashMap<String, Vec<Label>> = HashMap::new();
    // The snapshot is versioned, so one menu serves every row sharing a rule
    // set and position, however many runs it spans.
    let mut menus: HashMap<(i64, i64), Option<Vec<String>>> = HashMap::new();
    let mut named: Vec<(i64, String)> = Vec::with_capacity(rows.len());

    for row in &rows {
        let key = (row.rule_set_id, row.rule_index);
        let menu = match menus.get(&key) {
            Some(menu) => menu.clone(),
            None => {
                let menu = menu(db, row, &mut labels)?;
                menus.insert(key, menu.clone());
                menu
            }
        };
        let Some(entry) = menu
            .as_ref()
            .and_then(|menu| menu.get(usize::try_from(row.index).ok()?))
        else {
            continue;
        };
        named.push((row.verdict_id, entry.clone()));
    }

    let unresolved = rows.len() - named.len();
    let named = db.with_verdicts(|repo| repo.set_targets(&named))?;
    Ok(RepairSummary {
        named,
        unresolved,
    })
}

/// The menu a row's rule offered, as the entry names it stored.
fn menu(
    db: &Database,
    row: &UnnamedMulti,
    labels: &mut HashMap<String, Vec<Label>>,
) -> rusqlite::Result<Option<Vec<String>>> {
    if !labels.contains_key(&row.account_email) {
        let cached = db
            .with_labels(|repo| repo.get(&row.account_email))?
            .map(|cached| cached.labels)
            .unwrap_or_default();
        labels.insert(row.account_email.clone(), cached);
    }
    let Some((rule, _)) = usize::try_from(row.rule_index)
        .ok()
        .and_then(|index| snapshot_rule(&row.rules_json, index))
    else {
        return Ok(None);
    };
    Ok(Some(
        choice_catalog(&rule, &labels[&row.account_email])
            .iter()
            .map(|choice| choice.name.clone())
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use rverdict_core::{Calibration, Request};

    use super::*;
    use crate::decision::worker::shadow_batch;
    use crate::decision::worker::tests::{seeded_multi, FirstOption};
    use crate::decision::{Answered, DecisionModel, Evaluation, ModelInfo};

    /// Fails every question, so every row is a "no" that has to be attributed by
    /// position alone.
    struct NeverMatches {
        info: ModelInfo,
        calibration: Calibration,
    }

    impl NeverMatches {
        fn stub() -> Self {
            Self {
                info: ModelInfo {
                    id: "stub@1".into(),
                    backend: "test".into(),
                    precision: "f32".into(),
                },
                calibration: Calibration::default(),
            }
        }
    }

    impl DecisionModel for NeverMatches {
        fn info(&self) -> &ModelInfo {
            &self.info
        }

        fn calibration(&self) -> &Calibration {
            &self.calibration
        }

        fn evaluate(&self, request: &Request) -> Result<Evaluation, String> {
            let answers = request
                .parse_questions()
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|(id, question)| {
                    let rendered = rverdict_core::render(&question);
                    let mut logits = vec![0.0; rendered.options.len()];
                    logits[1] = 2.0;
                    Answered {
                        id,
                        kind: rendered.kind,
                        logits: rverdict_core::Logits {
                            logits,
                            null_logits: None,
                            state_tokens: 12,
                        },
                    }
                })
                .collect();
            Ok(Evaluation {
                answers,
                truncated: false,
            })
        }
    }

    /// Shadowed rows whose targets were then dropped, as migration 030 left
    /// them: the framing still says which entry, the column does not.
    fn seeded_unnamed() -> Database {
        let db = seeded_multi();
        shadow_batch(&db, &NeverMatches::stub(), 10).unwrap();
        db.with_conn(|conn| conn.execute("UPDATE workflow_verdicts SET target = NULL", []))
            .unwrap();
        db
    }

    fn targets(db: &Database) -> Vec<(String, Option<String>)> {
        db.with_verdicts(|repo| repo.all(None))
            .unwrap()
            .into_iter()
            .filter(|row| row.framing.starts_with("multi:"))
            .map(|row| (row.framing, row.target))
            .collect()
    }

    #[test]
    fn names_the_entry_a_row_asked_about() {
        let db = seeded_unnamed();
        assert!(targets(&db).iter().all(|(_, target)| target.is_none()));

        let summary = repair_targets(&db).unwrap();
        assert_eq!((summary.named, summary.unresolved), (2, 0));
        assert_eq!(
            targets(&db),
            [
                ("multi:0".into(), Some("\"Financial\"".into())),
                ("multi:1".into(), Some("\"Work\"".into()))
            ]
        );
    }

    /// Re-running must not re-derive: a row keeps the name it was recorded
    /// with, which stays right even if the menu has since moved.
    #[test]
    fn re_running_changes_nothing() {
        let db = seeded_unnamed();
        repair_targets(&db).unwrap();
        db.with_conn(|conn| {
            conn.execute(
                "UPDATE workflow_verdicts SET target = '\"Renamed\"' WHERE framing = 'multi:0'",
                [],
            )
        })
        .unwrap();

        let summary = repair_targets(&db).unwrap();
        assert_eq!((summary.named, summary.unresolved), (0, 0));
        assert_eq!(
            targets(&db).into_iter().find(|(framing, _)| framing == "multi:0"),
            Some(("multi:0".into(), Some("\"Renamed\"".into())))
        );
    }

    /// A row whose rule has left the snapshot cannot be named, and saying so is
    /// better than guessing at a position that no longer means anything.
    #[test]
    fn a_row_whose_rule_is_gone_is_left_alone() {
        let db = seeded_unnamed();
        db.with_conn(|conn| conn.execute("UPDATE workflow_rule_sets SET rules_json = '{}'", []))
            .unwrap();

        let summary = repair_targets(&db).unwrap();
        assert_eq!((summary.named, summary.unresolved), (0, 2));
        assert!(targets(&db).iter().all(|(_, target)| target.is_none()));
    }

    #[test]
    fn a_database_with_nothing_to_repair_is_a_no_op() {
        let db = seeded_multi();
        shadow_batch(&db, &FirstOption::default(), 10).unwrap();
        assert_eq!(
            repair_targets(&db).unwrap(),
            RepairSummary {
                named: 0,
                unresolved: 0
            }
        );
    }
}
