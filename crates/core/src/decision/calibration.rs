//! Phase 5 rung 1: per-rule calibration, *applied*.
//!
//! The shadow report only held the refit up as a what-if. This job does that
//! fit for real — one temperature per (rule, framing family) fitted against
//! the LLM's answers (the gold: user-confirmed ~99.9% accurate), split
//! half/half with NLL judged on the held-out half — and persists it so the
//! live gate uses it. Pure-Go detail: user-ruled rows stay out, so the fit
//! never chases a label the user called wrong (same rule as the report).

use std::collections::HashMap;

use rverdict_core::Calibration;
use rverdict_eval::calibrate::{self, Form};

use crate::db::Database;

use super::live::PerRuleParams;
use super::questions::framing_group;
use super::report::captures;

#[derive(Debug, Clone, Serialize)]
pub struct RefitSummary {
    pub fitted: usize,
    pub skipped: usize,
    pub rows_judged: usize,
}

use serde::Serialize;

const MIN_FOR_FIT: usize = 24;

/// Refits calibration for every (account, rule, framing, model) group that
/// has enough shadow history, and persists it. Returns per-group counts.
pub fn refit(db: &Database, model: Option<&str>) -> Result<RefitSummary, rusqlite::Error> {
    let rows = db.with_verdicts(|repo| repo.all(model))?;
    let feedback = db.with_verdicts(|repo| repo.feedback_for_steps())?;
    let ruled: std::collections::HashSet<i64> =
        feedback.iter().filter_map(|f| f.verdict_id).collect();

    let mut groups: HashMap<(String, i64, String, String), Vec<&crate::db::verdicts::VerdictRow>> =
        HashMap::new();
    for row in rows
        .iter()
        .filter(|r| r.status == "ok" && r.origin == "shadow")
        .filter(|r| !ruled.contains(&r.id))
    {
        groups
            .entry((
                row.account_email.clone(),
                row.rule_legacy_id,
                framing_group(&row.framing).to_owned(),
                row.model.clone(),
            ))
            .or_default()
            .push(row);
    }

    let mut fitted = 0;
    let mut skipped = 0;
    let mut rows_judged = 0;
    for ((account, rule_id, framing, model), group) in groups {
        let caps = captures(&group);
        rows_judged += caps.len();
        if caps.len() < MIN_FOR_FIT {
            skipped += 1;
            continue;
        }
        let (train, test) = calibrate::split(&caps, 0.5);
        if train.len() < MIN_FOR_FIT / 2 || test.is_empty() {
            skipped += 1;
            continue;
        }
        let base = Calibration::default();
        let fitted_cal = calibrate::fit(&train, Form::Scalar, &base);
        let before = calibrate::metrics(&test, &base);
        let after = calibrate::metrics(&test, &fitted_cal);
        if after.nll >= before.nll {
            skipped += 1;
            continue;
        }
        let params = PerRuleParams::new(fitted_cal.scaling.temperature, Vec::new(), caps.len());
        let json = serde_json::to_string(&params).unwrap_or_default();
        db.with_verdicts(|repo| {
            repo.put_calibration(
                &account,
                rule_id,
                &framing,
                &model,
                &json,
                caps.len() as i64,
            )
        })?;
        fitted += 1;
    }
    Ok(RefitSummary {
        fitted,
        skipped,
        rows_judged,
    })
}

#[cfg(test)]
mod tests;
