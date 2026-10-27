//! Per-rule shadow reports: how often rverdict agrees with the LLM, how
//! often it is confidently wrong, what user feedback says, and whether a
//! calibration refit on the rule's own history would help.

use std::collections::{BTreeMap, HashMap, HashSet};

use rverdict_core::{Calibration, Logits, RenderedKind};
use rverdict_eval::calibrate::{self, Capture, Form};
use serde::Serialize;

use crate::db::verdicts::{Adjudication, Feedback, VerdictRow};
use crate::db::Database;
use crate::decision::questions::framing_group;

/// Confidence at which a rule could act on rverdict's answer alone.
pub const ACT_THRESHOLD: f64 = 0.9;
/// Decisions a rule needs before its calibration refit is reported.
const MIN_FOR_CALIBRATION: usize = 40;

#[derive(Debug, Clone, Default, Serialize)]
pub struct RuleReport {
    pub account_email: String,
    pub rule_legacy_id: i64,
    pub rule_name: String,
    pub framing: String,
    pub model: String,
    pub decisions: usize,
    pub skipped: usize,
    pub errors: usize,
    /// Share of decisions where rverdict and the LLM agree.
    pub agreement: Option<f64>,
    /// The LLM's most common answer and its share: agreement is only
    /// meaningful against this baseline.
    pub llm_majority_share: Option<f64>,
    /// Agreement on the decisions where the LLM gave a less common answer.
    pub minority_agreement: Option<f64>,
    pub minority_decisions: usize,
    /// Decisions at or above [`ACT_THRESHOLD`].
    pub confident: usize,
    pub confident_share: Option<f64>,
    /// Confident decisions that disagree with the LLM: the promotion bar is
    /// at most 5%.
    pub confident_disagreements: usize,
    pub confident_disagreement_rate: Option<f64>,
    /// Decisions with user feedback on the LLM's answer.
    pub feedback: usize,
    pub feedback_llm_wrong: usize,
    /// Of the decisions the user marked wrong, how many rverdict disagreed
    /// with the LLM on.
    pub feedback_verdict_disagreed: usize,
    /// Where the two answers differed.
    pub disagreements: usize,
    /// Of those, how far the user has ruled on who was right.
    pub adjudicated: usize,
    pub adjudications_llm_right: usize,
    pub adjudications_rverdict_right: usize,
    pub adjudications_both_wrong: usize,
    /// Of the disagreements with a named winner, the share rverdict won. This
    /// is the number a rule has to clear before its answers are worth acting
    /// on, and it rests entirely on the user having ruled.
    pub rverdict_wins: Option<f64>,
    pub calibration: Option<CalibrationReport>,
    pub median_ms: Option<i64>,
    pub truncated: usize,
}

/// Held-out comparison of the model's calibration with one refit on half of
/// this rule's history (LLM answers as labels).
#[derive(Debug, Clone, Serialize)]
pub struct CalibrationReport {
    pub held_out: usize,
    pub nll_before: f64,
    pub nll_after: f64,
    pub ece_before: f64,
    pub ece_after: f64,
    pub accuracy_before: f64,
    pub accuracy_after: f64,
}

/// Feedback that says the LLM's answer was wrong.
pub fn is_negative(kind: &str) -> bool {
    matches!(
        kind,
        "label_removed"
            | "restored_from_trash"
            | "restored_from_spam"
            | "unarchived"
            | "unstarred"
            | "llm_wrong"
            | "rverdict_right"
    )
}

/// The option rverdict chose, as an index into the row's options.
/// `verdict_choice` names it only on a match, so a declined row otherwise
/// has no answer to write down. Ties keep the earlier option, as the
/// worker does.
pub fn verdict_option(row: &VerdictRow) -> Option<usize> {
    let probabilities: Vec<f64> = serde_json::from_str(row.probabilities_json.as_deref()?).ok()?;
    probabilities
        .iter()
        .copied()
        .enumerate()
        .fold(None, |best: Option<(usize, f64)>, (i, p)| match best {
            Some((_, seen)) if seen >= p => best,
            _ => Some((i, p)),
        })
        .map(|(index, _)| index)
}

/// Whether the user has settled what this row's gold label should be.
/// Fitting against the LLM's answer would then score either a label
/// already known to be wrong or the model's own output back at it.
fn is_settled(feedback: &[&Feedback]) -> bool {
    feedback
        .iter()
        .any(|f| f.kind == "llm_wrong" || Adjudication::from_kind(&f.kind).is_some())
}

pub fn rule_reports(
    db: &Database,
    model: Option<&str>,
    calibration: &Calibration,
) -> rusqlite::Result<Vec<RuleReport>> {
    let rows = db.with_verdicts(|repo| repo.all(model))?;
    let feedback = db.with_verdicts(|repo| repo.feedback_for_steps())?;
    Ok(build(&rows, &feedback, calibration))
}

fn build(rows: &[VerdictRow], feedback: &[Feedback], calibration: &Calibration) -> Vec<RuleReport> {
    // Feedback about one label is about that row alone. Step-level feedback is
    // a verdict on the step as a whole, so it applies to every row of that step
    // — but only where nothing more specific was said.
    let mut by_step: HashMap<i64, Vec<&Feedback>> = HashMap::new();
    let mut by_verdict: HashMap<i64, Vec<&Feedback>> = HashMap::new();
    for f in feedback {
        if let Some(verdict) = f.verdict_id {
            by_verdict.entry(verdict).or_default().push(f);
        } else if let Some(step) = f.step_id {
            by_step.entry(step).or_default().push(f);
        }
    }
    // Skips and errors carry no framing, so they are counted per rule.
    let mut failures: HashMap<(String, i64, String), (usize, usize)> = HashMap::new();
    let mut groups: BTreeMap<(String, i64, String, String), Vec<&VerdictRow>> = BTreeMap::new();
    for row in rows {
        if row.status == "ok" {
            groups
                .entry((
                    row.account_email.clone(),
                    row.rule_legacy_id,
                    framing_group(&row.framing).to_owned(),
                    row.model.clone(),
                ))
                .or_default()
                .push(row);
        } else {
            let entry = failures
                .entry((
                    row.account_email.clone(),
                    row.rule_legacy_id,
                    row.model.clone(),
                ))
                .or_default();
            if row.status == "error" {
                entry.1 += 1;
            } else {
                entry.0 += 1;
            }
        }
    }

    let mut reports: Vec<RuleReport> = groups
        .into_iter()
        .map(|((account_email, rule_legacy_id, framing, model), rows)| {
            let (skipped, errors) = failures
                .get(&(account_email.clone(), rule_legacy_id, model.clone()))
                .copied()
                .unwrap_or_default();
            let mut report = summarize(&rows, &by_step, &by_verdict, calibration);
            report.rule_name = rows.last().map(|r| r.rule_name.clone()).unwrap_or_default();
            report.account_email = account_email;
            report.rule_legacy_id = rule_legacy_id;
            report.framing = framing;
            report.model = model;
            report.skipped = skipped;
            report.errors = errors;
            report
        })
        .collect();
    reports.sort_by(|a, b| {
        (&a.account_email, &a.rule_name, &a.framing).cmp(&(
            &b.account_email,
            &b.rule_name,
            &b.framing,
        ))
    });
    reports
}

#[allow(clippy::cast_precision_loss)]
fn share(part: usize, whole: usize) -> Option<f64> {
    (whole > 0).then(|| part as f64 / whole as f64)
}

fn summarize(
    rows: &[&VerdictRow],
    feedback: &HashMap<i64, Vec<&Feedback>>,
    per_row: &HashMap<i64, Vec<&Feedback>>,
    calibration: &Calibration,
) -> RuleReport {
    let compared: Vec<&&VerdictRow> = rows.iter().filter(|r| r.agrees.is_some()).collect();
    let agreed = compared.iter().filter(|r| r.agrees == Some(true)).count();
    let disagreeing = compared.len() - agreed;

    let mut answers: HashMap<i64, usize> = HashMap::new();
    for row in rows {
        if let Some(option) = row.llm_option {
            *answers.entry(option).or_default() += 1;
        }
    }
    let majority = answers
        .iter()
        .max_by_key(|(_, n)| **n)
        .map(|(option, n)| (*option, *n));
    let labelled: usize = answers.values().sum();
    let minority: Vec<&&VerdictRow> = compared
        .iter()
        .copied()
        .filter(|r| majority.is_some_and(|(m, _)| r.llm_option.is_some_and(|o| o != m)))
        .collect();

    let confident: Vec<&&VerdictRow> = compared
        .iter()
        .copied()
        .filter(|r| r.confidence.is_some_and(|c| c >= ACT_THRESHOLD))
        .collect();
    let confident_disagreements = confident.iter().filter(|r| r.agrees == Some(false)).count();

    // Unlike the implicit counters below, a ruling is per label question: a
    // 57-label step the user marked wrong on two of them is two answered
    // questions, not one.
    let mut rulings = [0usize; 3];
    for row in compared.iter().filter(|r| r.agrees == Some(false)) {
        let step = feedback
            .get(&row.step_id)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let specific = per_row.get(&row.id).map(Vec::as_slice).unwrap_or_default();
        let index = match Adjudication::from_feedback(step, specific) {
            Some(Adjudication::Llm) => 0,
            Some(Adjudication::Rverdict) => 1,
            Some(Adjudication::Neither) => 2,
            None => continue,
        };
        rulings[index] += 1;
    }
    let adjudicated = rulings.iter().sum();
    let decided = rulings[0] + rulings[1];

    // A step is the LLM's one decision, but a step fans out to a row per
    // question — 57 labels for a multiple-match rule. Counting feedback per row
    // would score one thumbs-down as 57 wrong decisions, so these count steps.
    let mut rated: HashSet<i64> = HashSet::new();
    let mut marked_wrong: HashSet<i64> = HashSet::new();
    let mut verdict_disagreed: HashSet<i64> = HashSet::new();
    for row in &compared {
        let step_negative = feedback
            .get(&row.step_id)
            .is_some_and(|entries| entries.iter().any(|f| is_negative(&f.kind)));
        let specific = per_row.get(&row.id).map(Vec::as_slice).unwrap_or_default();
        if !step_negative && specific.is_empty() {
            continue;
        }
        rated.insert(row.step_id);
        if step_negative || specific.iter().any(|f| is_negative(&f.kind)) {
            marked_wrong.insert(row.step_id);
            if row.agrees == Some(false) {
                verdict_disagreed.insert(row.step_id);
            }
        }
    }
    let feedback_n = rated.len();
    let llm_wrong = marked_wrong.len();
    let verdict_disagreed = verdict_disagreed.len();

    let mut durations: Vec<i64> = rows.iter().filter_map(|r| r.duration_ms).collect();
    durations.sort_unstable();

    RuleReport {
        decisions: rows.len(),
        agreement: share(agreed, compared.len()),
        llm_majority_share: majority.and_then(|(_, n)| share(n, labelled)),
        minority_agreement: share(
            minority.iter().filter(|r| r.agrees == Some(true)).count(),
            minority.len(),
        ),
        minority_decisions: minority.len(),
        confident: confident.len(),
        confident_share: share(confident.len(), compared.len()),
        confident_disagreements,
        confident_disagreement_rate: share(confident_disagreements, confident.len()),
        feedback: feedback_n,
        feedback_llm_wrong: llm_wrong,
        feedback_verdict_disagreed: verdict_disagreed,
        disagreements: disagreeing,
        adjudicated,
        adjudications_llm_right: rulings[0],
        adjudications_rverdict_right: rulings[1],
        adjudications_both_wrong: rulings[2],
        rverdict_wins: share(rulings[1], decided),
        calibration: calibration_report(rows, feedback, per_row, calibration),
        median_ms: durations.get(durations.len() / 2).copied(),
        truncated: rows.iter().filter(|r| r.truncated == Some(true)).count(),
        ..RuleReport::default()
    }
}

/// The rows as calibration captures, labelled with the LLM's answers.
pub fn captures(rows: &[&VerdictRow]) -> Vec<Capture> {
    rows.iter()
        .filter_map(|row| {
            let kind: RenderedKind = serde_json::from_str(row.kind_json.as_deref()?).ok()?;
            let logits: Logits = serde_json::from_str(row.logits_json.as_deref()?).ok()?;
            Some(Capture {
                id: format!("{}-{}", row.step_id, row.framing),
                group: framing_group(&row.framing).to_owned(),
                kind,
                expected: usize::try_from(row.llm_option?).ok()?,
                logits,
            })
        })
        .collect()
}

/// Held-out comparison of the model's calibration with one refit on half of
/// this rule's history. Rows the user has ruled on are left out: the fit would
/// either chase a label already known to be wrong or score the model's own
/// output back at itself.
#[allow(clippy::too_many_arguments)]
fn calibration_report(
    rows: &[&VerdictRow],
    feedback: &HashMap<i64, Vec<&Feedback>>,
    per_row: &HashMap<i64, Vec<&Feedback>>,
    calibration: &Calibration,
) -> Option<CalibrationReport> {
    let captures = captures(
        &rows
            .iter()
            .filter(|row| {
                !is_settled(
                    feedback
                        .get(&row.step_id)
                        .map(Vec::as_slice)
                        .unwrap_or_default(),
                ) && !is_settled(per_row.get(&row.id).map(Vec::as_slice).unwrap_or_default())
            })
            .copied()
            .collect::<Vec<_>>(),
    );
    if captures.len() < MIN_FOR_CALIBRATION {
        return None;
    }
    let (train, test) = calibrate::split(&captures, 0.5);
    if train.len() < MIN_FOR_CALIBRATION / 2 || test.is_empty() {
        return None;
    }
    let fitted = calibrate::fit(&train, Form::Scalar, calibration);
    let before = calibrate::metrics(&test, calibration);
    let after = calibrate::metrics(&test, &fitted);
    Some(CalibrationReport {
        held_out: test.len(),
        nll_before: before.nll,
        nll_after: after.nll,
        ece_before: before.ece,
        ece_after: after.ece,
        accuracy_before: before.accuracy,
        accuracy_after: after.accuracy,
    })
}

#[cfg(test)]
mod tests {
    use rusqlite::params;
    use rverdict_core::Logits;

    use super::*;
    use crate::db::verdicts::NewVerdict;
    use crate::decision::worker::shadow_batch;
    use crate::decision::worker::tests::{seeded, seeded_multi, FirstOption};

    fn report(db: &Database, rule_name: &str) -> RuleReport {
        rule_reports(db, None, &Calibration::default())
            .unwrap()
            .into_iter()
            .find(|r| r.rule_name == rule_name)
            .unwrap()
    }

    /// The one verdict row in `db` where the two answers differ.
    fn disagreement(db: &Database) -> VerdictRow {
        db.with_verdicts(|r| r.all(None))
            .unwrap()
            .into_iter()
            .find(|v| v.agrees == Some(false))
            .expect("a disagreement")
    }

    #[test]
    fn reports_agreement_per_rule_and_framing() {
        let db = seeded();
        shadow_batch(&db, &FirstOption::default(), 10).unwrap();
        let reports = rule_reports(&db, None, &Calibration::default()).unwrap();
        let summary: Vec<_> = reports
            .iter()
            .map(|r| {
                (
                    r.rule_name.as_str(),
                    r.framing.as_str(),
                    r.decisions,
                    r.agreement,
                    r.disagreements,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("Alerts", "binary", 1, Some(1.0), 0),
                ("Alerts", "noul", 1, Some(1.0), 0),
                ("Sort", "menu", 1, Some(1.0), 0),
            ]
        );
        assert!(reports.iter().all(|r| r.calibration.is_none()));
    }

    #[test]
    fn multiple_match_targets_group_under_one_multi_framing() {
        let db = seeded_multi();
        shadow_batch(&db, &FirstOption::default(), 10).unwrap();
        let reports = rule_reports(&db, None, &Calibration::default()).unwrap();
        let sort: Vec<_> = reports
            .iter()
            .filter(|r| r.rule_name == "Sort")
            .map(|r| (r.framing.as_str(), r.decisions, r.agreement))
            .collect();
        assert_eq!(sort, [("multi", 2, Some(0.5))]);
    }

    /// A ruling is about one label question, so the other label the step fanned
    /// out to is neither counted nor answered by it.
    #[test]
    fn a_ruling_about_one_label_leaves_the_step_other_labels_alone() {
        let db = seeded_multi();
        shadow_batch(&db, &FirstOption::default(), 10).unwrap();
        let work = disagreement(&db);
        db.with_verdicts(|r| r.set_adjudication(work.id, Some(Adjudication::Rverdict)))
            .unwrap();

        let sort = report(&db, "Sort");
        assert_eq!(sort.decisions, 2);
        assert_eq!(sort.disagreements, 1);
        assert_eq!(sort.adjudicated, 1);
        assert_eq!(sort.adjudications_rverdict_right, 1);
        assert_eq!(sort.rverdict_wins, Some(1.0));
    }

    /// Re-ruling replaces the earlier statement rather than leaving both on the
    /// row for the report to weigh.
    #[test]
    fn a_second_ruling_replaces_the_first() {
        let db = seeded_multi();
        shadow_batch(&db, &FirstOption::default(), 10).unwrap();
        let work = disagreement(&db);
        db.with_verdicts(|r| r.set_adjudication(work.id, Some(Adjudication::Rverdict)))
            .unwrap();
        db.with_verdicts(|r| r.set_adjudication(work.id, Some(Adjudication::Neither)))
            .unwrap();

        let sort = report(&db, "Sort");
        assert_eq!(sort.adjudications_rverdict_right, 0);
        assert_eq!(sort.adjudications_both_wrong, 1);
        // Nobody won, so there is no share to report.
        assert_eq!(sort.rverdict_wins, None);
    }

    /// Clearing a ruling takes the decision back out of the report.
    #[test]
    fn a_ruling_can_be_cleared() {
        let db = seeded_multi();
        shadow_batch(&db, &FirstOption::default(), 10).unwrap();
        let work = disagreement(&db);
        db.with_verdicts(|r| r.set_adjudication(work.id, Some(Adjudication::Llm)))
            .unwrap();
        db.with_verdicts(|r| r.set_adjudication(work.id, None))
            .unwrap();

        let sort = report(&db, "Sort");
        assert_eq!(sort.adjudicated, 0);
        assert_eq!(sort.adjudications_llm_right, 0);
    }

    /// The option a model picked is readable from a row, and where the two
    /// answers agree the two indices must be the same.
    #[test]
    fn verdict_option_matches_the_llm_option_where_they_agree() {
        let db = seeded();
        shadow_batch(&db, &FirstOption::default(), 10).unwrap();
        for row in db.with_verdicts(|r| r.all(None)).unwrap() {
            let option = verdict_option(&row).expect("an option");
            if row.agrees == Some(true) {
                assert_eq!(
                    Some(i64::try_from(option).unwrap()),
                    row.llm_option,
                    "{row:?}"
                );
            }
        }
    }

    /// A fit that scored the model's own output back at itself would flatter it,
    /// and one that chased a label the user rejected would be chasing noise, so
    /// ruled-on rows are held out of the calibration comparison entirely.
    #[test]
    fn ruled_on_rows_are_left_out_of_the_calibration_fit() {
        let db = seeded();
        shadow_batch(&db, &FirstOption::default(), 10).unwrap();
        bulk(&db, 60);
        assert!(report(&db, BULK).calibration.is_some());

        for row in db
            .with_verdicts(|r| r.all(None))
            .unwrap()
            .iter()
            .filter(|r| r.rule_name == BULK)
        {
            db.with_verdicts(|r| r.set_adjudication(row.id, Some(Adjudication::Llm)))
                .unwrap();
        }
        assert!(report(&db, BULK).calibration.is_none());
    }

    const BULK: &str = "Bulk";

    /// Gives one rule enough well-formed history for [`MIN_FOR_CALIBRATION`]
    /// to report on, without the shadows of the other rules muddying the count.
    fn bulk(db: &Database, count: usize) {
        let kind = serde_json::to_string(&RenderedKind::Noul { explicit: true }).unwrap();
        for index in 0..count {
            let step = 100 + i64::try_from(index).unwrap();
            let yes = index % 2 == 0;
            let logits = Logits {
                logits: if yes {
                    vec![2.0, -1.0]
                } else {
                    vec![-1.0, 2.0]
                },
                null_logits: None,
                state_tokens: 12,
            };
            db.with_conn(|conn| {
                conn.execute(
                    "INSERT INTO workflow_steps (id, run_id, rule_index, rule_legacy_id, rule_name, outcome)
                     VALUES (?1, 1, ?2, 9, ?3, 'no_match')",
                    params![step, 9 + step, BULK],
                )
            })
            .unwrap();
            let new = NewVerdict {
                step_id: step,
                run_id: 1,
                account_email: "a@x",
                rule_index: 9,
                rule_legacy_id: 9,
                rule_name: BULK,
                framing: "noul",
                model: "stub@1",
                status: "ok",
                kind_json: Some(kind.clone()),
                logits_json: Some(serde_json::to_string(&logits).unwrap()),
                probabilities_json: Some(if yes { "[0.9,0.1]" } else { "[0.2,0.8]" }.into()),
                llm_option: Some(i64::from(yes)),
                agrees: Some(true),
                confidence: Some(0.85),
                ..NewVerdict::default()
            };
            db.with_verdicts(|r| r.insert(&new)).unwrap();
        }
    }
}
