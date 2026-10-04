//! Per-rule shadow reports: how often rverdict agrees with the LLM, how
//! often it is confidently wrong, what user feedback says, and whether a
//! calibration refit on the rule's own history would help.

use std::collections::{BTreeMap, HashMap};

use rverdict_core::{Calibration, Logits, RenderedKind};
use rverdict_eval::calibrate::{self, Capture, Form};
use serde::Serialize;

use crate::db::verdicts::{Feedback, VerdictRow};
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
            | "thumbs_down"
    )
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
    let mut by_step: HashMap<i64, Vec<&Feedback>> = HashMap::new();
    for f in feedback {
        if let Some(step) = f.step_id {
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
            let mut report = summarize(&rows, &by_step, calibration);
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
    calibration: &Calibration,
) -> RuleReport {
    let compared: Vec<&&VerdictRow> = rows.iter().filter(|r| r.agrees.is_some()).collect();
    let agreed = compared.iter().filter(|r| r.agrees == Some(true)).count();

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

    let mut feedback_n = 0;
    let mut llm_wrong = 0;
    let mut verdict_disagreed = 0;
    for row in &compared {
        let Some(entries) = feedback.get(&row.step_id) else {
            continue;
        };
        feedback_n += 1;
        if entries.iter().any(|f| is_negative(&f.kind)) {
            llm_wrong += 1;
            if row.agrees == Some(false) {
                verdict_disagreed += 1;
            }
        }
    }

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
        calibration: calibration_report(rows, calibration),
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

fn calibration_report(
    rows: &[&VerdictRow],
    calibration: &Calibration,
) -> Option<CalibrationReport> {
    let captures = captures(rows);
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
    use super::*;
    use crate::decision::worker::shadow_batch;
    use crate::decision::worker::tests::{seeded, seeded_multi, FirstOption};

    #[test]
    fn reports_agreement_per_rule_and_framing() {
        let db = seeded();
        shadow_batch(&db, &FirstOption::default(), 10).unwrap();
        db.with_verdicts(|r| r.set_rating(2, Some(false))).unwrap();
        let reports = rule_reports(&db, None, &Calibration::default()).unwrap();
        let summary: Vec<_> = reports
            .iter()
            .map(|r| {
                (
                    r.rule_name.as_str(),
                    r.framing.as_str(),
                    r.decisions,
                    r.agreement,
                    r.feedback_llm_wrong,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("Alerts", "binary", 1, Some(1.0), 0),
                ("Alerts", "noul", 1, Some(1.0), 0),
                ("Sort", "menu", 1, Some(1.0), 1),
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
}
