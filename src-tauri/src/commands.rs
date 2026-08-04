use post_office_core::config::AppConfig;
use post_office_core::db::rule_chat::ChatMessageRow;
use post_office_core::db::rules::{CreateRuleRequest, UpdateRuleRequest};
use post_office_core::gmail::models::Label;
use post_office_core::gmail::oauth::{auth_url, exchange_code, generate_pkce};
use post_office_core::gmail::{store_tokens, GmailAuth, GmailClient};
use post_office_core::llm::{InferenceRouter, LlmClient};
use post_office_core::llm::{LlmProviderProfile, LlmRoutingPolicy};
use post_office_core::processing::{
    mark_last_successful, run_backfill, run_processing_loop, OpProgress,
};
use post_office_core::rules::actions::execute_action;
use post_office_core::rules::chat::{apply_proposal, chat_with_rule, ChatProposal, ChatTurn};
use post_office_core::rules::engine::{display_action, ActionDisplay, TestResult};
use post_office_core::rules::evaluation::BulkVerdict;
use post_office_core::rules::models::{Action, Condition, Rule};
use post_office_core::rules::response_parser::ParsedAction;
use post_office_core::sync::{replay_history, start_watch, stop_watch, ReplayResult};
use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::Emitter;
use tauri::Manager;
use tauri::State;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use chrono::{DateTime, NaiveDate, Utc};

use crate::sync_runtime::SyncTrigger;
use crate::tray;
use crate::AppState;

pub fn ensure_polling_started(app: tauri::AppHandle, state: &AppState, config: &AppConfig) {
    if state.poller_started.swap(true, Ordering::SeqCst) {
        return;
    }

    let db_clone = Arc::new(state.db.clone());
    let state_clone = state.processing_state.clone();
    let config_clone = state.config.clone();
    let config_for_auth = config.clone();
    let poller_flag = state.poller_started.clone();
    tauri::async_runtime::spawn(async move {
        match load_gmail_auth(&app, &config_for_auth) {
            Ok(auth) => {
                let gmail = GmailClient::new(auth);
                let emit_handle = app.clone();
                run_processing_loop(
                    db_clone,
                    state_clone,
                    gmail,
                    config_clone,
                    move |progress| {
                        let _ = emit_handle.emit("cycle-progress", &progress);
                    },
                )
                .await;
            }
            Err(e) => {
                tracing::warn!("Gmail processing loop not started: {}", e);
            }
        }
        poller_flag.store(false, Ordering::SeqCst);
    });
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RuleCreateRequest {
    pub name: String,
    pub description: Option<String>,
    pub conditions: Vec<Condition>,
    pub prompt: String,
    pub actions: Vec<Action>,
    pub priority: i32,
    pub enabled: bool,
    #[serde(default = "default_policy")]
    pub inference_policy: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RuleUpdateRequest {
    pub name: String,
    pub description: Option<String>,
    pub conditions: Vec<Condition>,
    pub prompt: String,
    pub actions: Vec<Action>,
    pub priority: i32,
    pub enabled: bool,
    #[serde(default = "default_policy")]
    pub inference_policy: String,
}

fn default_policy() -> String {
    "default".into()
}

#[derive(Debug, Serialize)]
pub struct RecentMessage {
    pub id: String,
    pub from: String,
    pub subject: String,
    pub snippet: String,
}

#[derive(Debug, Serialize)]
pub struct ApplyResult {
    pub matched: bool,
    pub applied: Vec<ActionDisplay>,
    pub error: Option<String>,
}

/// Build an in-memory `Rule` from a request payload for dry-run/apply testing.
fn build_rule_model(rule: &RuleCreateRequest) -> Rule {
    Rule {
        id: 0,
        name: rule.name.clone(),
        description: rule.description.clone(),
        conditions: rule.conditions.clone(),
        prompt: rule.prompt.clone(),
        actions: rule.actions.clone(),
        priority: rule.priority,
        enabled: rule.enabled,
        parent_id: None,
        inference_policy: if rule.inference_policy.trim().is_empty() {
            default_policy()
        } else {
            rule.inference_policy.clone()
        },
    }
}

fn normalize_label_ref(value: &str, labels: &[Label]) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    labels
        .iter()
        .find(|l| l.id == trimmed)
        .or_else(|| labels.iter().find(|l| l.name.eq_ignore_ascii_case(trimmed)))
        .map(|l| l.id.clone())
}

fn normalize_label_ref_or_error(
    value: &str,
    labels: &[Label],
    context: &str,
) -> Result<String, String> {
    normalize_label_ref(value, labels)
        .ok_or_else(|| format!("Unknown label in {context}: {}", value.trim()))
}

fn normalize_rule_label_fields(
    conditions: &mut [Condition],
    actions: &mut [Action],
    labels: &[Label],
) -> Result<(), String> {
    for condition in conditions.iter_mut() {
        if let Condition::Label { value, .. } = condition {
            *value = normalize_label_ref_or_error(value, labels, "condition")?;
        }
    }

    for action in actions.iter_mut() {
        if let Action::Label { value } = action {
            *value = normalize_label_ref_or_error(value, labels, "action")?;
        }
    }

    Ok(())
}

fn normalize_proposal_label_actions(
    proposal: &mut ChatProposal,
    labels: &[Label],
) -> Result<(), String> {
    for action in proposal.actions_add.iter_mut() {
        if let Action::Label { value } = action {
            *value = normalize_label_ref_or_error(value, labels, "proposal action")?;
        }
    }
    Ok(())
}

fn conditions_need_label_resolution(conditions: &[Condition]) -> bool {
    conditions
        .iter()
        .any(|condition| matches!(condition, Condition::Label { .. }))
}

fn actions_need_label_resolution(actions: &[Action]) -> bool {
    actions
        .iter()
        .any(|action| matches!(action, Action::Label { .. }))
}

fn extract_header(email: &post_office_core::gmail::models::Message, name: &str) -> Option<String> {
    email.payload.as_ref().and_then(|p| {
        p.headers
            .iter()
            .find(|h| h.name == name)
            .map(|h| h.value.clone())
    })
}

#[derive(Debug, Serialize)]
pub struct ProcessingStatus {
    pub paused: bool,
    pub polling_enabled: bool,
    pub last_processed: Option<String>,
    pub last_successful: Option<String>,
    pub emails_processed_today: usize,
    pub last_cycle_count: usize,
    pub last_cycle_error: Option<String>,
    pub active_phase: String,
    pub current_progress: Option<OpProgress>,
    pub backfill_running: bool,
    pub backfill_cancel_requested: bool,
}

#[tauri::command]
pub async fn config_get(state: State<'_, AppState>) -> Result<AppConfig, String> {
    let config = state.config.lock().await;
    Ok(config.clone())
}

#[tauri::command]
pub async fn config_set(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    key: String,
    value: String,
) -> Result<(), String> {
    let sync_related = key.starts_with("sync.");
    let mut config = state.config.lock().await;
    let mut start_poller = false;
    match key.as_str() {
        "gmail.account" => config.gmail_account = if value.is_empty() { None } else { Some(value) },
        "llm.base_url" => config.llm_base_url = value,
        "llm.api_key" => config.llm_api_key = value,
        "llm.default_model" => config.llm_default_model = value,
        "llm.temperature" => {
            let val: f32 = value
                .parse()
                .map_err(|e: std::num::ParseFloatError| e.to_string())?;
            config.llm_temperature = val;
        }
        "llm.max_tokens" => {
            let val: u32 = value
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            config.llm_max_tokens = val;
        }
        "llm.timeout_secs" => {
            let val: u64 = value
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            config.llm_timeout_secs = val;
        }
        "llm.input_cost_per_million_usd" => {
            let val: f64 = value
                .parse()
                .map_err(|e: std::num::ParseFloatError| e.to_string())?;
            config.llm_input_cost_per_million_usd = val;
        }
        "llm.output_cost_per_million_usd" => {
            let val: f64 = value
                .parse()
                .map_err(|e: std::num::ParseFloatError| e.to_string())?;
            config.llm_output_cost_per_million_usd = val;
        }
        "llm.providers" => {
            config.llm_providers = serde_json::from_str(&value).map_err(|e| e.to_string())?;
        }
        "llm.routing_policies" => {
            config.llm_routing_policies =
                serde_json::from_str(&value).map_err(|e| e.to_string())?;
        }
        "llm.default_policy" => config.llm_default_policy = value,
        "polling.query" => config.polling_query = value,
        "polling.interval_minutes" => {
            let val: u32 = value
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            config.polling_interval_minutes = val;
        }
        "polling.max_per_cycle" => {
            let val: u32 = value
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            config.polling_max_per_cycle = val;
        }
        "polling.enabled" => {
            let val: bool = value
                .parse()
                .map_err(|e: std::str::ParseBoolError| e.to_string())?;
            config.polling_enabled = val;
        }
        "sync.enabled" => {
            let val: bool = value
                .parse()
                .map_err(|e: std::str::ParseBoolError| e.to_string())?;
            config.sync_enabled = val;
            start_poller = !val;
        }
        "sync.reconcile_interval_minutes" => {
            let val: u32 = value
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            config.sync_reconcile_interval_minutes = val;
        }
        "sync.watch.topic" => config.sync_watch_topic = value,
        "sync.watch.label_ids" => config.sync_watch_label_ids = value,
        "sync.relay.enabled" => {
            let val: bool = value
                .parse()
                .map_err(|e: std::str::ParseBoolError| e.to_string())?;
            config.relay_enabled = val;
        }
        "sync.relay.ws_url" => config.relay_ws_url = value,
        "sync.relay.auth_token" => config.relay_auth_token = value,
        "google.client_id" => config.google_client_id = value,
        "ui.tray_theme" => config.tray_theme = value,
        _ => return Err(format!("Unknown config key: {}", key)),
    }
    state
        .db
        .with_config(|repo| config.save(&repo))
        .map_err(|e| e.to_string())?;
    let config_snapshot = config.clone();
    drop(config);
    if sync_related {
        let _ = state.sync_trigger.send(SyncTrigger::Startup);
    }
    if start_poller {
        ensure_polling_started(app, &state, &config_snapshot);
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct LlmConfigUpdate {
    pub base_url: String,
    pub api_key: String,
    pub default_model: String,
    pub input_cost_per_million_usd: f64,
    pub output_cost_per_million_usd: f64,
    pub timeout_secs: u64,
    pub providers: Vec<LlmProviderProfile>,
    pub routing_policies: Vec<LlmRoutingPolicy>,
    pub default_policy: String,
}

#[tauri::command]
pub async fn llm_config_set(
    state: State<'_, AppState>,
    update: LlmConfigUpdate,
) -> Result<(), String> {
    let mut config = state.config.lock().await;
    config.llm_base_url = update.base_url;
    config.llm_api_key = update.api_key;
    config.llm_default_model = update.default_model;
    config.llm_input_cost_per_million_usd = update.input_cost_per_million_usd;
    config.llm_output_cost_per_million_usd = update.output_cost_per_million_usd;
    config.llm_timeout_secs = update.timeout_secs;
    config.llm_providers = update.providers;
    config.llm_routing_policies = update.routing_policies;
    config.llm_default_policy = update.default_policy;
    state
        .db
        .with_config(|repo| config.save(&repo))
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn rules_list(
    state: State<'_, AppState>,
) -> Result<Vec<post_office_core::rules::models::Rule>, String> {
    state
        .db
        .with_rules(|repo| repo.list_all())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rules_create(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    mut rule: RuleCreateRequest,
) -> Result<post_office_core::rules::models::Rule, String> {
    if conditions_need_label_resolution(&rule.conditions)
        || actions_need_label_resolution(&rule.actions)
    {
        let config = state.config.lock().await;
        let auth = load_gmail_auth(&app, &config)?;
        drop(config);

        let mut gmail = GmailClient::new(auth);
        let labels = gmail.list_labels().await.map_err(|e| e.to_string())?;
        normalize_rule_label_fields(&mut rule.conditions, &mut rule.actions, &labels)?;
    }

    let create = CreateRuleRequest {
        name: rule.name,
        description: rule.description,
        conditions: rule.conditions,
        prompt: rule.prompt,
        actions: rule.actions,
        priority: rule.priority,
        enabled: rule.enabled,
        inference_policy: if rule.inference_policy.trim().is_empty() {
            default_policy()
        } else {
            rule.inference_policy
        },
    };
    state
        .db
        .with_rules(|repo| repo.create(&create))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rules_update(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: i64,
    mut rule: RuleUpdateRequest,
) -> Result<post_office_core::rules::models::Rule, String> {
    if conditions_need_label_resolution(&rule.conditions)
        || actions_need_label_resolution(&rule.actions)
    {
        let config = state.config.lock().await;
        let auth = load_gmail_auth(&app, &config)?;
        drop(config);

        let mut gmail = GmailClient::new(auth);
        let labels = gmail.list_labels().await.map_err(|e| e.to_string())?;
        normalize_rule_label_fields(&mut rule.conditions, &mut rule.actions, &labels)?;
    }

    let update = UpdateRuleRequest {
        name: rule.name,
        description: rule.description,
        conditions: rule.conditions,
        prompt: rule.prompt,
        actions: rule.actions,
        priority: rule.priority,
        enabled: rule.enabled,
        inference_policy: if rule.inference_policy.trim().is_empty() {
            default_policy()
        } else {
            rule.inference_policy
        },
    };
    state
        .db
        .with_rules(|repo| repo.update(id, &update))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rules_delete(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    state
        .db
        .with_rules(|repo| repo.delete(id))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rule_chat_history(
    state: State<'_, AppState>,
    rule_id: i64,
) -> Result<Vec<ChatMessageRow>, String> {
    state
        .db
        .with_rule_chat(|repo| repo.list(rule_id))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rule_chat_send(
    state: State<'_, AppState>,
    rule_id: i64,
    message: String,
) -> Result<ChatTurn, String> {
    let llm = {
        let config = state.config.lock().await;
        InferenceRouter::from_config(&config).with_database(state.db.clone())
    };

    chat_with_rule(&state.db, &llm, rule_id, &message)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rule_apply_proposal(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    rule_id: i64,
    mut proposal: ChatProposal,
) -> Result<(), String> {
    if actions_need_label_resolution(&proposal.actions_add) {
        let config = state.config.lock().await;
        let auth = load_gmail_auth(&app, &config)?;
        drop(config);

        let mut gmail = GmailClient::new(auth);
        let labels = gmail.list_labels().await.map_err(|e| e.to_string())?;
        normalize_proposal_label_actions(&mut proposal, &labels)?;
    }

    apply_proposal(&state.db, rule_id, &proposal)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn gmail_recent_messages(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    max: u32,
) -> Result<Vec<RecentMessage>, String> {
    let config = state.config.lock().await;
    let auth = load_gmail_auth(&app, &config)?;
    let query = config.polling_query.clone();
    drop(config);

    let mut gmail = GmailClient::new(auth);
    let refs = gmail
        .list_messages(&query, max.max(1))
        .await
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for r in refs {
        if let Ok(msg) = gmail.get_message(&r.id).await {
            out.push(RecentMessage {
                id: r.id.clone(),
                from: extract_header(&msg, "From").unwrap_or_default(),
                subject: extract_header(&msg, "Subject").unwrap_or_default(),
                snippet: msg.snippet.clone(),
            });
        }
    }
    Ok(out)
}

#[tauri::command]
pub async fn rules_test(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    mut rule: RuleCreateRequest,
    message_id: String,
) -> Result<TestResult, String> {
    let (auth, llm) = {
        let config = state.config.lock().await;
        let auth = load_gmail_auth(&app, &config)?;
        let llm = InferenceRouter::from_config(&config).with_database(state.db.clone());
        (auth, llm)
    };

    let mut gmail = GmailClient::new(auth);
    if conditions_need_label_resolution(&rule.conditions)
        || actions_need_label_resolution(&rule.actions)
    {
        let labels = gmail.list_labels().await.map_err(|e| e.to_string())?;
        normalize_rule_label_fields(&mut rule.conditions, &mut rule.actions, &labels)?;
    }

    let email = gmail
        .get_message(&message_id)
        .await
        .map_err(|e| e.to_string())?;

    let rule_model = build_rule_model(&rule);

    let memories = state.db.with_rule_memory(|repo| {
        repo.list_for_rule(rule_model.id)
            .map(|rows| rows.into_iter().map(|e| e.text).collect::<Vec<String>>())
            .unwrap_or_default()
    });

    post_office_core::rules::engine::test_rule(&llm, &rule_model, &email, &memories)
        .await
        .map_err(|e| e.to_string())
}

/// Evaluates a rule against many emails in one batched LLM pass (or locally for
/// structured-action rules) and returns the proposed action per email. No emails
/// are mutated — the frontend applies chosen verdicts via `rules_apply`.
#[tauri::command]
pub async fn bulk_evaluate(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    mut rule: RuleCreateRequest,
    message_ids: Vec<String>,
) -> Result<Vec<BulkVerdict>, String> {
    let (auth, llm) = {
        let config = state.config.lock().await;
        let auth = load_gmail_auth(&app, &config)?;
        let llm = InferenceRouter::from_config(&config).with_database(state.db.clone());
        (auth, llm)
    };

    let mut gmail = GmailClient::new(auth);
    if conditions_need_label_resolution(&rule.conditions)
        || actions_need_label_resolution(&rule.actions)
    {
        let labels = gmail.list_labels().await.map_err(|e| e.to_string())?;
        normalize_rule_label_fields(&mut rule.conditions, &mut rule.actions, &labels)?;
    }

    let mut emails = Vec::new();
    for id in &message_ids {
        if let Ok(msg) = gmail.get_message(id).await {
            emails.push(msg);
        }
    }

    let rule_model = build_rule_model(&rule);

    post_office_core::rules::evaluation::bulk_evaluate(&llm, &rule_model, &emails)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rules_apply(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    mut rule: RuleCreateRequest,
    message_id: String,
) -> Result<ApplyResult, String> {
    let (auth, llm) = {
        let config = state.config.lock().await;
        let auth = load_gmail_auth(&app, &config)?;
        let llm = InferenceRouter::from_config(&config).with_database(state.db.clone());
        (auth, llm)
    };

    let mut gmail = GmailClient::new(auth);
    let labels = gmail.list_labels().await.map_err(|e| e.to_string())?;
    if conditions_need_label_resolution(&rule.conditions)
        || actions_need_label_resolution(&rule.actions)
    {
        normalize_rule_label_fields(&mut rule.conditions, &mut rule.actions, &labels)?;
    }

    let email = gmail
        .get_message(&message_id)
        .await
        .map_err(|e| e.to_string())?;

    let rule_model = build_rule_model(&rule);

    let memories = state.db.with_rule_memory(|repo| {
        repo.list_for_rule(rule_model.id)
            .map(|rows| rows.into_iter().map(|e| e.text).collect::<Vec<String>>())
            .unwrap_or_default()
    });

    let resolved =
        post_office_core::rules::engine::resolve_rule(&llm, &rule_model, &email, &memories)
            .await
            .map_err(|e| e.to_string())?;

    let resolved = match resolved {
        None => {
            return Ok(ApplyResult {
                matched: false,
                applied: vec![],
                error: None,
            });
        }
        Some(r) => r,
    };

    let mut outcomes: Vec<(&ParsedAction, Option<String>)> = Vec::new();
    for action in &resolved.actions {
        let error = execute_action(&mut gmail, &message_id, action, &labels)
            .await
            .err()
            .map(|e| e.to_string());
        outcomes.push((action, error));
    }

    let applied: Vec<ActionDisplay> = outcomes
        .iter()
        .filter(|(_, e)| e.is_none())
        .map(|(a, _)| display_action(a))
        .collect();
    let errors: Vec<String> = outcomes.iter().filter_map(|(_, e)| e.clone()).collect();

    Ok(ApplyResult {
        matched: true,
        applied,
        error: if errors.is_empty() {
            None
        } else {
            Some(errors.join("; "))
        },
    })
}

#[tauri::command]
pub async fn history_list(
    state: State<'_, AppState>,
    page: u32,
    per_page: u32,
) -> Result<Vec<post_office_core::db::history::HistoryEntry>, String> {
    state
        .db
        .with_history(|repo| repo.list(page, per_page))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rules_metrics(
    state: State<'_, AppState>,
) -> Result<Vec<post_office_core::db::history::RuleMetrics>, String> {
    state
        .db
        .with_history(|repo| repo.rules_metrics())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rule_roi_metrics(
    state: State<'_, AppState>,
) -> Result<Vec<post_office_core::db::llm_usage::RuleRoiMetrics>, String> {
    state
        .db
        .with_llm_usage(|repo| repo.rule_roi_metrics())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn history_search(
    state: State<'_, AppState>,
    query: String,
) -> Result<Vec<post_office_core::db::history::HistoryEntry>, String> {
    state
        .db
        .with_history(|repo| repo.search(&query))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn inference_jobs_list(
    state: State<'_, AppState>,
    page: u32,
    per_page: u32,
) -> Result<Vec<post_office_core::db::inference::InferenceJob>, String> {
    state
        .db
        .with_inference(|repo| repo.list(page, per_page))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn inference_job_retry(state: State<'_, AppState>, job_id: i64) -> Result<bool, String> {
    state
        .db
        .with_inference(|repo| repo.retry(job_id))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn llm_provider_set_api_key(provider_id: String, api_key: String) -> Result<(), String> {
    post_office_core::llm::credentials::store_provider_api_key(&provider_id, &api_key)
}

#[tauri::command]
pub async fn llm_provider_status_list(
    state: State<'_, AppState>,
) -> Result<Vec<post_office_core::db::llm_provider_status::LlmProviderStatus>, String> {
    state
        .db
        .with_llm_provider_status(|repo| repo.list())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn processing_status(state: State<'_, AppState>) -> Result<ProcessingStatus, String> {
    let polling_enabled = state.config.lock().await.polling_enabled;
    let ps = state.processing_state.lock().await;
    Ok(ProcessingStatus {
        paused: ps.paused.load(std::sync::atomic::Ordering::Relaxed),
        polling_enabled,
        last_processed: ps.last_processed.map(|dt| dt.to_rfc3339()),
        last_successful: ps.last_successful.map(|dt| dt.to_rfc3339()),
        emails_processed_today: ps.emails_processed_today,
        last_cycle_count: ps.last_cycle_count,
        last_cycle_error: ps.last_cycle_error.clone(),
        active_phase: ps.active_phase.clone(),
        current_progress: ps.current_progress.clone(),
        backfill_running: ps
            .backfill_running
            .load(std::sync::atomic::Ordering::Relaxed),
        backfill_cancel_requested: ps
            .backfill_cancel_requested
            .load(std::sync::atomic::Ordering::Relaxed),
    })
}

#[tauri::command]
pub async fn processing_pause(state: State<'_, AppState>) -> Result<(), String> {
    let ps = state.processing_state.lock().await;
    ps.paused.store(true, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}

#[tauri::command]
pub async fn processing_resume(state: State<'_, AppState>) -> Result<(), String> {
    let ps = state.processing_state.lock().await;
    ps.paused.store(false, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct SyncStatus {
    pub enabled: bool,
    pub relay_enabled: bool,
    pub account_email: Option<String>,
    pub state: Option<post_office_core::db::sync_state::GmailSyncState>,
    pub pending_events: usize,
}

#[tauri::command]
pub async fn sync_status(state: State<'_, AppState>) -> Result<SyncStatus, String> {
    let cfg = state.config.lock().await.clone();
    let account = cfg.gmail_account.clone();
    let sync_state = if let Some(account_email) = account.as_deref() {
        state
            .db
            .with_sync_state(|repo| repo.get(account_email))
            .map_err(|e| e.to_string())?
    } else {
        None
    };
    let pending = if let Some(account_email) = account.as_deref() {
        state
            .db
            .with_sync_state(|repo| repo.list_pending_events(account_email, 1000))
            .map(|rows| rows.len())
            .map_err(|e| e.to_string())?
    } else {
        0
    };

    Ok(SyncStatus {
        enabled: cfg.sync_enabled,
        relay_enabled: cfg.relay_enabled,
        account_email: account,
        state: sync_state,
        pending_events: pending,
    })
}

#[tauri::command]
pub async fn sync_replay_now(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<ReplayResult, String> {
    let cfg = state.config.lock().await.clone();
    let account_email = cfg
        .gmail_account
        .clone()
        .ok_or_else(|| "No Gmail account configured".to_string())?;
    let auth = load_gmail_auth(&app, &cfg)?;
    let llm = InferenceRouter::from_config(&cfg).with_database(state.db.clone());
    let mut gmail = GmailClient::new(auth);

    let db = Arc::new(state.db.clone());
    let processing_state = state.processing_state.clone();

    {
        let mut s = processing_state.lock().await;
        s.active_phase = "sync-fetching".into();
        s.current_progress = Some(OpProgress {
            source: "sync".into(),
            phase: "sync-fetching".into(),
            processed: 0,
            total: None,
            detail: Some("Checking Gmail history".into()),
            current_email_id: None,
            current_email_from: None,
            current_email_subject: None,
            current_email_sent_at: None,
        });
    }

    match replay_history(
        &db,
        &processing_state,
        &mut gmail,
        &llm,
        &cfg,
        &account_email,
        "manual",
        &|progress| {
            let _ = app.emit("sync-progress", &progress);
        },
    )
    .await
    {
        Ok(result) => {
            mark_last_successful(&db, &processing_state, Utc::now()).await;
            {
                let mut s = processing_state.lock().await;
                s.current_progress = None;
                s.active_phase = "idle".into();
            }
            let _ = app.emit(
                "sync-progress",
                OpProgress {
                    source: "sync".into(),
                    phase: "idle".into(),
                    processed: 0,
                    total: None,
                    detail: None,
                    current_email_id: None,
                    current_email_from: None,
                    current_email_subject: None,
                    current_email_sent_at: None,
                },
            );
            Ok(result)
        }
        Err(e) => {
            let msg = e.to_string();
            {
                let mut s = processing_state.lock().await;
                s.current_progress = None;
                s.active_phase = "error".into();
            }
            let _ = app.emit(
                "sync-progress",
                OpProgress {
                    source: "sync".into(),
                    phase: "error".into(),
                    processed: 0,
                    total: None,
                    detail: Some(msg.clone()),
                    current_email_id: None,
                    current_email_from: None,
                    current_email_subject: None,
                    current_email_sent_at: None,
                },
            );
            Err(msg)
        }
    }
}

#[tauri::command]
pub async fn sync_watch_start(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<post_office_core::gmail::models::WatchResponse, String> {
    let cfg = state.config.lock().await.clone();
    let account_email = cfg
        .gmail_account
        .clone()
        .ok_or_else(|| "No Gmail account configured".to_string())?;
    let topic = cfg.sync_watch_topic.trim();
    if topic.is_empty() {
        return Err("sync.watch.topic is empty".into());
    }

    let auth = load_gmail_auth(&app, &cfg)?;
    let mut gmail = GmailClient::new(auth);
    let labels = parse_watch_labels(&cfg.sync_watch_label_ids);
    start_watch(
        &Arc::new(state.db.clone()),
        &mut gmail,
        &account_email,
        topic,
        &labels,
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn sync_watch_stop(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let cfg = state.config.lock().await.clone();
    let account_email = cfg
        .gmail_account
        .clone()
        .ok_or_else(|| "No Gmail account configured".to_string())?;
    let auth = load_gmail_auth(&app, &cfg)?;
    let mut gmail = GmailClient::new(auth);
    stop_watch(&Arc::new(state.db.clone()), &mut gmail, &account_email)
        .await
        .map_err(|e| e.to_string())
}

fn parse_watch_labels(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToString::to_string)
        .collect()
}

/// One-off backfill: process mail in `[after, before]` (inclusive day bounds)
/// applying only the rules whose ids are in `rule_ids`. Emits `backfill-progress`
/// events so the UI can show a live status bar. Returns the number processed.
/// Does NOT change `processing.last_run` (the live resume point stays intact).
#[tauri::command]
pub async fn processing_backfill(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    after: String,
    before: String,
    rule_ids: Vec<i64>,
) -> Result<BackfillResult, String> {
    let after_dt = parse_day(&after).map_err(|e| format!("Invalid 'after' date: {}", e))?;
    let before_dt = parse_day_end(&before).map_err(|e| format!("Invalid 'before' date: {}", e))?;

    let (auth, llm) = {
        let config = state.config.lock().await;
        let auth = load_gmail_auth(&app, &config)?;
        let llm = InferenceRouter::from_config(&config).with_database(state.db.clone());
        (auth, llm)
    };

    let mut gmail = GmailClient::new(auth);
    let db = Arc::new(state.db.clone());
    let state_clone = state.processing_state.clone();
    let config = state.config.lock().await.clone();

    let cancel_flag = {
        let ps = state_clone.lock().await;
        if ps.backfill_running.swap(true, Ordering::SeqCst) {
            return Err("A backfill is already running".to_string());
        }
        ps.backfill_cancel_requested
            .store(false, std::sync::atomic::Ordering::Relaxed);
        ps.backfill_cancel_requested.clone()
    };

    let run_result = run_backfill(
        &db,
        &state_clone,
        &mut gmail,
        &llm,
        &config,
        after_dt,
        before_dt,
        &rule_ids,
        cancel_flag.as_ref(),
        &|progress| {
            let _ = app.emit("backfill-progress", &progress);
        },
    )
    .await;

    {
        let mut ps = state_clone.lock().await;
        ps.backfill_running
            .store(false, std::sync::atomic::Ordering::Relaxed);
        ps.backfill_cancel_requested
            .store(false, std::sync::atomic::Ordering::Relaxed);

        ps.active_phase = if run_result.is_ok() {
            "idle".into()
        } else {
            "error".into()
        };
        ps.current_progress = None;
    }

    match run_result {
        Ok(result) => {
            if !result.stopped {
                let _ = app.emit(
                    "backfill-progress",
                    OpProgress {
                        source: "backfill".into(),
                        phase: "idle".into(),
                        processed: result.processed,
                        total: Some(result.discovered),
                        detail: None,
                        current_email_id: None,
                        current_email_from: None,
                        current_email_subject: None,
                        current_email_sent_at: None,
                    },
                );
            }
            Ok(BackfillResult {
                processed: result.processed,
                discovered: result.discovered,
                stopped: result.stopped,
            })
        }
        Err(e) => {
            let msg = e.to_string();
            let _ = app.emit(
                "backfill-progress",
                OpProgress {
                    source: "backfill".into(),
                    phase: "error".into(),
                    processed: 0,
                    total: None,
                    detail: Some(msg.clone()),
                    current_email_id: None,
                    current_email_from: None,
                    current_email_subject: None,
                    current_email_sent_at: None,
                },
            );
            Err(msg)
        }
    }
}

#[derive(Debug, Serialize)]
pub struct BackfillResult {
    pub processed: usize,
    pub discovered: usize,
    pub stopped: bool,
}

#[tauri::command]
pub async fn processing_backfill_stop(state: State<'_, AppState>) -> Result<bool, String> {
    let ps = state.processing_state.lock().await;
    let running = ps
        .backfill_running
        .load(std::sync::atomic::Ordering::Relaxed);
    if running {
        ps.backfill_cancel_requested
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
    Ok(running)
}

/// Parse `YYYY-MM-DD` as start-of-day UTC.
fn parse_day(value: &str) -> Result<DateTime<Utc>, String> {
    let value = value.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Ok(dt.with_timezone(&Utc));
    }
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map(|d| DateTime::from_naive_utc_and_offset(d.and_hms_opt(0, 0, 0).unwrap(), Utc))
        .map_err(|e| e.to_string())
}

/// Parse `YYYY-MM-DD` as end-of-day (23:59:59) UTC, so a selected day is fully
/// included in the backfill window.
fn parse_day_end(value: &str) -> Result<DateTime<Utc>, String> {
    let value = value.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Ok(dt.with_timezone(&Utc));
    }
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map(|d| DateTime::from_naive_utc_and_offset(d.and_hms_opt(23, 59, 59).unwrap(), Utc))
        .map_err(|e| e.to_string())
}

/// Validates the LLM endpoint/model without touching Gmail or rules. Returns a
/// result object (never an invocation error) so the settings UI can show the
/// exact failure — a wrong base URL or model fails silently otherwise.
#[tauri::command]
pub async fn llm_test(
    base_url: String,
    api_key: String,
    model: String,
) -> Result<LlmTestResult, String> {
    let llm = LlmClient::new(&base_url, &api_key, &model);
    match llm.test_connection().await {
        Ok(resp) => Ok(LlmTestResult {
            ok: true,
            model: resp.model,
            error: None,
        }),
        Err(e) => Ok(LlmTestResult {
            ok: false,
            model: String::new(),
            error: Some(e.to_string()),
        }),
    }
}

#[tauri::command]
pub async fn llm_provider_test(
    state: State<'_, AppState>,
    provider_id: String,
) -> Result<LlmTestResult, String> {
    let config = state.config.lock().await.clone();
    let (base_url, api_key, model, timeout_secs) = if provider_id == "legacy" {
        (
            config.llm_base_url,
            config.llm_api_key,
            config.llm_default_model,
            config.llm_timeout_secs,
        )
    } else {
        let provider = config
            .llm_providers
            .iter()
            .find(|profile| profile.id == provider_id)
            .ok_or_else(|| format!("Provider not found: {provider_id}"))?;
        let key_ref = if provider.api_key_ref.trim().is_empty() {
            &provider.id
        } else {
            &provider.api_key_ref
        };
        let api_key =
            post_office_core::llm::credentials::load_provider_api_key(key_ref)?.unwrap_or_default();
        (
            provider.base_url.clone(),
            api_key,
            provider.model.clone(),
            provider.timeout_secs,
        )
    };

    let client =
        LlmClient::with_options(&base_url, &api_key, &model, timeout_secs, Some(provider_id));
    match client.test_connection().await {
        Ok(response) => Ok(LlmTestResult {
            ok: true,
            model: response.model,
            error: None,
        }),
        Err(error) => Ok(LlmTestResult {
            ok: false,
            model: String::new(),
            error: Some(error.to_string()),
        }),
    }
}

#[derive(Debug, serde::Serialize)]
pub struct LlmTestResult {
    pub ok: bool,
    pub model: String,
    pub error: Option<String>,
}

/// Lists the model ids an OpenAI-compatible endpoint exposes. Returns an error
/// string (not an invocation error) so the settings UI can fall back to a
/// free-text model field when the endpoint doesn't implement `/models`.
#[tauri::command]
pub async fn llm_list_models(base_url: String, api_key: String) -> Result<Vec<String>, String> {
    let llm = LlmClient::new(&base_url, &api_key, "");
    llm.list_models().await.map_err(|e| e.to_string())
}

fn read_oauth_json(app: &tauri::AppHandle) -> Option<serde_json::Value> {
    let dir = app.path().resource_dir().ok()?;
    let path = dir.join("google-oauth.json");
    let contents = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str::<serde_json::Value>(&contents).ok()
}

fn json_field(json: &Option<serde_json::Value>, key: &str) -> Option<String> {
    json.as_ref()
        .and_then(|v| v.get(key))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Resolve the Google OAuth client_id from, in priority order:
/// 1. POST_OFFICE_GOOGLE_CLIENT_ID env var
/// 2. bundled google-oauth.json (resource dir)
/// 3. google.client_id stored in app config (legacy override)
/// 4. client_id baked into the binary at build time
pub fn resolve_client_id(app: &tauri::AppHandle, config: &AppConfig) -> Result<String, String> {
    if let Ok(v) = std::env::var("POST_OFFICE_GOOGLE_CLIENT_ID") {
        if !v.trim().is_empty() {
            return Ok(v.trim().to_string());
        }
    }

    if let Some(id) = json_field(&read_oauth_json(app), "client_id") {
        return Ok(id);
    }

    if !config.google_client_id.trim().is_empty() {
        return Ok(config.google_client_id.trim().to_string());
    }

    let baked = post_office_core::gmail::oauth::BAKED_GOOGLE_CLIENT_ID;
    if !baked.trim().is_empty() {
        return Ok(baked.trim().to_string());
    }

    Err(
        "No Google OAuth client_id configured. Set POST_OFFICE_GOOGLE_CLIENT_ID or add a google-oauth.json next to the app."
            .to_string(),
    )
}

/// Resolve the Google OAuth client_secret. Google validates it even for Desktop
/// clients, so it is effectively required; PKCE still provides the verifier.
pub fn resolve_client_secret(app: &tauri::AppHandle) -> Option<String> {
    if let Ok(v) = std::env::var("POST_OFFICE_GOOGLE_CLIENT_SECRET") {
        if !v.trim().is_empty() {
            return Some(v.trim().to_string());
        }
    }

    if let Some(secret) = json_field(&read_oauth_json(app), "client_secret") {
        return Some(secret);
    }

    let baked = post_office_core::gmail::oauth::BAKED_GOOGLE_CLIENT_SECRET;
    if !baked.trim().is_empty() {
        return Some(baked.trim().to_string());
    }

    None
}

/// Loads a `GmailAuth` for the configured account using the *resolved* client
/// id (env → bundled json → baked-in), not `config.google_client_id`, which is
/// empty unless the user overrode it. `GmailAuth::load` rejects an empty
/// client_id, so using the raw config value here previously reported a working
/// connection as unauthenticated.
pub fn load_gmail_auth(app: &tauri::AppHandle, config: &AppConfig) -> Result<GmailAuth, String> {
    let account = config
        .gmail_account
        .as_ref()
        .ok_or("No Gmail account configured")?;
    let client_id = resolve_client_id(app, config)?;
    let secret = resolve_client_secret(app);
    GmailAuth::load(account, &client_id, secret.as_deref())
        .ok_or_else(|| "Gmail not authenticated".to_string())
}

#[tauri::command]
pub async fn gmail_authenticate(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let config = state.config.lock().await;
    let client_id = resolve_client_id(&app, &config)?;
    let client_secret = resolve_client_secret(&app);
    drop(config);

    // Google validates the secret even for Desktop clients, so an absent one only
    // yields an opaque 400 from the token endpoint. Fail early with a fix.
    let client_secret = client_secret.ok_or(
        "No Google OAuth client_secret configured. Google requires it even for Desktop clients; add it to google-oauth.json or set POST_OFFICE_GOOGLE_CLIENT_SECRET.",
    )?;

    let (verifier, challenge) = generate_pkce();
    let url = auth_url(&client_id, &challenge, "post-office");
    open::that(&url).map_err(|e| format!("Failed to open browser: {}", e))?;

    let code = listen_for_oauth_code()
        .await
        .map_err(|e| format!("OAuth callback failed: {}", e))?;

    let http = reqwest::Client::new();
    let token = exchange_code(&http, &client_id, &code, &verifier, Some(&client_secret))
        .await
        .map_err(|e| format!("Token exchange failed: {}", e))?;

    let access_token = token.access_token.clone();
    let refresh_token = token
        .refresh_token
        .ok_or("No refresh token returned (ensure prompt=consent)")?;

    let auth = GmailAuth::new(
        access_token.clone(),
        refresh_token.clone(),
        client_id,
        Some(client_secret),
        token.expires_in,
    );
    let mut gmail = GmailClient::new(auth);
    let profile = gmail
        .get_profile()
        .await
        .map_err(|e| format!("Failed to fetch profile: {}", e))?;

    store_tokens(&profile.email_address, &access_token, &refresh_token)
        .map_err(|e| format!("Failed to store tokens: {}", e))?;

    let sync_enabled = {
        let mut config = state.config.lock().await;
        config.gmail_account = Some(profile.email_address.clone());
        state
            .db
            .with_config(|repo| config.save(&repo))
            .map_err(|e| e.to_string())?;
        config.sync_enabled
    };

    if !sync_enabled {
        let cfg = state.config.lock().await.clone();
        ensure_polling_started(app.clone(), &state, &cfg);
    }

    let _ = state.sync_trigger.send(SyncTrigger::Startup);

    Ok(profile.email_address)
}

async fn listen_for_oauth_code() -> Result<String, String> {
    let listener = TcpListener::bind("127.0.0.1:7890")
        .await
        .map_err(|e| format!("Failed to bind localhost:7890: {}", e))?;

    let accept = async {
        let (mut stream, _) = listener.accept().await.map_err(|e| e.to_string())?;
        let mut buf = [0u8; 4096];
        let n = stream.read(&mut buf).await.map_err(|e| e.to_string())?;
        let request = String::from_utf8_lossy(&buf[..n]);
        let first_line = request.lines().next().ok_or("Empty request")?.to_string();

        let body = "<!doctype html><html><body style='font-family:sans-serif'><h2>Post Office</h2><p>Authentication complete. You can close this tab and return to the app.</p></body></html>";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.flush().await;

        Ok::<String, String>(first_line)
    };

    let first_line = tokio::time::timeout(std::time::Duration::from_secs(300), accept)
        .await
        .map_err(|_| "Timed out waiting for OAuth callback".to_string())??;

    let path = first_line
        .strip_prefix("GET ")
        .or_else(|| first_line.strip_prefix("GET"))
        .ok_or("Unexpected request")?;
    let path = path.split_whitespace().next().ok_or("Unexpected request")?;

    let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");
    let params: std::collections::HashMap<String, String> =
        url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();

    if let Some(err) = params.get("error") {
        let desc = params
            .get("error_description")
            .cloned()
            .unwrap_or_else(|| err.clone());
        return Err(format!("Google returned an error: {}", desc));
    }

    params
        .get("code")
        .cloned()
        .ok_or_else(|| "No authorization code in callback".to_string())
}

#[tauri::command]
pub async fn gmail_get_profile(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<post_office_core::gmail::models::GmailProfile, String> {
    let config = state.config.lock().await;
    let auth = load_gmail_auth(&app, &config)?;
    let mut gmail = GmailClient::new(auth);
    gmail.get_profile().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn gmail_list_labels(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<post_office_core::gmail::models::Label>, String> {
    let config = state.config.lock().await;
    let auth = load_gmail_auth(&app, &config)?;
    let mut gmail = GmailClient::new(auth);
    gmail.list_labels().await.map_err(|e| e.to_string())
}

/// Reports the *verified* Gmail connection state: it loads the stored tokens and
/// performs a live profile fetch, so `connected` reflects working auth rather
/// than just `gmail.account` being set in config.
#[tauri::command]
pub async fn gmail_connection_status(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<post_office_core::gmail::models::GmailConnection, String> {
    let config = state.config.lock().await;
    if config.gmail_account.is_none() {
        return Ok(post_office_core::gmail::models::GmailConnection {
            connected: false,
            email: None,
            messages_total: None,
            threads_total: None,
            error: None,
        });
    }

    let auth = match load_gmail_auth(&app, &config) {
        Ok(auth) => auth,
        Err(e) => {
            return Ok(post_office_core::gmail::models::GmailConnection {
                connected: false,
                email: None,
                messages_total: None,
                threads_total: None,
                error: Some(e),
            });
        }
    };

    let mut gmail = GmailClient::new(auth);
    match gmail.get_profile().await {
        Ok(profile) => Ok(post_office_core::gmail::models::GmailConnection {
            connected: true,
            email: Some(profile.email_address),
            messages_total: Some(profile.messages_total),
            threads_total: Some(profile.threads_total),
            error: None,
        }),
        Err(e) => Ok(post_office_core::gmail::models::GmailConnection {
            connected: true,
            email: None,
            messages_total: None,
            threads_total: None,
            error: Some(format!(
                "Gmail auth is no longer valid (token may be expired or revoked). Reconnect to fix: {}",
                e
            )),
        }),
    }
}

#[tauri::command]
pub async fn history_by_email(
    state: State<'_, AppState>,
    email_id: String,
) -> Result<Vec<post_office_core::db::history::HistoryEntry>, String> {
    state
        .db
        .with_history(|repo| repo.by_email(&email_id))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn tray_refresh(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let theme = state.config.lock().await.tray_theme.clone();
    tray::apply_tray_theme(&app, &theme);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_create_request_deserializes_conditions() {
        let json = serde_json::json!({
            "name": "t",
            "description": null,
            "conditions": [
                {"type": "from", "operator": "contains", "value": "mongo"},
                {"type": "subject", "operator": "not_contains", "value": "alert"}
            ],
            "prompt": "p",
            "actions": [{"type": "label", "value": "Mongo"}],
            "priority": 0,
            "enabled": true
        });
        let req: RuleCreateRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.conditions.len(), 2);
        assert_eq!(req.actions.len(), 1);
    }
}
