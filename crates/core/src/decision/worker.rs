//! Shadows LLM decisions: asks the model about steps it has not seen yet
//! and records its answers beside the LLM's.

use std::collections::HashMap;
use std::time::Instant;

use crate::db::label_qualifications::LabelQualification;
use crate::db::verdicts::{NewVerdict, PendingStep};
use crate::db::Database;
use crate::gmail::models::{Label, Message};
use crate::rules::email_view::EmailView;
use crate::rules::engine::choice_catalog;
use crate::workflow::snapshot_rule;

use super::questions::{llm_answer, plan, verdict};
use super::DecisionModel;

/// Shadows up to `limit` pending steps, newest first. Returns how many were
/// handled; 0 means nothing is pending. The database is not held while the
/// model runs.
pub fn shadow_batch(
    db: &Database,
    model: &dyn DecisionModel,
    limit: usize,
) -> rusqlite::Result<usize> {
    let info = model.info().clone();
    let pending = db.with_verdicts(|repo| repo.pending_steps(&info.id, limit))?;
    let mut labels: HashMap<String, Vec<Label>> = HashMap::new();
    let mut qualifications: HashMap<String, Vec<LabelQualification>> = HashMap::new();
    for step in &pending {
        let labels = match labels.get(&step.account_email) {
            Some(labels) => labels,
            None => {
                let cached = db
                    .with_labels(|repo| repo.get(&step.account_email))?
                    .map(|c| c.labels)
                    .unwrap_or_default();
                labels.entry(step.account_email.clone()).or_insert(cached)
            }
        };
        let qualifications = match qualifications.get(&step.account_email) {
            Some(qualifications) => qualifications,
            None => {
                let cached = db
                    .with_label_qualifications(|repo| repo.list_for_account(&step.account_email))?;
                qualifications
                    .entry(step.account_email.clone())
                    .or_insert(cached)
            }
        };
        shadow_step(db, model, step, labels, qualifications)?;
    }
    Ok(pending.len())
}

fn shadow_step(
    db: &Database,
    model: &dyn DecisionModel,
    step: &PendingStep,
    labels: &[Label],
    qualifications: &[LabelQualification],
) -> rusqlite::Result<()> {
    let info = model.info();
    let base = NewVerdict {
        step_id: step.step_id,
        run_id: step.run_id,
        account_email: &step.account_email,
        rule_index: step.rule_index,
        rule_legacy_id: step.rule_legacy_id,
        rule_name: &step.rule_name,
        framing: "-",
        model: &info.id,
        backend: Some(&info.backend),
        precision: Some(&info.precision),
        status: "skipped",
        ..NewVerdict::default()
    };
    let skip = |detail: &str| {
        db.with_verdicts(|repo| {
            repo.insert(&NewVerdict {
                detail: Some(detail),
                ..base.clone()
            })
        })
    };

    let Some((rule, memories)) = usize::try_from(step.rule_index)
        .ok()
        .and_then(|index| snapshot_rule(&step.rules_json, index))
    else {
        return skip("the rule is missing from its run's snapshot");
    };
    let Some(message) = step
        .message_json
        .as_deref()
        .and_then(|json| serde_json::from_str::<Message>(json).ok())
    else {
        return skip("the message snapshot is missing or unreadable");
    };
    let view = EmailView::from_message(&message);
    let menu = choice_catalog(&rule, labels);
    let Some(plan) = plan(&rule, &menu, &view, &memories, qualifications) else {
        return skip("the rule makes no model decision");
    };
    let llm = llm_answer(&step.outcome, step.reply.as_deref(), &menu);

    let started = Instant::now();
    let evaluation = match model.evaluate(&plan.request) {
        Ok(evaluation) => evaluation,
        Err(error) => {
            return db.with_verdicts(|repo| {
                repo.insert(&NewVerdict {
                    status: "error",
                    detail: Some(&error),
                    ..base.clone()
                })
            })
        }
    };
    let elapsed = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
    let share = elapsed / i64::try_from(plan.asked.len().max(1)).unwrap_or(1);

    for asked in &plan.asked {
        let Some(answer) = evaluation.answers.iter().find(|a| a.id == asked.id) else {
            continue;
        };
        let v = verdict(
            asked,
            &answer.kind,
            &answer.logits,
            model.calibration(),
            llm.as_ref(),
        );
        db.with_verdicts(|repo| {
            repo.insert(&NewVerdict {
                framing: &asked.id,
                status: "ok",
                question_json: Some(asked.question.to_string()),
                options_json: serde_json::to_string(&asked.options).ok(),
                kind_json: serde_json::to_string(&answer.kind).ok(),
                logits_json: serde_json::to_string(&answer.logits).ok(),
                probabilities_json: serde_json::to_string(&v.probabilities).ok(),
                verdict_matched: Some(v.matched),
                verdict_choice: v.choice.as_deref(),
                confidence: Some(v.confidence),
                llm_matched: llm.as_ref().map(|l| l.matched),
                llm_choices_json: llm
                    .as_ref()
                    .and_then(|l| serde_json::to_string(&l.chosen).ok()),
                llm_option: v.llm_option.and_then(|o| i64::try_from(o).ok()),
                agrees: v.agrees,
                state_tokens: i64::try_from(answer.logits.state_tokens).ok(),
                truncated: Some(evaluation.truncated),
                duration_ms: Some(share),
                ..base.clone()
            })
        })?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::Path;

    use rverdict_core::{Calibration, Logits, Request};
    use serde_json::json;

    use super::*;
    use crate::decision::{Answered, Evaluation, ModelInfo};

    /// Always prefers the first option.
    pub(crate) struct FirstOption {
        pub info: ModelInfo,
        pub calibration: Calibration,
    }

    impl Default for FirstOption {
        fn default() -> Self {
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

    impl DecisionModel for FirstOption {
        fn info(&self) -> &ModelInfo {
            &self.info
        }

        fn calibration(&self) -> &Calibration {
            &self.calibration
        }

        fn evaluate(&self, request: &Request) -> Result<Evaluation, String> {
            let questions = request.parse_questions().map_err(|e| e.to_string())?;
            let answers = questions
                .into_iter()
                .map(|(id, question)| {
                    let rendered = rverdict_core::render(&question);
                    let mut logits = vec![0.0; rendered.options.len()];
                    logits[0] = 2.0;
                    Answered {
                        id,
                        kind: rendered.kind,
                        logits: Logits {
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

    /// A database with one account, two labels, and two LLM-decided steps:
    /// an instruction-only rule (matched) and a menu rule ("Financial").
    pub(crate) fn seeded() -> Database {
        let db = Database::open(Path::new(":memory:")).unwrap();
        db.migrate().unwrap();
        let rules = json!({
            "rules": [
                {"rule": {"id": 1, "name": "Alerts", "description": null, "conditions": [],
                          "prompt": "Is this a MongoDB alert?", "actions": [{"type": "archive"}],
                          "priority": 0, "enabled": true, "parent_id": null}, "memories": []},
                {"rule": {"id": 2, "name": "Sort", "description": null, "conditions": [],
                          "prompt": "File it.", "choices": [{"type": "label", "value": "Financial"},
                          {"type": "label", "value": "Work"}], "actions": [],
                          "priority": 1, "enabled": true, "parent_id": null}, "memories": ["Bills are Financial"]}
            ],
            "llm": {"base_url": "", "default_model": "", "temperature": 0.0, "max_tokens": 1, "timeout_secs": 1,
                    "context_window_tokens": 1, "legacy_max_concurrent_requests": 1,
                    "legacy_output_tokens_per_second": 0.0, "legacy_chat_reasoning_effort": "server_default",
                    "legacy_name": "", "legacy_quality_tier": "", "legacy_privacy_status": "",
                    "legacy_enabled": true, "input_cost_per_million_usd": 0.0,
                    "output_cost_per_million_usd": 0.0, "providers": [], "routing_policies": [],
                    "default_policy": "default"}
        });
        let message = json!({"id": "g1", "threadId": "t1", "labelIds": ["INBOX"], "snippet": "Invoice attached",
                             "historyId": "5", "payload": {"mimeType": "text/plain", "headers":
                             [{"name": "Subject", "value": "Invoice 4411"}], "body": null}});
        let labels = json!([{"id": "L1", "name": "Financial", "type": "user"},
                            {"id": "L2", "name": "Work", "type": "user"}]);
        db.with_conn(|conn| {
            conn.execute_batch(&format!(
                "INSERT INTO workflow_rule_sets (id, account_email, version, rules_json, active)
                     VALUES (1, 'a@x', 1, '{rules}', 1);
                 INSERT INTO workflow_messages (id, account_email, gmail_message_id, message_json)
                     VALUES (1, 'a@x', 'g1', '{message}');
                 INSERT INTO gmail_labels (account_email, labels, fetched_at) VALUES ('a@x', '{labels}', 'now');
                 INSERT INTO workflow_runs (id, account_email, message_id, rule_set_id, state)
                     VALUES (1, 'a@x', 1, 1, 'completed');
                 INSERT INTO workflow_steps (id, run_id, rule_index, rule_legacy_id, rule_name, outcome, decision_json)
                     VALUES (1, 1, 0, 1, 'Alerts', 'matched', 'MATCH | alert'),
                            (2, 1, 1, 2, 'Sort', 'matched', 'Financial | a bill'),
                            (3, 1, 2, 3, 'Local', 'matched', NULL);
                 INSERT INTO workflow_llm_attempts (run_id, rule_index, status) VALUES (1, 0, 'succeeded'), (1, 1, 'succeeded');
                 INSERT INTO workflow_action_plans (run_id, rule_index, add_label_ids, remove_label_ids, state)
                     VALUES (1, 1, '[\"L1\"]', '[]', 'succeeded');"
            ))
        })
        .unwrap();
        db
    }

    #[test]
    fn llm_decided_steps_are_shadowed_once_with_both_framings_for_instructions() {
        let db = seeded();
        let model = FirstOption::default();
        assert_eq!(db.with_verdicts(|r| r.progress("stub@1")).unwrap(), (2, 0));
        assert_eq!(shadow_batch(&db, &model, 10).unwrap(), 2);
        assert_eq!(shadow_batch(&db, &model, 10).unwrap(), 0);
        assert_eq!(db.with_verdicts(|r| r.progress("stub@1")).unwrap(), (0, 2));

        let rows = db.with_verdicts(|r| r.all(None)).unwrap();
        let summary: Vec<_> = rows
            .iter()
            .map(|v| (v.step_id, v.framing.as_str(), v.status.as_str(), v.agrees))
            .collect();
        assert_eq!(
            summary,
            [
                (2, "menu", "ok", Some(true)),
                (1, "noul", "ok", Some(true)),
                (1, "binary", "ok", Some(true)),
            ]
        );
        assert_eq!(rows[0].verdict_choice.as_deref(), Some("\"Financial\""));
        assert_eq!(rows[0].llm_option, Some(0));
    }

    #[test]
    fn steps_that_cannot_be_asked_are_recorded_as_skipped() {
        let db = seeded();
        db.with_conn(|conn| conn.execute("UPDATE workflow_messages SET message_json = NULL", []))
            .unwrap();
        shadow_batch(&db, &FirstOption::default(), 10).unwrap();
        let rows = db.with_verdicts(|r| r.all(None)).unwrap();
        assert!(rows
            .iter()
            .all(|v| v.status == "skipped" && v.framing == "-"));
        assert_eq!(shadow_batch(&db, &FirstOption::default(), 10).unwrap(), 0);
    }

    /// The seeded database with the menu rule flipped to multiple-match.
    pub(crate) fn seeded_multi() -> Database {
        let db = seeded();
        let rules: String = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT rules_json FROM workflow_rule_sets WHERE id = 1",
                    [],
                    |row| row.get(0),
                )
            })
            .unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&rules).unwrap();
        value["rules"][1]["rule"]["match_mode"] = json!("multiple");
        db.with_conn(|conn| {
            conn.execute(
                "UPDATE workflow_rule_sets SET rules_json = ?1 WHERE id = 1",
                [value.to_string()],
            )
        })
        .unwrap();
        db
    }

    #[test]
    fn multiple_match_rules_record_one_verdict_per_choice() {
        let db = seeded_multi();
        assert_eq!(shadow_batch(&db, &FirstOption::default(), 10).unwrap(), 2);
        let rows = db.with_verdicts(|r| r.all(None)).unwrap();
        let multi: Vec<_> = rows
            .iter()
            .filter(|v| v.step_id == 2)
            .map(|v| (v.framing.as_str(), v.agrees, v.verdict_choice.as_deref()))
            .collect();
        // rverdict says yes to both labels; the LLM chose only Financial.
        assert_eq!(
            multi,
            [
                ("multi:0", Some(true), Some("\"Financial\"")),
                ("multi:1", Some(false), Some("\"Work\"")),
            ]
        );
    }
}
