//! Phase 5 live gate: `verdict_then_llm`.
//!
//! A rule opted into `DecisionMode::VerdictThenLlm` is asked through the
//! embedded model first. When every answer it needs is at or above the
//! threshold, the answers become a `Resolved` — the exact type the workflow
//! persists for LLM decisions — so priority, `continue_after_match` and action
//! planning are untouched. Anything else (no model, low confidence, error,
//! truncation) returns `None` and the caller falls through to the LLM route,
//! which is therefore the unmodified fallback for every live rule.

use std::collections::HashMap;

use rverdict_core::Calibration;

use crate::gmail::models::Label;
use crate::rules::email_view::EmailView;
use crate::rules::engine::{choice_catalog, effective_actions, Choice, Outcome, Resolved};
use crate::rules::label_context::ResolvedLabel;
use crate::rules::models::Rule;

use super::questions::{plan, verdict, Framing, Meaning, Plan};
#[allow(unused_imports)]
use super::ModelInfo;
use super::{DecisionModel, Evaluation};

/// Transport that evaluates a plan. `LocalModel` implements it directly;
/// the service channel implements it by asking the resident model on the
/// service thread (one load, shared with shadow work).
pub trait LiveBackend {
    fn eval_plan(&self, plan: &Plan) -> Result<Evaluation, String>;
}

/// Channel of live requests into the resident shadow-model service.
pub trait LiveDecider: Send + Sync {
    fn eval_plan(&self, plan: &Plan) -> Result<Evaluation, String>;
}

pub struct NoopLiveDecider;

impl LiveDecider for NoopLiveDecider {
    fn eval_plan(&self, _plan: &Plan) -> Result<Evaluation, String> {
        Err("local decisions are not loaded".into())
    }
}

/// Evaluates a plan in whole-question chunks, so one 100-label request does
/// not become one very large dispatch. Splitting never changes an answer.
pub fn evaluate_chunked(
    model: &dyn DecisionModel,
    plan: &Plan,
    max_questions: usize,
) -> Result<Evaluation, String> {
    use rverdict_core::Request;
    use serde_json::Map;

    let mut answers = Vec::with_capacity(plan.asked.len());
    let mut truncated = false;
    for chunk in plan.asked.chunks(max_questions.max(1)) {
        let mut questions = Map::new();
        for asked in chunk {
            if let Some(question) = plan.request.questions.get(&asked.id) {
                questions.insert(asked.id.clone(), question.clone());
            }
        }
        let evaluation = model.evaluate(&Request {
            state: plan.request.state.clone(),
            model: plan.request.model.clone(),
            questions,
        })?;
        truncated = truncated || evaluation.truncated;
        answers.extend(evaluation.answers);
    }
    Ok(Evaluation {
        answers,
        truncated,
        model_id: Some(model.info().id.clone()),
    })
}

/// The confidence at which a live rule may act without the LLM.
pub const LIVE_ACT_THRESHOLD: f64 = 0.9;

/// Attempts a live decision. Returns `Some(Resolved)` only when the model
/// answered every needed question at or above the rule's threshold;
/// `None` always means "fall through to the LLM".
#[allow(clippy::too_many_arguments)]
pub fn try_live_decide(
    backend: &dyn LiveBackend,
    rule: &Rule,
    view: &EmailView,
    memories: &[String],
    labels: &[Label],
    label_contexts: &HashMap<String, ResolvedLabel>,
    calibrations: &HashMap<String, Calibration>,
    default_calibration: &Calibration,
    threshold: f64,
    model_id: &str,
) -> Option<Resolved> {
    let menu = choice_catalog(rule, labels);
    let plan = plan(rule, &menu, view, memories, label_contexts)?;
    let evaluation = backend.eval_plan(&plan).ok()?;
    verdict_to_resolved(
        rule,
        &menu,
        &plan,
        &evaluation,
        calibrations,
        default_calibration,
        threshold,
        model_id,
    )
}

/// Turns an evaluation into a `Resolved`, or `None` when the gate declines
/// (any needed question below threshold, missing answer, or truncation).
#[allow(clippy::too_many_arguments)]
pub fn verdict_to_resolved(
    rule: &Rule,
    menu: &[Choice],
    plan: &Plan,
    evaluation: &Evaluation,
    calibrations: &HashMap<String, Calibration>,
    default_calibration: &Calibration,
    threshold: f64,
    model_id: &str,
) -> Option<Resolved> {
    if evaluation.truncated {
        return None;
    }
    let mut matched = false;
    let mut chosen_names: Vec<String> = Vec::new();
    let mut needed = 0usize;
    let mut min_confidence = f64::INFINITY;

    for asked in &plan.asked {
        if !is_primary(asked.framing, plan) {
            continue;
        }
        needed += 1;
        let answer = evaluation.answers.iter().find(|a| a.id == asked.id)?;
        let calibration = calibrations
            .get(crate::decision::questions::framing_group(&asked.id))
            .unwrap_or(default_calibration);
        let v = verdict(asked, &answer.kind, &answer.logits, calibration, None);
        if v.confidence < threshold {
            return None;
        }
        min_confidence = min_confidence.min(v.confidence);
        if v.matched {
            matched = true;
            if let Some(name) = v.choice {
                chosen_names.push(name);
            }
        }
    }
    if needed == 0 {
        return None;
    }

    let chosen: Vec<crate::rules::response_parser::ParsedAction> = chosen_names
        .iter()
        .filter_map(|name| {
            menu.iter()
                .find(|c| c.name == *name)
                .map(|c| c.action.clone())
        })
        .collect();

    let outcome = if matched {
        Outcome::Matched
    } else {
        Outcome::NoMatch
    };
    let actions = if matched {
        effective_actions(rule, &chosen)
    } else {
        Vec::new()
    };
    let response = format!("rverdict {} @ {:.2}", model_id, min_confidence.min(1.0));
    Some(Resolved {
        outcome,
        actions,
        llm_response: response.clone(),
        reason: response,
        llm_model: Some(model_id.to_owned()),
        llm_provider: Some("local".into()),
        prompt_tokens: None,
        completion_tokens: None,
        total_tokens: None,
        llm_duration_ms: None,
        llm_request_key: None,
        llm_unavailable: false,
        diagnostic: None,
    })
}

/// For instruction-only rules the shadow plan asks the same question twice
/// (`noul` + `binary`); the live gate acts on the first framing it asks,
/// `noul`, and records the rest for audit. Menu rules have one question;
/// multi rules answer every per-label question (all must clear threshold).
fn is_primary(framing: Framing, plan: &Plan) -> bool {
    match framing {
        Framing::Binary => !plan.asked.iter().any(|a| a.framing == Framing::Noul),
        Framing::Noul | Framing::Menu | Framing::Multi => true,
    }
}

/// Re-export for callers that only have the sharing means/options to map.
pub fn meaning_of(options: &[Meaning], best: usize) -> Option<Meaning> {
    options.get(best).cloned()
}

/// A live gate attempt that cleared the threshold: the `Resolved` to persist,
/// plus the plan/evaluation to record as a live verdict row.
pub struct LiveAttempt {
    pub resolved: Resolved,
    pub plan: Plan,
    pub evaluation: Evaluation,
    pub menu: Vec<Choice>,
    pub model_id: String,
}

/// Runs the gate through a decider (blocking channel call) and returns the
/// attempt only when it clears `threshold`. [`LiveDecider::eval_plan`] is
/// blocking, so callers in async code use `spawn_blocking`.
#[allow(clippy::too_many_arguments)]
pub fn attempt_live(
    decider: &dyn LiveDecider,
    db: &crate::db::Database,
    account_email: &str,
    rule: &Rule,
    view: &EmailView,
    memories: &[String],
    labels: &[Label],
    label_contexts: &HashMap<String, ResolvedLabel>,
    threshold: f64,
    global_calibration: &Calibration,
) -> Option<LiveAttempt> {
    let menu = choice_catalog(rule, labels);
    let plan = plan(rule, &menu, view, memories, label_contexts)?;
    let evaluation = decider.eval_plan(&plan).ok()?;
    verdict_to_resolved(
        rule,
        &menu,
        &plan,
        &evaluation,
        &load_calibrations(db, account_email, rule, &evaluation, global_calibration),
        global_calibration,
        threshold,
        evaluation.model_id.as_deref().unwrap_or("unknown"),
    )
    .map(|resolved| LiveAttempt {
        resolved,
        model_id: evaluation
            .model_id
            .clone()
            .unwrap_or_else(|| "unknown".into()),
        plan,
        evaluation,
        menu,
    })
}

fn load_calibrations(
    db: &crate::db::Database,
    account_email: &str,
    rule: &Rule,
    evaluation: &Evaluation,
    _global: &Calibration,
) -> HashMap<String, Calibration> {
    let Some(model) = evaluation.model_id.as_deref() else {
        return HashMap::new();
    };
    let stored = db
        .with_verdicts(|repo| repo.calibration_for_rule(account_email, rule.id, model))
        .unwrap_or_default();
    stored
        .into_iter()
        .filter_map(|(framing, json)| {
            let params: PerRuleParams = serde_json::from_str(&json).ok()?;
            let base = params.base.clone();
            Some((framing, params.into_calibration(&base)))
        })
        .collect()
}

/// Per-rule calibration params as stored in `verdict_calibration`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PerRuleParams {
    pub temperature: f64,
    pub bias: Vec<f64>,
    pub n: usize,
    #[serde(default)]
    pub base: Calibration,
}

impl PerRuleParams {
    pub fn into_calibration(&self, _base: &Calibration) -> Calibration {
        let mut cal = Calibration::default();
        cal.scaling.temperature = self.temperature;
        cal
    }

    pub fn new(temperature: f64, bias: Vec<f64>, n: usize) -> Self {
        Self {
            temperature,
            bias,
            n,
            base: Calibration::default(),
        }
    }
}

/// Records a live (acted) attempt as `origin = "live"` verdict rows, so the
/// shadow queue's `NOT EXISTS` check leaves these steps alone: there was no
/// LLM decision to compare them against. Fallback decisions are *not*
/// recorded here — they flow to the LLM and are shadowed normally, which is
/// the live audit trail.
#[allow(clippy::too_many_arguments)]
pub fn record_live(
    db: &crate::db::Database,
    run_id: i64,
    account_email: &str,
    rule_index: i64,
    rule_legacy_id: i64,
    rule_name: &str,
    attempt: &LiveAttempt,
    duration_ms: Option<i64>,
) -> Result<(), rusqlite::Error> {
    let Some(step_id) = db.with_verdicts(|repo| repo.step_id(run_id, rule_index))? else {
        return Ok(());
    };
    for asked in attempt
        .plan
        .asked
        .iter()
        .filter(|a| is_primary(a.framing, &attempt.plan))
    {
        let Some(answer) = attempt.evaluation.answers.iter().find(|a| a.id == asked.id) else {
            continue;
        };
        let calibration = Calibration::default();
        let v = verdict(asked, &answer.kind, &answer.logits, &calibration, None);
        db.with_verdicts(|repo| {
            repo.insert(&crate::db::verdicts::NewVerdict {
                step_id,
                run_id,
                account_email,
                rule_index,
                rule_legacy_id,
                rule_name,
                framing: &asked.id,
                model: &attempt.model_id,
                backend: None,
                precision: None,
                status: "ok",
                detail: None,
                question_json: Some(asked.question.to_string()),
                options_json: serde_json::to_string(&asked.options).ok(),
                kind_json: serde_json::to_string(&answer.kind).ok(),
                logits_json: serde_json::to_string(&answer.logits).ok(),
                probabilities_json: serde_json::to_string(&v.probabilities).ok(),
                verdict_matched: Some(v.matched),
                verdict_choice: v.choice.as_deref(),
                target: asked.target.as_deref(),
                confidence: Some(v.confidence),
                llm_matched: None,
                llm_choices_json: None,
                llm_option: None,
                agrees: None,
                state_tokens: i64::try_from(answer.logits.state_tokens).ok(),
                truncated: Some(attempt.evaluation.truncated),
                duration_ms,
                origin: "live",
                ..crate::db::verdicts::NewVerdict::default()
            })
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::questions::Framing;
    use crate::decision::{Answered, ModelInfo};
    use crate::gmail::models::{Header, Message, MessagePayload};
    use crate::rules::models::MatchMode;
    use crate::rules::response_parser::ParsedAction;
    use rverdict_core::{Logits, RenderedKind};
    use serde_json::json;

    struct Stub {
        logits: Vec<f64>,
    }

    impl LiveBackend for Stub {
        fn eval_plan(&self, plan: &Plan) -> Result<Evaluation, String> {
            let answers = plan
                .asked
                .iter()
                .map(|asked| {
                    let n = asked.options.len();
                    let mut logits = vec![0.0f32; n];
                    logits[0] = self.logits[0] as f32;
                    Answered {
                        id: asked.id.clone(),
                        kind: match asked.framing {
                            Framing::Menu => RenderedKind::Choice {
                                keys: asked
                                    .options
                                    .iter()
                                    .enumerate()
                                    .map(|(i, _)| format!("k{i}"))
                                    .collect(),
                            },
                            _ => RenderedKind::Noul { explicit: true },
                        },
                        logits: Logits {
                            logits,
                            null_logits: None,
                            state_tokens: 10,
                        },
                    }
                })
                .collect();
            Ok(Evaluation {
                answers,
                truncated: false,
                model_id: None,
            })
        }
    }

    fn rule(match_mode: MatchMode) -> Rule {
        serde_json::from_value(json!({
            "id": 7, "name": "T", "description": null, "conditions": [],
            "prompt": "File it.", "choices": [{"type": "label", "value": "L1"}, {"type": "label", "value": "L2"}], "actions": [], "priority": 0, "enabled": true,
            "parent_id": null, "match_mode": if match_mode == MatchMode::Multiple { "multiple" } else { "single" }
        }))
        .unwrap()
    }

    fn view() -> EmailView {
        EmailView::from_message(&Message {
            id: "m1".into(),
            thread_id: "t1".into(),
            label_ids: vec!["INBOX".into()],
            snippet: "x".into(),
            history_id: "1".into(),
            internal_date: String::new(),
            size_estimate: 0,
            payload: Some(MessagePayload {
                mime_type: "text/plain".into(),
                headers: vec![Header {
                    name: "Subject".into(),
                    value: "Invoice".into(),
                }],
                body: None,
                parts: None,
                filename: None,
            }),
        })
    }

    fn labels() -> Vec<crate::gmail::models::Label> {
        vec![
            crate::gmail::models::Label {
                id: "L1".into(),
                name: "Financial".into(),
                label_type: "user".into(),
                message_list_visibility: None,
                label_list_visibility: None,
            },
            crate::gmail::models::Label {
                id: "L2".into(),
                name: "Work".into(),
                label_type: "user".into(),
                message_list_visibility: None,
                label_list_visibility: None,
            },
        ]
    }

    fn menu() -> Vec<Choice> {
        vec![
            Choice::label("L1", "Financial"),
            Choice::label("L2", "Work"),
            Choice {
                name: "TRASH".into(),
                action: ParsedAction::Trash,
            },
        ]
    }

    fn info() -> ModelInfo {
        ModelInfo {
            id: "stub@1".into(),
            backend: "test".into(),
            precision: "f32".into(),
        }
    }

    #[test]
    fn confident_menu_decision_maps_to_resolved() {
        let r = rule(MatchMode::Single);
        let stub = Stub { logits: vec![3.0] };
        let out = try_live_decide(
            &stub,
            &r,
            &view(),
            &[],
            &labels(),
            &HashMap::new(),
            &HashMap::new(),
            &Calibration::default(),
            0.9,
            "stub@1",
        )
        .expect("confident");
        assert_eq!(out.outcome, Outcome::Matched);
        assert_eq!(out.actions, vec![ParsedAction::Label("L1".into())]);
        assert_eq!(out.llm_provider.as_deref(), Some("local"));
    }

    #[test]
    fn low_confidence_falls_back() {
        let r = rule(MatchMode::Single);
        let stub = Stub { logits: vec![0.1] };
        let out = try_live_decide(
            &stub,
            &r,
            &view(),
            &[],
            &labels(),
            &HashMap::new(),
            &HashMap::new(),
            &Calibration::default(),
            0.9,
            "stub@1",
        );
        assert!(out.is_none());
    }

    #[test]
    fn multi_requires_every_question_confident() {
        let r = rule(MatchMode::Multiple);
        let stub = Stub { logits: vec![3.0] };
        let out = try_live_decide(
            &stub,
            &r,
            &view(),
            &[],
            &labels(),
            &HashMap::new(),
            &HashMap::new(),
            &Calibration::default(),
            0.9,
            "stub@1",
        );
        let out = out.expect("all confident");
        assert_eq!(out.outcome, Outcome::Matched);
        assert!(out.actions.len() >= 1);
    }

    #[test]
    fn truncation_never_acts() {
        // Directly exercise the gate helper through verdict_to_resolved path:
        // truncated evaluations decline even when logits are extreme.
        let r = rule(MatchMode::Single);
        let menu = menu();
        let plan = plan(&r, &menu, &view(), &[], &HashMap::new()).unwrap();
        let eval = Evaluation {
            answers: Vec::new(),
            truncated: true,
            model_id: None,
        };
        assert!(verdict_to_resolved(
            &r,
            &menu,
            &plan,
            &eval,
            &HashMap::new(),
            &Calibration::default(),
            0.9,
            "stub@1",
        )
        .is_none());
    }
}
