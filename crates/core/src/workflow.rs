use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::config::AppConfig;
use crate::db::workflow::{ActionPlan, ClaimedRun};
use crate::db::Database;
use crate::gmail::models::{HistoryRecord, Message};
use crate::gmail::{cached_labels, GmailClient, GmailError};
use crate::llm::{
    InferenceRouter, InferenceRuntime, LlmProviderProfile, LlmRoutingPolicy, ReasoningEffort,
};
use crate::rules::actions::resolve_actions;
use crate::rules::engine::{resolve_rule, Outcome, Resolved};
use crate::rules::matcher;
use crate::rules::models::Rule;

const RETRY_DELAY_SECS: u64 = 60;
const MAX_RUNS_PER_TICK: usize = 20;

#[derive(Debug, Clone, Serialize)]
pub struct IngestionResult {
    pub from_history_id: String,
    pub to_history_id: String,
    pub queued_messages: usize,
    pub initialized: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RuleSetSnapshot {
    rules: Vec<RuleSnapshot>,
    llm: LlmSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RuleSnapshot {
    rule: Rule,
    memories: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LlmSnapshot {
    base_url: String,
    default_model: String,
    temperature: f32,
    max_tokens: u32,
    timeout_secs: u64,
    context_window_tokens: u32,
    legacy_max_concurrent_requests: u8,
    legacy_output_tokens_per_second: f64,
    legacy_chat_reasoning_effort: ReasoningEffort,
    legacy_name: String,
    legacy_quality_tier: String,
    legacy_privacy_status: String,
    legacy_enabled: bool,
    input_cost_per_million_usd: f64,
    output_cost_per_million_usd: f64,
    providers: Vec<LlmProviderProfile>,
    routing_policies: Vec<LlmRoutingPolicy>,
    default_policy: String,
}

impl LlmSnapshot {
    fn capture(config: &AppConfig) -> Self {
        Self {
            base_url: config.llm_base_url.clone(),
            default_model: config.llm_default_model.clone(),
            temperature: config.llm_temperature,
            max_tokens: config.llm_max_tokens,
            timeout_secs: config.llm_timeout_secs,
            context_window_tokens: config.llm_context_window_tokens,
            legacy_max_concurrent_requests: config.llm_legacy_max_concurrent_requests,
            legacy_output_tokens_per_second: config.llm_legacy_output_tokens_per_second,
            legacy_chat_reasoning_effort: config.llm_legacy_chat_reasoning_effort,
            legacy_name: config.llm_legacy_name.clone(),
            legacy_quality_tier: config.llm_legacy_quality_tier.clone(),
            legacy_privacy_status: config.llm_legacy_privacy_status.clone(),
            legacy_enabled: config.llm_legacy_enabled,
            input_cost_per_million_usd: config.llm_input_cost_per_million_usd,
            output_cost_per_million_usd: config.llm_output_cost_per_million_usd,
            providers: config.llm_providers.clone(),
            routing_policies: config.llm_routing_policies.clone(),
            default_policy: config.llm_default_policy.clone(),
        }
    }

    fn apply_to(&self, config: &AppConfig) -> AppConfig {
        let mut config = config.clone();
        config.llm_base_url = self.base_url.clone();
        config.llm_default_model = self.default_model.clone();
        config.llm_temperature = self.temperature;
        config.llm_max_tokens = self.max_tokens;
        config.llm_timeout_secs = self.timeout_secs;
        config.llm_context_window_tokens = self.context_window_tokens;
        config.llm_legacy_max_concurrent_requests = self.legacy_max_concurrent_requests;
        config.llm_legacy_output_tokens_per_second = self.legacy_output_tokens_per_second;
        config.llm_legacy_chat_reasoning_effort = self.legacy_chat_reasoning_effort;
        config.llm_legacy_name = self.legacy_name.clone();
        config.llm_legacy_quality_tier = self.legacy_quality_tier.clone();
        config.llm_legacy_privacy_status = self.legacy_privacy_status.clone();
        config.llm_legacy_enabled = self.legacy_enabled;
        config.llm_input_cost_per_million_usd = self.input_cost_per_million_usd;
        config.llm_output_cost_per_million_usd = self.output_cost_per_million_usd;
        config.llm_providers = self.providers.clone();
        config.llm_routing_policies = self.routing_policies.clone();
        config.llm_default_policy = self.default_policy.clone();
        config
    }
}

/// Publishes a full immutable rule-set snapshot. Existing runs retain their
/// referenced snapshot while messages that arrive after an edit use this one.
pub fn publish_rule_set(
    db: &Database,
    account_email: &str,
    config: &AppConfig,
) -> Result<(), rusqlite::Error> {
    let rules = db.with_rules(|repo| repo.list_all(account_email))?;
    let snapshots = rules
        .into_iter()
        .map(|rule| {
            let memories = db.with_rule_memory(|repo| {
                repo.list_for_rule(rule.id).map(|entries| {
                    entries
                        .into_iter()
                        .map(|entry| entry.text)
                        .collect::<Vec<_>>()
                })
            })?;
            Ok(RuleSnapshot { rule, memories })
        })
        .collect::<Result<Vec<_>, rusqlite::Error>>()?;
    let serialized = serde_json::to_string(&RuleSetSnapshot {
        rules: snapshots,
        llm: LlmSnapshot::capture(config),
    })
    .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    db.with_workflow(|repo| {
        if repo
            .active_rule_set(account_email)?
            .is_some_and(|current| current.rules_json == serialized)
        {
            Ok(())
        } else {
            repo.activate_rule_set(account_email, &serialized)
                .map(|_| ())
        }
    })
}

fn ensure_rule_set(
    db: &Database,
    account_email: &str,
    config: &AppConfig,
) -> Result<(), rusqlite::Error> {
    if db
        .with_workflow(|repo| repo.active_rule_set(account_email))?
        .is_none()
    {
        publish_rule_set(db, account_email, config)?;
    }
    Ok(())
}

/// Cursor-driven ingestion only queues `messagesAdded` events. Label events are
/// useful metadata but must never restart a completed message workflow.
pub async fn ingest_history(
    db: &Arc<Database>,
    gmail: &mut GmailClient,
    account_email: &str,
    config: &AppConfig,
) -> Result<IngestionResult, Box<dyn std::error::Error + Send + Sync>> {
    ensure_rule_set(db, account_email, config)?;
    let existing = db.with_workflow(|repo| repo.mailbox_cursor(account_email))?;
    let Some(from_history_id) = existing else {
        let profile = gmail.get_profile().await?;
        db.with_workflow(|repo| repo.initialize_mailbox(account_email, &profile.history_id))?;
        return Ok(IngestionResult {
            from_history_id: profile.history_id.clone(),
            to_history_id: profile.history_id,
            queued_messages: 0,
            initialized: true,
        });
    };

    let mut page_token = None;
    let mut to_history_id = from_history_id.clone();
    let mut message_ids = BTreeSet::new();
    loop {
        let page = match gmail
            .list_history_page(&from_history_id, page_token.as_deref())
            .await
        {
            Ok(page) => page,
            Err(GmailError::Api { code: 404, .. }) => {
                let profile = gmail.get_profile().await?;
                db.with_workflow(|repo| {
                    repo.reset_mailbox_cursor(account_email, &profile.history_id)
                })?;
                return Ok(IngestionResult {
                    from_history_id,
                    to_history_id: profile.history_id,
                    queued_messages: 0,
                    initialized: true,
                });
            }
            Err(error) => return Err(error.into()),
        };
        if let Some(history_id) = page.history_id.as_deref() {
            to_history_id = max_history_id(to_history_id, history_id);
        }
        for record in page.history.as_deref().unwrap_or_default() {
            to_history_id = max_history_id(to_history_id, &record.id);
            collect_arrivals(&mut message_ids, record);
        }
        page_token = page.next_page_token;
        if page_token.is_none() {
            break;
        }
    }

    let ids = message_ids.into_iter().collect::<Vec<_>>();
    let queued_messages =
        db.with_workflow(|repo| repo.enqueue_arrivals(account_email, &to_history_id, &ids))?;
    Ok(IngestionResult {
        from_history_id,
        to_history_id,
        queued_messages,
        initialized: false,
    })
}

fn collect_arrivals(ids: &mut BTreeSet<String>, record: &HistoryRecord) {
    for message in record.messages_added.as_deref().unwrap_or_default() {
        ids.insert(message.message.id.clone());
    }
}

fn max_history_id(current: String, candidate: &str) -> String {
    match (current.parse::<u128>(), candidate.parse::<u128>()) {
        (Ok(current), Ok(candidate)) if candidate > current => candidate.to_string(),
        _ => current,
    }
}

pub async fn process_pending_runs(
    db: &Arc<Database>,
    account_email: &str,
    gmail: &mut GmailClient,
    config: &AppConfig,
    inference_runtime: InferenceRuntime,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let mut processed = 0;
    while processed < MAX_RUNS_PER_TICK {
        let lease_token = new_lease_token();
        let Some(run) = db.with_workflow(|repo| repo.claim_next(account_email, &lease_token))?
        else {
            break;
        };
        process_run(db, gmail, config, inference_runtime.clone(), run).await?;
        processed += 1;
    }
    Ok(processed)
}

async fn process_run(
    db: &Arc<Database>,
    gmail: &mut GmailClient,
    config: &AppConfig,
    inference_runtime: InferenceRuntime,
    run: ClaimedRun,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let snapshot: RuleSetSnapshot = match serde_json::from_str(&run.rules_json) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            db.with_workflow(|repo| {
                repo.mark_needs_attention(run.id, &run.lease_token, &error.to_string())
            })?;
            return Ok(());
        }
    };
    let llm = InferenceRouter::from_config_with_runtime(
        &snapshot.llm.apply_to(config),
        inference_runtime,
    )
    .with_database(db.as_ref().clone());
    let mut message = match run.message_json.as_deref() {
        Some(json) => match serde_json::from_str(json) {
            Ok(message) => message,
            Err(error) => {
                db.with_workflow(|repo| {
                    repo.mark_needs_attention(run.id, &run.lease_token, &error.to_string())
                })?;
                return Ok(());
            }
        },
        None => match gmail.get_message(&run.gmail_message_id).await {
            Ok(message) => {
                let message_json = serde_json::to_string(&message)?;
                let labels_json = serde_json::to_string(&message.label_ids)?;
                let stored = db.with_workflow(|repo| {
                    repo.store_message_snapshot(
                        run.id,
                        &run.lease_token,
                        &message.thread_id,
                        &message_json,
                        &labels_json,
                    )
                })?;
                if !stored {
                    return Ok(());
                }
                message
            }
            Err(error) => {
                db.with_workflow(|repo| {
                    repo.retry_run(
                        run.id,
                        &run.lease_token,
                        &error.to_string(),
                        RETRY_DELAY_SECS,
                    )
                })?;
                return Ok(());
            }
        },
    };

    let labels = cached_labels(db, gmail, &run.account_email)
        .await
        .unwrap_or_default();
    let mut rule_index = run.next_rule_index;
    loop {
        let Some(rule_snapshot) = snapshot.rules.get(rule_index as usize) else {
            db.with_workflow(|repo| {
                repo.complete_run(run.id, &run.lease_token, "all rules considered")
            })?;
            return Ok(());
        };
        let rule = &rule_snapshot.rule;

        if let Some(action) = db.with_workflow(|repo| repo.action_for_rule(run.id, rule_index))? {
            let confirmed =
                apply_action_plan(db, gmail, &run, &mut message, rule, rule_index, &action).await?;
            if !confirmed || !rule.continue_after_match {
                return Ok(());
            }
            rule_index += 1;
            continue;
        }

        if !rule
            .conditions
            .iter()
            .all(|condition| matcher::evaluate(condition, &message, &message.label_ids))
        {
            let advanced = db.with_workflow(|repo| {
                repo.record_step_and_advance(
                    run.id,
                    &run.lease_token,
                    rule_index,
                    rule.id,
                    &rule.name,
                    "condition_skipped",
                    None,
                    None,
                )
            })?;
            if !advanced {
                return Ok(());
            }
            rule_index += 1;
            continue;
        }

        let started = Instant::now();
        let resolved =
            match resolve_rule(&llm, rule, &message, &rule_snapshot.memories, &labels).await {
                Ok(resolved) => resolved,
                Err(error) => {
                    let error = error.to_string();
                    db.with_workflow(|repo| {
                        repo.record_llm_attempt(
                            run.id,
                            rule_index,
                            None,
                            None,
                            "error",
                            Some(&error),
                            Some(elapsed_ms(started)),
                        )?;
                        repo.retry_run(run.id, &run.lease_token, &error, RETRY_DELAY_SECS)
                    })?;
                    return Ok(());
                }
            };
        record_decision_attempt(
            db,
            run.id,
            rule_index,
            &resolved,
            "succeeded",
            None,
            started,
        )?;

        match resolved.outcome {
            Outcome::NoMatch => {
                let advanced = db.with_workflow(|repo| {
                    repo.record_step_and_advance(
                        run.id,
                        &run.lease_token,
                        rule_index,
                        rule.id,
                        &rule.name,
                        "no_match",
                        non_empty(&resolved.llm_response),
                        None,
                    )
                })?;
                if !advanced {
                    return Ok(());
                }
                rule_index += 1;
            }
            Outcome::Unparsed => {
                let error = resolved
                    .diagnostic
                    .as_deref()
                    .unwrap_or("Invalid LLM decision");
                db.with_workflow(|repo| {
                    repo.retry_run(run.id, &run.lease_token, error, RETRY_DELAY_SECS)
                })?;
                return Ok(());
            }
            Outcome::Matched if resolved.actions.is_empty() => {
                let advanced = db.with_workflow(|repo| {
                    repo.record_step_and_advance(
                        run.id,
                        &run.lease_token,
                        rule_index,
                        rule.id,
                        &rule.name,
                        "matched_no_action",
                        non_empty(&resolved.llm_response),
                        resolved.diagnostic.as_deref(),
                    )
                })?;
                if !advanced {
                    return Ok(());
                }
                if !rule.continue_after_match {
                    db.with_workflow(|repo| {
                        repo.complete_run(
                            run.id,
                            &run.lease_token,
                            "rule claimed message without action",
                        )
                    })?;
                    return Ok(());
                }
                rule_index += 1;
            }
            Outcome::Matched => {
                let mutation = match resolve_actions(&resolved.actions, &labels) {
                    Ok(mutation) => mutation,
                    Err(error) => {
                        db.with_workflow(|repo| {
                            repo.mark_needs_attention(run.id, &run.lease_token, &error.to_string())
                        })?;
                        return Ok(());
                    }
                };
                let recorded = db.with_workflow(|repo| {
                    repo.record_step(
                        run.id,
                        &run.lease_token,
                        rule_index,
                        rule.id,
                        &rule.name,
                        "matched",
                        non_empty(&resolved.llm_response),
                        resolved.diagnostic.as_deref(),
                    )
                })?;
                if !recorded {
                    return Ok(());
                }
                let Some(action) = db.with_workflow(|repo| {
                    repo.plan_action(
                        run.id,
                        &run.lease_token,
                        rule_index,
                        &mutation.add,
                        &mutation.remove,
                    )
                })?
                else {
                    return Ok(());
                };
                let confirmed =
                    apply_action_plan(db, gmail, &run, &mut message, rule, rule_index, &action)
                        .await?;
                if !confirmed || !rule.continue_after_match {
                    return Ok(());
                }
                rule_index += 1;
            }
        }
    }
}

async fn apply_action_plan(
    db: &Arc<Database>,
    gmail: &mut GmailClient,
    run: &ClaimedRun,
    message: &mut Message,
    rule: &Rule,
    rule_index: i64,
    action: &ActionPlan,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let add_refs = action
        .add_label_ids
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let remove_refs = action
        .remove_label_ids
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let applied = match gmail
        .modify_labels(&run.gmail_message_id, &add_refs, &remove_refs)
        .await
    {
        Ok(()) => true,
        Err(error) => {
            // The request may have reached Gmail before the connection failed.
            // Reconcile first so retries converge on state rather than intent.
            match gmail.get_message(&run.gmail_message_id).await {
                Ok(remote) if target_reached(&remote, action) => {
                    *message = remote;
                    true
                }
                _ => {
                    db.with_workflow(|repo| {
                        repo.retry_action(
                            action.id,
                            run.id,
                            &run.lease_token,
                            &error.to_string(),
                            RETRY_DELAY_SECS,
                        )
                    })?;
                    return Ok(false);
                }
            }
        }
    };
    if applied {
        apply_local_labels(message, action);
        let message_json = serde_json::to_string(message)?;
        let labels_json = serde_json::to_string(&message.label_ids)?;
        return db
            .with_workflow(|repo| {
                repo.confirm_action(
                    action.id,
                    run.id,
                    &run.lease_token,
                    &message_json,
                    &labels_json,
                    rule_index + 1,
                    rule.continue_after_match,
                )
            })
            .map_err(Into::into);
    }
    Ok(false)
}

fn target_reached(message: &Message, action: &ActionPlan) -> bool {
    action
        .add_label_ids
        .iter()
        .all(|label| message.label_ids.contains(label))
        && action
            .remove_label_ids
            .iter()
            .all(|label| !message.label_ids.contains(label))
}

fn apply_local_labels(message: &mut Message, action: &ActionPlan) {
    let mut labels = message.label_ids.iter().cloned().collect::<BTreeSet<_>>();
    for label in &action.remove_label_ids {
        labels.remove(label);
    }
    labels.extend(action.add_label_ids.iter().cloned());
    message.label_ids = labels.into_iter().collect();
}

fn record_decision_attempt(
    db: &Arc<Database>,
    run_id: i64,
    rule_index: i64,
    resolved: &Resolved,
    status: &str,
    error: Option<&str>,
    started: Instant,
) -> Result<(), rusqlite::Error> {
    db.with_workflow(|repo| {
        repo.record_llm_attempt(
            run_id,
            rule_index,
            resolved.llm_provider.as_deref(),
            resolved.llm_model.as_deref(),
            status,
            error,
            Some(elapsed_ms(started)),
        )
    })
}

fn elapsed_ms(started: Instant) -> i64 {
    started.elapsed().as_millis().min(i64::MAX as u128) as i64
}

fn non_empty(value: &str) -> Option<&str> {
    (!value.trim().is_empty()).then_some(value)
}

fn new_lease_token() -> String {
    format!("{:032x}", rand::thread_rng().gen::<u128>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_message_added_events_are_ingested() {
        let record: HistoryRecord = serde_json::from_str(
            r#"{
                "id":"10",
                "messages":[{"id":"generic","threadId":"t"}],
                "messagesAdded":[{"message":{"id":"arrival","threadId":"t"}}],
                "labelsAdded":[{"message":{"id":"label-change","threadId":"t"}}]
            }"#,
        )
        .unwrap();
        let mut ids = BTreeSet::new();
        collect_arrivals(&mut ids, &record);
        assert_eq!(ids.into_iter().collect::<Vec<_>>(), ["arrival"]);
    }
}
