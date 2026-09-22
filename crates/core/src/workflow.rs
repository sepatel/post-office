use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Instant;

use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::config::AppConfig;
use crate::db::workflow::{
    ActionPlan, ClaimedRun, WorkflowActionPlan, WorkflowEvent, WorkflowLlmAttempt, WorkflowMailbox,
    WorkflowRun, WorkflowStateCount, WorkflowStep, MAX_ATTEMPTS,
};
use crate::db::Database;
use crate::gmail::models::{HistoryRecord, Message};
use crate::gmail::{cached_labels, GmailClient, GmailError};
use crate::llm::{
    InferenceRouter, InferenceRuntime, LlmProviderProfile, LlmRoutingPolicy, ReasoningEffort,
};
use crate::rules::actions::resolve_actions;
use crate::rules::engine::{needs_llm_decision, resolve_rule, Outcome, Resolved};
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

#[derive(Debug, Clone, Serialize)]
pub struct QueueItem {
    pub message_id: i64,
    pub run_id: i64,
    pub gmail_message_id: String,
    pub gmail_thread_id: Option<String>,
    pub sender: Option<String>,
    pub subject: Option<String>,
    pub preview: String,
    pub labels: Vec<String>,
    pub state: String,
    pub next_rule_index: i64,
    pub next_rule_name: Option<String>,
    pub has_message_snapshot: bool,
    pub attempt_count: i64,
    pub next_attempt_at: String,
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
    pub rule_set_version: i64,
    pub next_provider_id: Option<String>,
    pub next_endpoint: Option<String>,
    pub next_model: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct QueueSummary {
    pub counts: Vec<WorkflowStateCount>,
    pub mailbox: Option<WorkflowMailbox>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MessageDetail {
    #[serde(flatten)]
    pub message: QueueItem,
    pub body: String,
    pub current_rule: Option<WorkflowRuleContext>,
    pub retry_summary: WorkflowRetrySummary,
    pub steps: Vec<WorkflowStep>,
    pub llm_attempts: Vec<WorkflowLlmAttempt>,
    pub action_plans: Vec<WorkflowActionPlan>,
    pub events: Vec<WorkflowEvent>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowRuleContext {
    pub rule_index: i64,
    pub rule_count: usize,
    pub rule_name: String,
    pub policy_id: String,
    pub policy_name: String,
    pub providers: Vec<WorkflowRouteProvider>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowRouteProvider {
    pub id: String,
    pub name: String,
    pub model: String,
    pub endpoint: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowRetrySummary {
    pub automatic_attempt_count: i64,
    pub automatic_attempt_limit: i64,
    pub automatic_retry_scheduled_count: usize,
    pub manual_retry_requested_count: usize,
    pub historical_retry_policy: bool,
}

pub fn queue_items(
    db: &Database,
    account_email: &str,
    state: Option<&str>,
    page: u32,
    per_page: u32,
) -> Result<Vec<QueueItem>, rusqlite::Error> {
    let labels = label_names(db, account_email);
    db.with_workflow(|repo| repo.list_runs(account_email, state, page, per_page))?
        .iter()
        .map(|run| queue_item(run, &labels))
        .collect()
}

pub fn queue_summary(db: &Database, account_email: &str) -> Result<QueueSummary, rusqlite::Error> {
    db.with_workflow(|repo| {
        Ok(QueueSummary {
            counts: repo.state_counts(account_email)?,
            mailbox: repo.mailbox(account_email)?,
        })
    })
}

pub fn message_detail(
    db: &Database,
    account_email: &str,
    message_id: i64,
) -> Result<Option<MessageDetail>, rusqlite::Error> {
    let Some(run) = db.with_workflow(|repo| repo.run_for_message(account_email, message_id))?
    else {
        return Ok(None);
    };
    let labels = label_names(db, account_email);
    let message = queue_item(&run, &labels)?;
    let body = parse_message(&run)
        .map(|message| crate::rules::engine::email_parts(&message).1)
        .unwrap_or_default();
    let snapshot = serde_json::from_str::<RuleSetSnapshot>(&run.rules_json).ok();
    let current_rule = snapshot
        .as_ref()
        .and_then(|snapshot| rule_context(snapshot, run.next_rule_index));
    db.with_workflow(|repo| {
        let mut action_plans = repo.action_plans(run.run_id)?;
        for action in &mut action_plans {
            action.add_label_names = action
                .add_label_ids
                .iter()
                .map(|label| labels.get(label).cloned().unwrap_or_else(|| label.clone()))
                .collect();
            action.remove_label_names = action
                .remove_label_ids
                .iter()
                .map(|label| labels.get(label).cloned().unwrap_or_else(|| label.clone()))
                .collect();
        }
        let mut llm_attempts = repo.llm_attempts(run.run_id)?;
        if let Some(snapshot) = &snapshot {
            for attempt in &mut llm_attempts {
                attempt.rule_name = rule_name(snapshot, attempt.rule_index);
            }
        }
        let events = repo.events(run.run_id)?;
        Ok(Some(MessageDetail {
            message,
            body,
            current_rule,
            retry_summary: retry_summary(run.attempt_count, &events),
            steps: repo.steps(run.run_id)?,
            llm_attempts,
            action_plans,
            events,
        }))
    })
}

pub fn retry_now(db: &Database, account_email: &str, run_id: i64) -> Result<bool, rusqlite::Error> {
    db.with_workflow(|repo| repo.retry_now(account_email, run_id))
}

pub fn rule_set_status(
    db: &Database,
    account_email: &str,
) -> Result<Vec<crate::db::workflow::WorkflowRuleSetStatus>, rusqlite::Error> {
    db.with_workflow(|repo| repo.rule_set_status(account_email))
}

fn queue_item(
    run: &WorkflowRun,
    label_names: &HashMap<String, String>,
) -> Result<QueueItem, rusqlite::Error> {
    let message = parse_message(run);
    let labels = run
        .labels_json
        .as_deref()
        .and_then(|json| serde_json::from_str(json).ok())
        .or_else(|| message.as_ref().map(|message| message.label_ids.clone()))
        .unwrap_or_default()
        .into_iter()
        .map(|label| label_names.get(&label).cloned().unwrap_or(label))
        .collect();
    let (sender, subject, preview) = message
        .as_ref()
        .map(|message| {
            (
                header(message, "From"),
                header(message, "Subject"),
                message.snippet.clone(),
            )
        })
        .unwrap_or((None, None, String::new()));
    let next_rule_name = serde_json::from_str::<RuleSetSnapshot>(&run.rules_json)
        .ok()
        .and_then(|snapshot| rule_name(&snapshot, run.next_rule_index));
    Ok(QueueItem {
        message_id: run.message_id,
        run_id: run.run_id,
        gmail_message_id: run.gmail_message_id.clone(),
        gmail_thread_id: run.gmail_thread_id.clone(),
        sender,
        subject,
        preview,
        labels,
        state: run.state.clone(),
        next_rule_index: run.next_rule_index,
        next_rule_name,
        has_message_snapshot: message.is_some(),
        attempt_count: run.attempt_count,
        next_attempt_at: run.next_attempt_at.clone(),
        last_error: run.last_error.clone(),
        created_at: run.created_at.clone(),
        updated_at: run.updated_at.clone(),
        completed_at: run.completed_at.clone(),
        rule_set_version: run.rule_set_version,
        next_provider_id: run.route_provider_id.clone(),
        next_endpoint: run.route_endpoint.clone(),
        next_model: run.route_model.clone(),
    })
}

fn rule_name(snapshot: &RuleSetSnapshot, rule_index: i64) -> Option<String> {
    usize::try_from(rule_index)
        .ok()
        .and_then(|index| snapshot.rules.get(index))
        .map(|entry| entry.rule.name.clone())
}

fn rule_context(snapshot: &RuleSetSnapshot, rule_index: i64) -> Option<WorkflowRuleContext> {
    let index = usize::try_from(rule_index).ok()?;
    let rule = snapshot.rules.get(index)?;
    let policy = snapshot
        .llm
        .routing_policies
        .iter()
        .find(|policy| policy.id == rule.rule.inference_policy)
        .or_else(|| {
            snapshot
                .llm
                .routing_policies
                .iter()
                .find(|policy| policy.id == snapshot.llm.default_policy)
        });
    let (policy_id, policy_name, provider_ids) = policy
        .map(|policy| {
            (
                policy.id.clone(),
                policy.name.clone(),
                policy.candidate_provider_ids.clone(),
            )
        })
        .unwrap_or_else(|| ("default".into(), "Default".into(), vec!["legacy".into()]));

    Some(WorkflowRuleContext {
        rule_index,
        rule_count: snapshot.rules.len(),
        rule_name: rule.rule.name.clone(),
        policy_id,
        policy_name,
        providers: provider_ids
            .iter()
            .map(|provider_id| route_provider(&snapshot.llm, provider_id))
            .collect(),
    })
}

fn route_provider(snapshot: &LlmSnapshot, provider_id: &str) -> WorkflowRouteProvider {
    if provider_id == "legacy" {
        return WorkflowRouteProvider {
            id: "legacy".into(),
            name: snapshot.legacy_name.clone(),
            model: snapshot.default_model.clone(),
            endpoint: snapshot.base_url.clone(),
            enabled: snapshot.legacy_enabled,
        };
    }
    snapshot
        .providers
        .iter()
        .find(|provider| provider.id == provider_id)
        .map(|provider| WorkflowRouteProvider {
            id: provider.id.clone(),
            name: provider.name.clone(),
            model: provider.model.clone(),
            endpoint: provider.base_url.clone(),
            enabled: provider.enabled,
        })
        .unwrap_or_else(|| WorkflowRouteProvider {
            id: provider_id.into(),
            name: "Missing provider".into(),
            model: String::new(),
            endpoint: String::new(),
            enabled: false,
        })
}

fn retry_summary(attempt_count: i64, events: &[WorkflowEvent]) -> WorkflowRetrySummary {
    let automatic_retry_scheduled_count = events
        .iter()
        .filter(|event| event.kind == "retry_scheduled")
        .count();
    let manual_retry_requested_count = events
        .iter()
        .filter(|event| {
            matches!(
                event.kind.as_str(),
                "retry_requested" | "retry_current_rules_requested"
            )
        })
        .count();
    WorkflowRetrySummary {
        automatic_attempt_count: attempt_count,
        automatic_attempt_limit: MAX_ATTEMPTS,
        automatic_retry_scheduled_count,
        manual_retry_requested_count,
        // Current runs can schedule only two retries before their third attempt
        // needs attention. More scheduled retries are preserved old history.
        historical_retry_policy: automatic_retry_scheduled_count >= MAX_ATTEMPTS as usize,
    }
}

fn label_names(db: &Database, account_email: &str) -> HashMap<String, String> {
    db.with_labels(|repo| {
        repo.get(account_email).map(|cached| {
            cached
                .into_iter()
                .flat_map(|cached| cached.labels)
                .map(|label| (label.id, label.name))
                .collect()
        })
    })
    .unwrap_or_default()
}

fn parse_message(run: &WorkflowRun) -> Option<Message> {
    run.message_json
        .as_deref()
        .and_then(|json| serde_json::from_str(json).ok())
}

fn header(message: &Message, name: &str) -> Option<String> {
    message.payload.as_ref().and_then(|payload| {
        payload
            .headers
            .iter()
            .find(|header| header.name.eq_ignore_ascii_case(name))
            .map(|header| header.value.clone())
    })
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

/// Publishes the latest rules for every unfinished run that has not been leased.
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
            repo.rebind_unfinished_runs(account_email).map(|_| ())
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
        prepare_run(db, gmail, config, inference_runtime.clone(), run).await?;
        processed += 1;
    }
    Ok(processed)
}

pub async fn prepare_run(
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
            Err(GmailError::Api { code: 404, .. }) => {
                db.with_workflow(|repo| {
                    repo.resolve_externally(
                        run.id,
                        &run.lease_token,
                        "Message was deleted outside Post Office",
                    )
                })?;
                return Ok(());
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

    if run.message_json.is_some() {
        match gmail.get_message(&run.gmail_message_id).await {
            Ok(remote) => {
                let pending_action =
                    db.with_workflow(|repo| repo.action_for_rule(run.id, run.next_rule_index))?;
                let action_already_applied = pending_action
                    .as_ref()
                    .is_some_and(|action| target_reached(&remote, action));
                if !action_already_applied {
                    if let Some(reason) = externally_resolved_reason(&message, &remote) {
                        let message_json = serde_json::to_string(&remote)?;
                        let labels_json = serde_json::to_string(&remote.label_ids)?;
                        let stored = db.with_workflow(|repo| {
                            repo.store_message_snapshot(
                                run.id,
                                &run.lease_token,
                                &remote.thread_id,
                                &message_json,
                                &labels_json,
                            )
                        })?;
                        if !stored {
                            return Ok(());
                        }
                        db.with_workflow(|repo| {
                            repo.resolve_externally(run.id, &run.lease_token, reason)
                        })?;
                        return Ok(());
                    }
                }
                let message_json = serde_json::to_string(&remote)?;
                let labels_json = serde_json::to_string(&remote.label_ids)?;
                let stored = db.with_workflow(|repo| {
                    repo.store_message_snapshot(
                        run.id,
                        &run.lease_token,
                        &remote.thread_id,
                        &message_json,
                        &labels_json,
                    )
                })?;
                if !stored {
                    return Ok(());
                }
                message = remote;
            }
            Err(GmailError::Api { code: 404, .. }) => {
                db.with_workflow(|repo| {
                    repo.resolve_externally(
                        run.id,
                        &run.lease_token,
                        "Message was deleted outside Post Office",
                    )
                })?;
                return Ok(());
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
        }
    }

    let llm = InferenceRouter::from_config_with_runtime(
        &snapshot.llm.apply_to(config),
        inference_runtime,
    )
    .with_database(db.as_ref().clone());
    let llm = if run.manual_retry {
        llm.with_endpoint_circuit_bypass()
    } else {
        llm
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

        if !needs_llm_decision(rule, &labels) {
            let resolved =
                resolve_rule(&llm, rule, &message, &rule_snapshot.memories, &labels).await?;
            persist_decision(db, &run, rule, rule_index, &labels, &resolved)?;
            return Ok(());
        }

        match llm.next_provider(&rule.inference_policy) {
            Ok(provider) => {
                db.with_workflow(|repo| {
                    repo.queue_for_route(
                        run.id,
                        &run.lease_token,
                        &provider.provider_id,
                        &provider.endpoint,
                        &provider.model,
                        provider.max_concurrent_requests,
                    )
                })?;
            }
            Err(error) => {
                record_decision_error(db, &run, &llm, rule_index, &rule.inference_policy, error)?;
            }
        }
        return Ok(());
    }
}

pub async fn process_routed_run(
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
    let message = match run.message_json.as_deref() {
        Some(json) => match serde_json::from_str(json) {
            Ok(message) => message,
            Err(error) => {
                db.with_workflow(|repo| {
                    repo.mark_needs_attention(run.id, &run.lease_token, &error.to_string())
                })?;
                return Ok(());
            }
        },
        None => {
            db.with_workflow(|repo| {
                repo.mark_needs_attention(
                    run.id,
                    &run.lease_token,
                    "A routed decision is missing its Gmail message snapshot",
                )
            })?;
            return Ok(());
        }
    };
    let llm = InferenceRouter::from_config_with_runtime(
        &snapshot.llm.apply_to(config),
        inference_runtime,
    )
    .with_database(db.as_ref().clone());
    let llm = if run.manual_retry {
        llm.with_endpoint_circuit_bypass()
    } else {
        llm
    }
    .without_fallback();
    let labels = cached_labels(db, gmail, &run.account_email)
        .await
        .unwrap_or_default();
    let Some(rule_snapshot) = snapshot.rules.get(run.next_rule_index as usize) else {
        db.with_workflow(|repo| {
            repo.complete_run(run.id, &run.lease_token, "all rules considered")
        })?;
        return Ok(());
    };
    let rule = &rule_snapshot.rule;
    if !needs_llm_decision(rule, &labels) {
        db.with_workflow(|repo| repo.yield_run(run.id, &run.lease_token))?;
        return Ok(());
    }
    let (route_endpoint, route_model) = match (&run.route_endpoint, &run.route_model) {
        (Some(endpoint), Some(model)) => (endpoint, model),
        _ => {
            db.with_workflow(|repo| repo.yield_run(run.id, &run.lease_token))?;
            return Ok(());
        }
    };
    match llm.next_provider(&rule.inference_policy) {
        Ok(provider) if provider.endpoint == *route_endpoint && provider.model == *route_model => {}
        Ok(_) => {
            db.with_workflow(|repo| repo.yield_run(run.id, &run.lease_token))?;
            return Ok(());
        }
        Err(error) => {
            record_decision_error(
                db,
                &run,
                &llm,
                run.next_rule_index,
                &rule.inference_policy,
                error,
            )?;
            return Ok(());
        }
    }
    let started = Instant::now();
    let resolved = match resolve_rule(&llm, rule, &message, &rule_snapshot.memories, &labels).await
    {
        Ok(resolved) => resolved,
        Err(error) => {
            record_decision_error(
                db,
                &run,
                &llm,
                run.next_rule_index,
                &rule.inference_policy,
                error,
            )?;
            return Ok(());
        }
    };
    let attribution = llm.provider_attribution(&rule.inference_policy);
    let provider_name = attribution.as_ref().and_then(|value| {
        (resolved.llm_provider.as_deref() == Some(value.provider_id.as_str()))
            .then_some(value.provider_name.as_str())
    });
    record_decision_attempt(
        db,
        run.id,
        run.next_rule_index,
        &resolved,
        "succeeded",
        None,
        started,
        attribution.as_ref().map(|value| value.endpoint.as_str()),
        provider_name,
    )?;
    persist_decision(db, &run, rule, run.next_rule_index, &labels, &resolved)?;
    Ok(())
}

fn record_decision_error(
    db: &Arc<Database>,
    run: &ClaimedRun,
    llm: &InferenceRouter,
    rule_index: i64,
    policy_id: &str,
    error: impl std::fmt::Display,
) -> Result<(), rusqlite::Error> {
    let error = error.to_string();
    let attribution = llm.provider_attribution(policy_id);
    let endpoint_circuit_open = llm.endpoint_circuit_open(policy_id);
    db.with_workflow(|repo| {
        repo.record_llm_attempt(
            run.id,
            rule_index,
            attribution.as_ref().map(|value| value.provider_id.as_str()),
            attribution
                .as_ref()
                .map(|value| value.provider_name.as_str()),
            attribution.as_ref().map(|value| value.model.as_str()),
            if endpoint_circuit_open {
                "blocked"
            } else {
                "error"
            },
            Some(&error),
            None,
            attribution.as_ref().map(|value| value.endpoint.as_str()),
        )?;
        if let Some(provider) = run
            .route_provider_id
            .as_deref()
            .and_then(|provider_id| llm.fallback_provider(policy_id, provider_id))
        {
            return repo.queue_for_route(
                run.id,
                &run.lease_token,
                &provider.provider_id,
                &provider.endpoint,
                &provider.model,
                provider.max_concurrent_requests,
            );
        }
        if endpoint_circuit_open {
            repo.mark_needs_attention(run.id, &run.lease_token, &error)
        } else {
            repo.retry_run(run.id, &run.lease_token, &error, RETRY_DELAY_SECS)
        }
    })
    .map(|_| ())
}

fn persist_decision(
    db: &Arc<Database>,
    run: &ClaimedRun,
    rule: &Rule,
    rule_index: i64,
    labels: &[crate::gmail::models::Label],
    resolved: &Resolved,
) -> Result<(), rusqlite::Error> {
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
            if advanced {
                db.with_workflow(|repo| repo.yield_run(run.id, &run.lease_token))?;
            }
        }
        Outcome::Unparsed => {
            let error = resolved
                .diagnostic
                .as_deref()
                .unwrap_or("Invalid LLM decision");
            db.with_workflow(|repo| {
                repo.retry_run(run.id, &run.lease_token, error, RETRY_DELAY_SECS)
            })?;
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
            if rule.continue_after_match {
                db.with_workflow(|repo| repo.yield_run(run.id, &run.lease_token))?;
            } else {
                db.with_workflow(|repo| {
                    repo.complete_run(
                        run.id,
                        &run.lease_token,
                        "rule claimed message without action",
                    )
                })?;
            }
        }
        Outcome::Matched => {
            let mutation = match resolve_actions(&resolved.actions, labels) {
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
            let planned = db.with_workflow(|repo| {
                repo.plan_action(
                    run.id,
                    &run.lease_token,
                    rule_index,
                    &mutation.add,
                    &mutation.remove,
                )
            })?;
            if planned.is_some() {
                db.with_workflow(|repo| repo.yield_run(run.id, &run.lease_token))?;
            }
        }
    }
    Ok(())
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
    if target_reached(message, action) {
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

fn externally_resolved_reason(previous: &Message, current: &Message) -> Option<&'static str> {
    let was_trashed = previous.label_ids.iter().any(|label| label == "TRASH");
    let is_trashed = current.label_ids.iter().any(|label| label == "TRASH");
    if !was_trashed && is_trashed {
        return Some("Message was moved to Trash outside Post Office");
    }

    let was_inbox = previous.label_ids.iter().any(|label| label == "INBOX");
    let is_inbox = current.label_ids.iter().any(|label| label == "INBOX");
    (was_inbox && !is_inbox).then_some("Message was archived outside Post Office")
}

fn apply_local_labels(message: &mut Message, action: &ActionPlan) {
    let mut labels = message.label_ids.iter().cloned().collect::<BTreeSet<_>>();
    for label in &action.remove_label_ids {
        labels.remove(label);
    }
    labels.extend(action.add_label_ids.iter().cloned());
    message.label_ids = labels.into_iter().collect();
}

#[expect(
    clippy::too_many_arguments,
    reason = "Decision telemetry carries all provider and timing fields as one durable attempt."
)]
fn record_decision_attempt(
    db: &Arc<Database>,
    run_id: i64,
    rule_index: i64,
    resolved: &Resolved,
    status: &str,
    error: Option<&str>,
    started: Instant,
    endpoint: Option<&str>,
    provider_name: Option<&str>,
) -> Result<(), rusqlite::Error> {
    db.with_workflow(|repo| {
        repo.record_llm_attempt(
            run_id,
            rule_index,
            resolved.llm_provider.as_deref(),
            provider_name,
            resolved.llm_model.as_deref(),
            status,
            error,
            Some(elapsed_ms(started)),
            endpoint,
        )
    })
}

fn elapsed_ms(started: Instant) -> i64 {
    started.elapsed().as_millis().min(i64::MAX as u128) as i64
}

fn non_empty(value: &str) -> Option<&str> {
    (!value.trim().is_empty()).then_some(value)
}

pub fn new_lease_token() -> String {
    format!("{:032x}", rand::thread_rng().gen::<u128>())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message_with_labels(labels: &[&str]) -> Message {
        Message {
            id: "m1".into(),
            thread_id: "t1".into(),
            label_ids: labels.iter().map(|label| (*label).into()).collect(),
            snippet: String::new(),
            history_id: "1".into(),
            internal_date: String::new(),
            size_estimate: 0,
            payload: None,
        }
    }

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

    #[test]
    fn detects_manual_archive_and_trash() {
        assert_eq!(
            externally_resolved_reason(&message_with_labels(&["INBOX"]), &message_with_labels(&[]),),
            Some("Message was archived outside Post Office")
        );
        assert_eq!(
            externally_resolved_reason(
                &message_with_labels(&["INBOX"]),
                &message_with_labels(&["TRASH"]),
            ),
            Some("Message was moved to Trash outside Post Office")
        );
    }

    #[test]
    fn exposes_the_rule_and_route_pinned_to_a_run() {
        let config = AppConfig {
            llm_providers: vec![LlmProviderProfile {
                id: "taxonomy".into(),
                name: "Shade Taxonomy".into(),
                base_url: "http://shade:4000/v1".into(),
                model: "taxonomy".into(),
                api_key_ref: "taxonomy".into(),
                quality_tier: "cheap".into(),
                privacy_status: "local".into(),
                input_cost_per_million_usd: 0.0,
                output_cost_per_million_usd: 0.0,
                timeout_secs: 30,
                context_window_tokens: 8_192,
                max_concurrent_requests: 1,
                output_tokens_per_second: 0.0,
                chat_reasoning_effort: ReasoningEffort::ServerDefault,
                enabled: true,
            }],
            llm_routing_policies: vec![LlmRoutingPolicy {
                id: "taxonomy".into(),
                name: "Taxonomy".into(),
                candidate_provider_ids: vec!["taxonomy".into()],
                minimum_quality: "cheap".into(),
                privacy_requirement: Default::default(),
                allow_fallback: true,
            }],
            ..AppConfig::default()
        };
        let snapshot = RuleSetSnapshot {
            rules: vec![RuleSnapshot {
                rule: Rule {
                    id: 7,
                    name: "Trash GitHub notifications".into(),
                    description: None,
                    conditions: vec![],
                    prompt: "Decide whether to discard this message.".into(),
                    choices: vec![],
                    choose_from_all_labels: false,
                    actions: vec![],
                    priority: 0,
                    enabled: true,
                    parent_id: None,
                    inference_policy: "taxonomy".into(),
                    decision_reasoning_effort: ReasoningEffort::ServerDefault,
                    decision_max_tokens: None,
                    continue_after_match: false,
                },
                memories: vec![],
            }],
            llm: LlmSnapshot::capture(&config),
        };

        let context = rule_context(&snapshot, 0).unwrap();

        assert_eq!(context.rule_name, "Trash GitHub notifications");
        assert_eq!(context.policy_name, "Taxonomy");
        assert_eq!(context.providers[0].name, "Shade Taxonomy");
        assert_eq!(context.providers[0].model, "taxonomy");
    }

    #[test]
    fn separates_manual_retries_from_preserved_legacy_history() {
        let mut events = (0..6)
            .map(|id| WorkflowEvent {
                id,
                kind: "retry_scheduled".into(),
                detail: None,
                created_at: String::new(),
            })
            .collect::<Vec<_>>();
        events.extend((6..8).map(|id| WorkflowEvent {
            id,
            kind: "retry_requested".into(),
            detail: None,
            created_at: String::new(),
        }));
        events.push(WorkflowEvent {
            id: 8,
            kind: "retry_current_rules_requested".into(),
            detail: None,
            created_at: String::new(),
        });

        let summary = retry_summary(3, &events);

        assert_eq!(summary.automatic_attempt_count, 3);
        assert_eq!(summary.automatic_retry_scheduled_count, 6);
        assert_eq!(summary.manual_retry_requested_count, 3);
        assert!(summary.historical_retry_policy);
    }
}
