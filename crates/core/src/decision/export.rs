//! Exports shadowed decisions as rverdict training and calibration data:
//! `decisions.jsonl` (labelled decisions, readable by `rverdict calibrate`
//! and training) and `decisions.captures.jsonl` (the model's raw output, so
//! calibration can be refit without running it again).
//!
//! Labels are the LLM's answers (`source: "llm"`), overridden by the user's
//! where feedback says what the answer should have been (`source: "user"`).
//! The files contain email text: keep them local.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use rverdict_core::RenderedKind;
use serde::Serialize;
use serde_json::{json, Value};

use crate::db::verdicts::{Feedback, VerdictRow};
use crate::db::Database;
use crate::gmail::models::{Label, Message};
use crate::rules::email_view::EmailView;

use super::questions::{framing_group, state_text, Meaning};
use super::report::{captures, is_negative};

#[derive(Debug, Clone, Serialize)]
pub struct ExportSummary {
    pub decisions: usize,
    pub from_user: usize,
    pub captures: usize,
    pub decisions_path: PathBuf,
    pub captures_path: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error(transparent)]
    Database(#[from] rusqlite::Error),
    #[error("writing {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// The label the user's feedback gives a decision, if it says one.
fn user_option(
    row: &VerdictRow,
    meanings: &[Meaning],
    feedback: &[&Feedback],
    label_names: &HashMap<String, String>,
) -> Option<usize> {
    if feedback.iter().any(|f| f.kind == "thumbs_up") {
        return row.llm_option.and_then(|o| usize::try_from(o).ok());
    }
    // A label the user added that is on the menu is the right answer.
    for f in feedback.iter().filter(|f| f.kind == "label_added") {
        let Some(name) = label_names.get(&f.label_id) else {
            continue;
        };
        let wanted = format!("\"{name}\"");
        if let Some(i) = meanings
            .iter()
            .position(|m| *m == Meaning::Choice(wanted.clone()))
        {
            return Some(i);
        }
    }
    // Undoing a yes/no rule's action says it should not have applied.
    if feedback.iter().any(|f| is_negative(&f.kind))
        && row.llm_matched == Some(true)
        && row.framing != "menu"
    {
        return meanings.iter().position(|m| *m == Meaning::DoesNotApply);
    }
    None
}

fn expected_label(kind: &RenderedKind, option: usize) -> Option<Value> {
    match kind {
        RenderedKind::Noul { .. } => Some(json!(if option == 0 { "yes" } else { "no" })),
        RenderedKind::Choice { keys } => keys.get(option).map(|k| json!(k)),
        RenderedKind::Score => Some(json!(option)),
    }
}

pub fn export(
    db: &Database,
    dir: &Path,
    model: Option<&str>,
) -> Result<ExportSummary, ExportError> {
    let io = |path: &Path| {
        let path = path.to_owned();
        move |source| ExportError::Io { path, source }
    };
    std::fs::create_dir_all(dir).map_err(io(dir))?;
    let decisions_path = dir.join("decisions.jsonl");
    let captures_path = dir.join("decisions.captures.jsonl");

    let rows: Vec<VerdictRow> = db
        .with_verdicts(|repo| repo.all(model))?
        .into_iter()
        .filter(|r| r.status == "ok")
        .collect();
    let feedback = db.with_verdicts(|repo| repo.feedback_for_steps())?;
    let mut by_step: HashMap<i64, Vec<&Feedback>> = HashMap::new();
    for f in &feedback {
        if let Some(step) = f.step_id {
            by_step.entry(step).or_default().push(f);
        }
    }
    let mut label_names: HashMap<String, String> = HashMap::new();
    for account in rows
        .iter()
        .map(|r| r.account_email.clone())
        .collect::<std::collections::BTreeSet<_>>()
    {
        let labels: Vec<Label> = db
            .with_labels(|repo| repo.get(&account))?
            .map(|c| c.labels)
            .unwrap_or_default();
        label_names.extend(labels.into_iter().map(|l| (l.id, l.name)));
    }

    let mut decisions = std::io::BufWriter::new(
        std::fs::File::create(&decisions_path).map_err(io(&decisions_path))?,
    );
    let mut summary = ExportSummary {
        decisions: 0,
        from_user: 0,
        captures: 0,
        decisions_path: decisions_path.clone(),
        captures_path: captures_path.clone(),
    };
    let mut relabelled: Vec<VerdictRow> = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(kind) = row
            .kind_json
            .as_deref()
            .and_then(|k| serde_json::from_str::<RenderedKind>(k).ok())
        else {
            continue;
        };
        let meanings: Vec<Meaning> = row
            .options_json
            .as_deref()
            .and_then(|o| serde_json::from_str(o).ok())
            .unwrap_or_default();
        let empty = Vec::new();
        let feedback = by_step.get(&row.step_id).unwrap_or(&empty);
        let user = user_option(&row, &meanings, feedback, &label_names);
        let (option, source) = match (user, row.llm_option) {
            (Some(option), _) => (option, "user"),
            (None, Some(option)) => (usize::try_from(option).unwrap_or(usize::MAX), "llm"),
            (None, None) => continue,
        };
        let Some(expected) = expected_label(&kind, option) else {
            continue;
        };
        let Some((Some(message), Some(question))) =
            db.with_verdicts(|repo| repo.export_inputs(row.id))?
        else {
            continue;
        };
        let Ok(message) = serde_json::from_str::<Message>(&message) else {
            continue;
        };
        let line = json!({
            "id": format!("{}-{}", row.step_id, row.framing),
            "subset": framing_group(&row.framing),
            "family": format!("rule {}: {}", row.rule_legacy_id, row.rule_name),
            "state": state_text(&EmailView::from_message(&message)),
            "question": serde_json::from_str::<Value>(&question).unwrap_or(Value::Null),
            "expected": expected,
            "source": source,
        });
        writeln!(decisions, "{line}").map_err(io(&decisions_path))?;
        summary.decisions += 1;
        if source == "user" {
            summary.from_user += 1;
        }
        relabelled.push(VerdictRow {
            llm_option: i64::try_from(option).ok(),
            ..row
        });
    }
    decisions.flush().map_err(io(&decisions_path))?;

    let mut out =
        std::io::BufWriter::new(std::fs::File::create(&captures_path).map_err(io(&captures_path))?);
    let refs: Vec<&VerdictRow> = relabelled.iter().collect();
    for capture in captures(&refs) {
        let line = serde_json::to_string(&capture).unwrap_or_default();
        writeln!(out, "{line}").map_err(io(&captures_path))?;
        summary.captures += 1;
    }
    out.flush().map_err(io(&captures_path))?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::worker::shadow_batch;
    use crate::decision::worker::tests::{seeded, FirstOption};

    #[test]
    fn exports_llm_labels_and_user_corrections() {
        let db = seeded();
        shadow_batch(&db, &FirstOption::default(), 10).unwrap();
        // The user moved the "Sort" email from Financial to Work.
        db.with_verdicts(|r| {
            r.record_feedback(
                "a@x",
                1,
                Some(1),
                Some(2),
                Some(1),
                "label_added",
                "L2",
                "9",
            )
        })
        .unwrap();
        let dir = std::env::temp_dir().join(format!("po-export-{}", std::process::id()));
        let summary = export(&db, &dir, None).unwrap();
        assert_eq!(
            (summary.decisions, summary.from_user, summary.captures),
            (3, 1, 3)
        );

        let lines: Vec<Value> = std::fs::read_to_string(&summary.decisions_path)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let sort = lines.iter().find(|l| l["subset"] == "menu").unwrap();
        assert_eq!(
            (&sort["expected"], &sort["source"]),
            (&json!("c1"), &json!("user"))
        );
        let noul = lines.iter().find(|l| l["subset"] == "noul").unwrap();
        assert_eq!(noul["expected"], json!("yes"));
        assert!(noul["state"]
            .as_str()
            .unwrap()
            .starts_with("Subject: Invoice 4411"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
