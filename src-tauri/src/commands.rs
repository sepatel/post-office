use post_office_core::config::AppConfig;
use post_office_core::db::rule_chat::ChatMessageRow;
use post_office_core::db::rules::{CreateRuleRequest, UpdateRuleRequest};
use post_office_core::gmail::models::Label;
use post_office_core::gmail::oauth::{auth_url, exchange_code, generate_pkce};
use post_office_core::gmail::{delete_tokens, store_tokens, GmailAuth, GmailClient};
use post_office_core::llm::{InferenceRouter, LlmClient};
use post_office_core::llm::{LlmProviderProfile, LlmRoutingPolicy};
use post_office_core::processing::{
    hydrate_processing_state, mark_last_successful, OpProgress, ProcessingState,
};
use post_office_core::rules::chat::{apply_proposal, chat_with_rule, ChatProposal, ChatTurn};
use post_office_core::rules::engine::{
    dry_run_pipeline, rules_after, ActionDisplay, FallthroughStep, PipelineDryRun,
    PipelineDryRunProgress, TestResult,
};
use post_office_core::rules::evaluation::EvaluationVerdict;
use post_office_core::rules::models::{Action, Condition, Rule};
use post_office_core::sync::{replay_history, start_watch, stop_watch, ReplayResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tauri::Emitter;
use tauri::Manager;
use tauri::State;
use tauri_plugin_notification::NotificationExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use chrono::Utc;

use crate::sync_runtime::SyncTrigger;
use crate::tray;
use crate::{AppState, PipelineDryRunStatus, ProcessingStates};

const WORKFLOW_WORKER_IDLE_SECS: u64 = 2;

pub fn spawn_message_workflow_worker(
    app: tauri::AppHandle,
    db: Arc<post_office_core::db::Database>,
    processing_states: ProcessingStates,
    config: Arc<tokio::sync::Mutex<AppConfig>>,
    inference_runtime: post_office_core::llm::InferenceRuntime,
) {
    tauri::async_runtime::spawn(async move {
        let mut attention_counts = HashMap::<String, i64>::new();
        loop {
            let cfg = config.lock().await.clone();
            let accounts = db
                .with_accounts(|repo| repo.list())
                .unwrap_or_default()
                .into_iter()
                .filter(|account| !account.paused)
                .collect::<Vec<_>>();
            let mut processed = false;

            for account in accounts {
                let account_email = account.email;
                let processing_state =
                    processing_state_for(&processing_states, &db, &account_email);
                let (
                    paused,
                    stop_requested,
                    stop_requested_flag,
                    workflow_running,
                    workflow_active_runs,
                ) = {
                    let processing = processing_state.lock().await;
                    (
                        processing.paused.load(Ordering::Relaxed),
                        processing.stop_requested.load(Ordering::Relaxed),
                        processing.stop_requested.clone(),
                        processing.workflow_running.clone(),
                        processing.workflow_active_runs.clone(),
                    )
                };
                if paused || stop_requested {
                    continue;
                }
                if workflow_active_runs.fetch_add(1, Ordering::AcqRel) == 0 {
                    workflow_running.store(true, Ordering::Release);
                }
                let auth = match load_gmail_auth_for_account(&app, &cfg, &account_email) {
                    Ok(auth) => auth,
                    Err(error) => {
                        let _ = db.with_accounts(|repo| repo.record_error(&account_email, &error));
                        tracing::warn!(
                            "Message workflow could not load Gmail credentials for {}: {}",
                            account_email,
                            error
                        );
                        if workflow_active_runs.fetch_sub(1, Ordering::AcqRel) == 1 {
                            workflow_running.store(false, Ordering::Release);
                        }
                        continue;
                    }
                };
                if stop_requested_flag.load(Ordering::Acquire) {
                    if workflow_active_runs.fetch_sub(1, Ordering::AcqRel) == 1 {
                        workflow_running.store(false, Ordering::Release);
                    }
                    continue;
                }
                let mut gmail = GmailClient::new(auth);

                let lease_token = post_office_core::workflow::new_lease_token();
                let next_run =
                    db.with_workflow(|repo| repo.claim_next_unrouted(&account_email, &lease_token));
                match next_run {
                    Ok(Some(run)) => {
                        processed = true;
                        if workflow_active_runs.fetch_add(1, Ordering::AcqRel) == 0 {
                            workflow_running.store(true, Ordering::Release);
                        }
                        let result = post_office_core::workflow::prepare_run(
                            &db,
                            &mut gmail,
                            &cfg,
                            inference_runtime.clone(),
                            run,
                        )
                        .await;
                        if workflow_active_runs.fetch_sub(1, Ordering::AcqRel) == 1 {
                            workflow_running.store(false, Ordering::Release);
                        }
                        if let Err(error) = result {
                            tracing::warn!(
                                "Message workflow preparation failed for {}: {}",
                                account_email,
                                error
                            );
                        }
                    }
                    Ok(None) => {}
                    Err(error) => tracing::warn!(
                        "Message workflow could not claim preparation work for {}: {}",
                        account_email,
                        error
                    ),
                }

                if stop_requested_flag.load(Ordering::Acquire) {
                    if workflow_active_runs.fetch_sub(1, Ordering::AcqRel) == 1 {
                        workflow_running.store(false, Ordering::Release);
                    }
                    continue;
                }

                let routes = db.with_workflow(|repo| repo.ready_routes(&account_email));
                match routes {
                    Ok(routes) => {
                        for route in routes {
                            while let Some(permit) = inference_runtime.try_acquire(
                                &route.endpoint,
                                &route.model,
                                route.max_concurrent_requests,
                            ) {
                                let lease_token = post_office_core::workflow::new_lease_token();
                                let run = match db.with_workflow(|repo| {
                                    repo.claim_next_for_route(
                                        &account_email,
                                        &route.endpoint,
                                        &route.model,
                                        &lease_token,
                                    )
                                }) {
                                    Ok(Some(run)) => run,
                                    Ok(None) => break,
                                    Err(error) => {
                                        tracing::warn!(
                                            "Message workflow could not claim routed work for {}: {}",
                                            account_email,
                                            error
                                        );
                                        break;
                                    }
                                };
                                processed = true;
                                if workflow_active_runs.fetch_add(1, Ordering::AcqRel) == 0 {
                                    workflow_running.store(true, Ordering::Release);
                                }
                                let task_db = Arc::clone(&db);
                                let task_config = cfg.clone();
                                let task_runtime = inference_runtime.clone();
                                let task_gmail = gmail.clone();
                                let task_route = route.clone();
                                let task_running = workflow_running.clone();
                                let task_active_runs = workflow_active_runs.clone();
                                tauri::async_runtime::spawn(async move {
                                    let mut task_gmail = task_gmail;
                                    let decision_runtime = task_runtime.clone();
                                    let renewal_run = run.clone();
                                    let decision = task_runtime.with_reservation(
                                        &task_route.endpoint,
                                        &task_route.model,
                                        permit,
                                        post_office_core::workflow::process_routed_run(
                                            &task_db,
                                            &mut task_gmail,
                                            &task_config,
                                            decision_runtime,
                                            run,
                                        ),
                                    );
                                    tokio::pin!(decision);
                                    let mut lease_renewal =
                                        tokio::time::interval(Duration::from_secs(60));
                                    lease_renewal.tick().await;
                                    let result = loop {
                                        tokio::select! {
                                            result = &mut decision => break result,
                                            _ = lease_renewal.tick() => {
                                                if !task_db.with_workflow(|repo| {
                                                    repo.renew_lease(
                                                        renewal_run.id,
                                                        &renewal_run.lease_token,
                                                    )
                                                }).unwrap_or(false) {
                                                    tracing::warn!("Message workflow lost its decision lease");
                                                }
                                            }
                                        }
                                    };
                                    if let Err(error) = result {
                                        tracing::warn!(
                                            "Message workflow decision failed: {}",
                                            error
                                        );
                                    }
                                    if task_active_runs.fetch_sub(1, Ordering::AcqRel) == 1 {
                                        task_running.store(false, Ordering::Release);
                                    }
                                });
                            }
                        }
                    }
                    Err(error) => tracing::warn!(
                        "Message workflow could not inspect routed work for {}: {}",
                        account_email,
                        error
                    ),
                }
                if workflow_active_runs.fetch_sub(1, Ordering::AcqRel) == 1 {
                    workflow_running.store(false, Ordering::Release);
                }

                let attention = db
                    .with_workflow(|repo| repo.state_counts(&account_email))
                    .ok()
                    .and_then(|counts| {
                        counts
                            .into_iter()
                            .find(|entry| entry.state == "needs_attention")
                            .map(|entry| entry.count)
                    })
                    .unwrap_or(0);
                if let Some(previous) = attention_counts.insert(account_email.clone(), attention) {
                    if attention > previous {
                        let _ = app
                            .notification()
                            .builder()
                            .title("Post Office needs attention")
                            .body(format!(
                                "{} message{} need attention in {}",
                                attention,
                                if attention == 1 { "" } else { "s" },
                                account_email
                            ))
                            .show();
                    }
                }
            }

            if !processed {
                tokio::time::sleep(Duration::from_secs(WORKFLOW_WORKER_IDLE_SECS)).await;
            }
        }
    });
}

pub(crate) fn processing_state_for(
    processing_states: &ProcessingStates,
    db: &post_office_core::db::Database,
    account_email: &str,
) -> Arc<tokio::sync::Mutex<ProcessingState>> {
    let mut states = processing_states.lock().unwrap();
    states
        .entry(account_email.to_string())
        .or_insert_with(|| {
            let mut processing = ProcessingState::new();
            hydrate_processing_state(db, &mut processing, account_email);
            if let Ok(Some(account)) = db.with_accounts(|repo| repo.get(account_email)) {
                processing.paused.store(account.paused, Ordering::Relaxed);
            } else {
                processing.stop_requested.store(true, Ordering::Relaxed);
            }
            Arc::new(tokio::sync::Mutex::new(processing))
        })
        .clone()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RuleCreateRequest {
    pub name: String,
    pub description: Option<String>,
    pub conditions: Vec<Condition>,
    pub prompt: String,
    #[serde(default)]
    pub choices: Vec<Action>,
    #[serde(default)]
    pub choose_from_all_labels: bool,
    pub actions: Vec<Action>,
    pub priority: i32,
    pub enabled: bool,
    #[serde(default = "default_policy")]
    pub inference_policy: String,
    #[serde(default = "default_decision_reasoning_effort")]
    pub decision_reasoning_effort: post_office_core::llm::ReasoningEffort,
    #[serde(default)]
    pub decision_max_tokens: Option<u32>,
    #[serde(default)]
    pub continue_after_match: bool,
    #[serde(default)]
    pub source_rule_id: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RuleUpdateRequest {
    pub name: String,
    pub description: Option<String>,
    pub conditions: Vec<Condition>,
    pub prompt: String,
    #[serde(default)]
    pub choices: Vec<Action>,
    #[serde(default)]
    pub choose_from_all_labels: bool,
    pub actions: Vec<Action>,
    pub priority: i32,
    pub enabled: bool,
    #[serde(default = "default_policy")]
    pub inference_policy: String,
    #[serde(default = "default_decision_reasoning_effort")]
    pub decision_reasoning_effort: post_office_core::llm::ReasoningEffort,
    #[serde(default)]
    pub decision_max_tokens: Option<u32>,
    #[serde(default)]
    pub continue_after_match: bool,
    #[serde(default)]
    pub source_rule_id: Option<i64>,
}

fn default_policy() -> String {
    "default".into()
}

async fn publish_current_rule_set(state: &AppState, account_email: &str) -> Result<(), String> {
    let config = state.config.lock().await.clone();
    post_office_core::workflow::publish_rule_set(&state.db, account_email, &config)
        .map_err(|error| error.to_string())
}

async fn publish_all_rule_sets(state: &AppState) -> Result<(), String> {
    let config = state.config.lock().await.clone();
    let accounts = state
        .db
        .with_accounts(|repo| repo.list())
        .map_err(|error| error.to_string())?;
    for account in accounts {
        post_office_core::workflow::publish_rule_set(&state.db, &account.email, &config)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn default_decision_reasoning_effort() -> post_office_core::llm::ReasoningEffort {
    post_office_core::llm::ReasoningEffort::Off
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
        id: rule.source_rule_id.unwrap_or(0),
        name: rule.name.clone(),
        description: rule.description.clone(),
        conditions: rule.conditions.clone(),
        prompt: rule.prompt.clone(),
        choices: rule.choices.clone(),
        choose_from_all_labels: rule.choose_from_all_labels,
        actions: rule.actions.clone(),
        priority: rule.priority,
        enabled: rule.enabled,
        parent_id: None,
        inference_policy: if rule.inference_policy.trim().is_empty() {
            default_policy()
        } else {
            rule.inference_policy.clone()
        },
        decision_reasoning_effort: rule.decision_reasoning_effort,
        decision_max_tokens: rule.decision_max_tokens,
        continue_after_match: rule.continue_after_match,
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
        if let Action::Label { value } | Action::RemoveLabel { value } = action {
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
        if let Action::Label { value } | Action::RemoveLabel { value } = action {
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
        .any(|action| matches!(action, Action::Label { .. } | Action::RemoveLabel { .. }))
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
    state: State<'_, AppState>,
    key: String,
    value: String,
) -> Result<(), String> {
    let sync_related = key.starts_with("sync.");
    let llm_related = key.starts_with("llm.");
    let mut config = state.config.lock().await;
    match key.as_str() {
        "gmail.account" => config.gmail_account = if value.is_empty() { None } else { Some(value) },
        "llm.base_url" => config.llm_base_url = value,
        "llm.api_key" => {
            post_office_core::llm::credentials::store_provider_api_key("legacy", &value)?;
            config.llm_api_key.clear();
        }
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
        "llm.context_window_tokens" => {
            let val: u32 = value
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            config.llm_context_window_tokens = val;
        }
        "llm.legacy_name" => config.llm_legacy_name = value,
        "llm.legacy_quality_tier" => config.llm_legacy_quality_tier = value,
        "llm.legacy_privacy_status" => config.llm_legacy_privacy_status = value,
        "llm.legacy_enabled" => {
            let val: bool = value
                .parse()
                .map_err(|e: std::str::ParseBoolError| e.to_string())?;
            config.llm_legacy_enabled = val;
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
    drop(config);
    if sync_related {
        let _ = state.sync_trigger.send(SyncTrigger::Startup);
    }
    if llm_related {
        publish_all_rule_sets(&state).await?;
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct AccountsState {
    pub accounts: Vec<post_office_core::db::accounts::Account>,
    pub active_email: Option<String>,
}

#[tauri::command]
pub async fn accounts_list(state: State<'_, AppState>) -> Result<AccountsState, String> {
    let active_email = state.config.lock().await.gmail_account.clone();
    let accounts = state
        .db
        .with_accounts(|repo| repo.list())
        .map_err(|error| error.to_string())?;
    Ok(AccountsState {
        accounts,
        active_email,
    })
}

#[tauri::command]
pub async fn accounts_select(state: State<'_, AppState>, email: String) -> Result<(), String> {
    let exists = state
        .db
        .with_accounts(|repo| repo.get(&email))
        .map_err(|error| error.to_string())?
        .is_some();
    if !exists {
        return Err("Account not found".into());
    }

    let mut config = state.config.lock().await;
    config.gmail_account = Some(email);
    state
        .db
        .with_config(|repo| config.save(&repo))
        .map_err(|error| error.to_string())?;
    drop(config);
    publish_all_rule_sets(&state).await
}

#[tauri::command]
pub async fn accounts_set_paused(
    state: State<'_, AppState>,
    email: String,
    paused: bool,
) -> Result<(), String> {
    state
        .db
        .with_accounts(|repo| repo.set_paused(&email, paused))
        .map_err(|error| error.to_string())?;
    let processing_state = state.processing_state_for(&email);
    processing_state
        .lock()
        .await
        .paused
        .store(paused, std::sync::atomic::Ordering::Relaxed);
    if paused {
        state.inference_runtime.cancel_all();
    }

    Ok(())
}

#[tauri::command]
pub async fn accounts_reorder(
    state: State<'_, AppState>,
    emails: Vec<String>,
) -> Result<(), String> {
    state
        .db
        .with_accounts(|repo| repo.reorder(&emails))
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn accounts_remove(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    email: String,
) -> Result<(), String> {
    let exists = state
        .db
        .with_accounts(|repo| repo.get(&email))
        .map_err(|error| error.to_string())?
        .is_some();
    if !exists {
        return Err("Account not found".into());
    }

    let processing_state = state.processing_state_for(&email);
    {
        let processing = processing_state.lock().await;
        processing
            .stop_requested
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
    state.inference_runtime.cancel_all();
    let stopped = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let active_runs = processing_state
                .lock()
                .await
                .workflow_active_runs
                .load(std::sync::atomic::Ordering::Acquire);
            if active_runs == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    if stopped.is_err() {
        processing_state
            .lock()
            .await
            .stop_requested
            .store(false, Ordering::Relaxed);
        return Err("Timed out waiting for active mail processing to stop".into());
    }

    let config = state.config.lock().await.clone();
    if let Ok(auth) = load_gmail_auth_for_account(&app, &config, &email) {
        let mut gmail = GmailClient::new(auth);
        let _ = stop_watch(&Arc::new(state.db.clone()), &mut gmail, &email).await;
    }

    state
        .db
        .with_accounts(|repo| repo.delete_with_data(&email))
        .map_err(|error| error.to_string())?;
    delete_tokens(&email).map_err(|error| error.to_string())?;
    state.remove_processing_state(&email);

    let next_active = state
        .db
        .with_accounts(|repo| repo.list())
        .map_err(|error| error.to_string())?
        .into_iter()
        .next()
        .map(|account| account.email);
    let mut config = state.config.lock().await;
    config.gmail_account = next_active;
    state
        .db
        .with_config(|repo| config.save(&repo))
        .map_err(|error| error.to_string())
}

#[derive(Debug, Deserialize)]
pub struct LlmConfigUpdate {
    pub base_url: String,
    pub api_key: String,
    pub default_model: String,
    pub input_cost_per_million_usd: f64,
    pub output_cost_per_million_usd: f64,
    pub max_tokens: u32,
    pub timeout_secs: u64,
    pub context_window_tokens: u32,
    pub legacy_max_concurrent_requests: u8,
    pub legacy_output_tokens_per_second: f64,
    pub legacy_chat_reasoning_effort: post_office_core::llm::ReasoningEffort,
    pub legacy_name: String,
    pub legacy_quality_tier: String,
    pub legacy_privacy_status: String,
    pub legacy_enabled: bool,
    pub providers: Vec<LlmProviderProfile>,
    pub routing_policies: Vec<LlmRoutingPolicy>,
    pub default_policy: String,
}

#[tauri::command]
pub async fn llm_config_set(
    state: State<'_, AppState>,
    update: LlmConfigUpdate,
) -> Result<(), String> {
    post_office_core::llm::credentials::store_provider_api_key("legacy", &update.api_key)?;
    let mut config = state.config.lock().await;
    config.llm_base_url = update.base_url;
    config.llm_api_key.clear();
    config.llm_default_model = update.default_model;
    config.llm_input_cost_per_million_usd = update.input_cost_per_million_usd;
    config.llm_output_cost_per_million_usd = update.output_cost_per_million_usd;
    config.llm_max_tokens = update.max_tokens.clamp(64, 8_192);
    config.llm_timeout_secs = update.timeout_secs;
    config.llm_context_window_tokens = update.context_window_tokens;
    config.llm_legacy_max_concurrent_requests = update.legacy_max_concurrent_requests;
    config.llm_legacy_output_tokens_per_second = update.legacy_output_tokens_per_second;
    config.llm_legacy_chat_reasoning_effort = update.legacy_chat_reasoning_effort;
    config.llm_legacy_name = update.legacy_name;
    config.llm_legacy_quality_tier = update.legacy_quality_tier;
    config.llm_legacy_privacy_status = update.legacy_privacy_status;
    config.llm_legacy_enabled = update.legacy_enabled;
    config.llm_providers = update.providers;
    config.llm_routing_policies = update.routing_policies;
    config.llm_default_policy = update.default_policy;
    let updated_config = config.clone();
    state
        .db
        .with_config(|repo| config.save(&repo))
        .map_err(|error| error.to_string())?;
    drop(config);
    post_office_core::llm::configure_runtime(&updated_config, &state.inference_runtime);
    publish_all_rule_sets(&state).await
}

#[tauri::command]
pub async fn rules_list(
    state: State<'_, AppState>,
) -> Result<Vec<post_office_core::rules::models::Rule>, String> {
    let account_email = active_account(&*state.config.lock().await)?;
    state
        .db
        .with_rules(|repo| repo.list_all(&account_email))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rules_create(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    mut rule: RuleCreateRequest,
) -> Result<post_office_core::rules::models::Rule, String> {
    let account_email = active_account(&*state.config.lock().await)?;
    if conditions_need_label_resolution(&rule.conditions)
        || actions_need_label_resolution(&rule.actions)
    {
        let labels = account_labels(&app, &state, &account_email).await?;
        normalize_rule_label_fields(&mut rule.conditions, &mut rule.actions, &labels)?;
    }

    let create = CreateRuleRequest {
        name: rule.name,
        description: rule.description,
        conditions: rule.conditions,
        prompt: rule.prompt,
        choices: rule.choices,
        choose_from_all_labels: rule.choose_from_all_labels,
        actions: rule.actions,
        priority: rule.priority,
        enabled: rule.enabled,
        inference_policy: if rule.inference_policy.trim().is_empty() {
            default_policy()
        } else {
            rule.inference_policy
        },
        decision_reasoning_effort: rule.decision_reasoning_effort,
        decision_max_tokens: rule.decision_max_tokens,
        continue_after_match: rule.continue_after_match,
    };
    let created = state
        .db
        .with_rules(|repo| repo.create(&account_email, &create))
        .map_err(|e| e.to_string())?;
    publish_current_rule_set(&state, &account_email).await?;
    Ok(created)
}

#[tauri::command]
pub async fn rules_update(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: i64,
    mut rule: RuleUpdateRequest,
) -> Result<post_office_core::rules::models::Rule, String> {
    let account_email = active_account(&*state.config.lock().await)?;
    if conditions_need_label_resolution(&rule.conditions)
        || actions_need_label_resolution(&rule.actions)
    {
        let labels = account_labels(&app, &state, &account_email).await?;
        normalize_rule_label_fields(&mut rule.conditions, &mut rule.actions, &labels)?;
    }

    let update = UpdateRuleRequest {
        name: rule.name,
        description: rule.description,
        conditions: rule.conditions,
        prompt: rule.prompt,
        choices: rule.choices,
        choose_from_all_labels: rule.choose_from_all_labels,
        actions: rule.actions,
        priority: rule.priority,
        enabled: rule.enabled,
        inference_policy: if rule.inference_policy.trim().is_empty() {
            default_policy()
        } else {
            rule.inference_policy
        },
        decision_reasoning_effort: rule.decision_reasoning_effort,
        decision_max_tokens: rule.decision_max_tokens,
        continue_after_match: rule.continue_after_match,
    };
    let updated = state
        .db
        .with_rules(|repo| repo.update(&account_email, id, &update))
        .map_err(|e| e.to_string())?;
    publish_current_rule_set(&state, &account_email).await?;
    Ok(updated)
}

#[tauri::command]
pub async fn rules_delete(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let account_email = active_account(&*state.config.lock().await)?;
    state
        .db
        .with_rules(|repo| repo.delete(&account_email, id))
        .map_err(|e| e.to_string())?;
    publish_current_rule_set(&state, &account_email).await
}

#[tauri::command]
pub async fn rules_reorder(state: State<'_, AppState>, ids: Vec<i64>) -> Result<(), String> {
    let account_email = active_account(&*state.config.lock().await)?;
    state
        .db
        .with_rules(|repo| repo.reorder(&account_email, &ids))
        .map_err(|e| e.to_string())?;
    publish_current_rule_set(&state, &account_email).await
}

// Portable backup for moving to a different computer: rules plus their
// memories and the non-secret LLM provider/routing configuration. Secret keys
// stay in the OS keyring and Gmail auth is per-machine, so both are excluded
// and must be reconnected on the new computer.
#[tauri::command]
pub async fn backup_export(
    state: State<'_, AppState>,
) -> Result<post_office_core::backup::BackupDoc, String> {
    let config = state.config.lock().await.clone();
    let account_email = active_account(&config)?;
    post_office_core::backup::build_export(&state.db, &account_email, &config)
}

#[tauri::command]
pub async fn backup_import(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    payload: String,
) -> Result<post_office_core::backup::ImportResult, String> {
    let doc = post_office_core::backup::parse_backup(&payload)?;
    let account_email = active_account(&*state.config.lock().await)?;
    // Best-effort live labels so label references resolve by name when ids
    // differ on the new machine; falls back to the cached labels offline.
    let live_labels = match account_labels(&app, &state, &account_email).await {
        Ok(labels) => labels,
        Err(_) => state
            .db
            .with_labels(|repo| repo.get(&account_email))
            .map_err(|e| e.to_string())?
            .map(|cached| cached.labels)
            .unwrap_or_default(),
    };
    let mut config = state.config.lock().await;
    let result = post_office_core::backup::apply_import(
        &state.db,
        &account_email,
        &mut config,
        doc,
        &live_labels,
    )?;
    let config_snapshot = config.clone();
    post_office_core::workflow::publish_rule_set(&state.db, &account_email, &config_snapshot)
        .map_err(|e| e.to_string())?;
    Ok(result)
}

#[tauri::command]
pub async fn rule_chat_history(
    state: State<'_, AppState>,
    rule_id: i64,
) -> Result<Vec<ChatMessageRow>, String> {
    let account_email = active_account(&*state.config.lock().await)?;
    state
        .db
        .with_rules(|repo| repo.get_by_id(&account_email, rule_id))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Rule not found".to_string())?;
    state
        .db
        .with_rule_chat(|repo| repo.list(rule_id))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rule_memories_list(
    state: State<'_, AppState>,
    rule_id: i64,
) -> Result<Vec<post_office_core::db::rule_memory::MemoryEntry>, String> {
    let account_email = active_account(&*state.config.lock().await)?;
    state
        .db
        .with_rules(|repo| repo.get_by_id(&account_email, rule_id))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Rule not found".to_string())?;
    state
        .db
        .with_rule_memory(|repo| repo.list_for_rule(rule_id))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rule_memory_delete(
    state: State<'_, AppState>,
    rule_id: i64,
    memory_id: i64,
) -> Result<(), String> {
    let account_email = active_account(&*state.config.lock().await)?;
    state
        .db
        .with_rules(|repo| repo.get_by_id(&account_email, rule_id))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Rule not found".to_string())?;
    state
        .db
        .with_rule_memory(|repo| repo.delete(rule_id, memory_id))
        .map_err(|e| e.to_string())?;
    publish_current_rule_set(&state, &account_email).await
}

#[tauri::command]
pub async fn rule_chat_send(
    state: State<'_, AppState>,
    rule_id: i64,
    message: String,
) -> Result<ChatTurn, String> {
    let account_email = active_account(&*state.config.lock().await)?;
    let llm = {
        let config = state.config.lock().await;
        InferenceRouter::from_config_with_runtime(&config, state.inference_runtime.clone())
            .with_database(state.db.clone())
    };

    let turn = chat_with_rule(&state.db, &llm, &account_email, rule_id, &message)
        .await
        .map_err(|e| e.to_string())?;
    publish_current_rule_set(&state, &account_email).await?;
    Ok(turn)
}

#[tauri::command]
pub async fn rule_apply_proposal(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    rule_id: i64,
    mut proposal: ChatProposal,
) -> Result<(), String> {
    let account_email = active_account(&*state.config.lock().await)?;
    if actions_need_label_resolution(&proposal.actions_add) {
        let labels = account_labels(&app, &state, &account_email).await?;
        normalize_proposal_label_actions(&mut proposal, &labels)?;
    }

    apply_proposal(&state.db, &account_email, rule_id, &proposal)
        .await
        .map_err(|e| e.to_string())?;
    publish_current_rule_set(&state, &account_email).await
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
    let (account_email, auth, llm) = {
        let config = state.config.lock().await;
        let account_email = active_account(&config)?;
        let auth = load_gmail_auth(&app, &config)?;
        let llm =
            InferenceRouter::from_config_with_runtime(&config, state.inference_runtime.clone())
                .with_database(state.db.clone());
        (account_email, auth, llm)
    };

    let mut gmail = GmailClient::new(auth);
    let labels = post_office_core::gmail::cached_labels(&state.db, &mut gmail, &account_email)
        .await
        .map_err(|e| e.to_string())?;
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

    let mut result = post_office_core::rules::engine::test_rule(
        &llm,
        &rule_model,
        &email,
        &rule_memories(&state, rule_model.id),
        &labels,
    )
    .await
    .map_err(|e| e.to_string())?;

    // A decision rule that declines hands the email to the next rule, so the
    // tester has to walk the same chain or it misrepresents what would happen.
    if !result.matched && !result.indeterminate {
        let rules = state
            .db
            .with_rules(|repo| repo.list_all(&account_email))
            .unwrap_or_default();
        for candidate in rules_after(&rules, rule_model.id, &email) {
            let step = post_office_core::rules::engine::test_rule(
                &llm,
                candidate,
                &email,
                &rule_memories(&state, candidate.id),
                &labels,
            )
            .await
            .map_err(|e| e.to_string())?;
            // Mirror the pipeline: a match ends the chain unless the rule opts
            // into continuing, and an unvalidated decision ends it outright.
            let stop = step.indeterminate || (step.matched && !candidate.continue_after_match);
            result
                .fallthrough
                .push(FallthroughStep::new(candidate, &step));
            if stop {
                break;
            }
        }
    }

    Ok(result)
}

fn rule_memories(state: &AppState, rule_id: i64) -> Vec<String> {
    state.db.with_rule_memory(|repo| {
        repo.list_for_rule(rule_id)
            .map(|rows| rows.into_iter().map(|entry| entry.text).collect())
            .unwrap_or_default()
    })
}

/// Evaluates a rule against loaded emails one at a time and returns the proposed
/// action per email. No emails are mutated — the frontend applies chosen verdicts
/// via `rules_apply`.
#[tauri::command]
pub async fn evaluate_messages(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    mut rule: RuleCreateRequest,
    message_ids: Vec<String>,
) -> Result<Vec<EvaluationVerdict>, String> {
    let (account_email, auth, llm) = {
        let config = state.config.lock().await;
        let account_email = active_account(&config)?;
        let auth = load_gmail_auth(&app, &config)?;
        let llm =
            InferenceRouter::from_config_with_runtime(&config, state.inference_runtime.clone())
                .with_database(state.db.clone());
        (account_email, auth, llm)
    };

    let mut gmail = GmailClient::new(auth);
    let labels = post_office_core::gmail::cached_labels(&state.db, &mut gmail, &account_email)
        .await
        .map_err(|e| e.to_string())?;
    if conditions_need_label_resolution(&rule.conditions)
        || actions_need_label_resolution(&rule.actions)
    {
        normalize_rule_label_fields(&mut rule.conditions, &mut rule.actions, &labels)?;
    }

    let mut emails = Vec::new();
    for id in &message_ids {
        if let Ok(msg) = gmail.get_message(id).await {
            emails.push(msg);
        }
    }

    let rule_model = build_rule_model(&rule);

    let memories = state.db.with_rule_memory(|repo| {
        repo.list_for_rule(rule_model.id)
            .map(|rows| rows.into_iter().map(|entry| entry.text).collect::<Vec<_>>())
            .unwrap_or_default()
    });

    post_office_core::rules::evaluation::evaluate_messages(
        &llm,
        &rule_model,
        &emails,
        &memories,
        &labels,
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rules_apply(
    _app: tauri::AppHandle,
    _state: State<'_, AppState>,
    _rule: RuleCreateRequest,
    _message_id: String,
) -> Result<ApplyResult, String> {
    Err("Direct Gmail application was removed. Queue a message run instead.".into())
}

#[tauri::command]
pub async fn workflow_queue_summary(
    state: State<'_, AppState>,
) -> Result<post_office_core::workflow::QueueSummary, String> {
    let account_email = active_account(&*state.config.lock().await)?;
    post_office_core::workflow::queue_summary(&state.db, &account_email)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn workflow_messages_list(
    state: State<'_, AppState>,
    run_state: Option<String>,
    page: u32,
    per_page: u32,
) -> Result<Vec<post_office_core::workflow::QueueItem>, String> {
    let account_email = active_account(&*state.config.lock().await)?;
    post_office_core::workflow::queue_items(
        &state.db,
        &account_email,
        run_state.as_deref(),
        page,
        per_page,
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn workflow_message_get(
    state: State<'_, AppState>,
    message_id: i64,
) -> Result<Option<post_office_core::workflow::MessageDetail>, String> {
    let account_email = active_account(&*state.config.lock().await)?;
    post_office_core::workflow::message_detail(&state.db, &account_email, message_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn workflow_retry_now(state: State<'_, AppState>, run_id: i64) -> Result<bool, String> {
    let account_email = active_account(&*state.config.lock().await)?;
    post_office_core::workflow::retry_now(&state.db, &account_email, run_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn workflow_retry_with_current_rules(
    state: State<'_, AppState>,
    run_id: i64,
) -> Result<bool, String> {
    let account_email = active_account(&*state.config.lock().await)?;
    publish_current_rule_set(&state, &account_email).await?;
    post_office_core::workflow::retry_with_current_rule_set(&state.db, &account_email, run_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn workflow_rule_set_status(
    state: State<'_, AppState>,
) -> Result<Vec<post_office_core::db::workflow::WorkflowRuleSetStatus>, String> {
    let account_email = active_account(&*state.config.lock().await)?;
    post_office_core::workflow::rule_set_status(&state.db, &account_email)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn workflow_endpoint_status(
    state: State<'_, AppState>,
) -> Result<Vec<post_office_core::db::llm_endpoint_status::LlmEndpointStatus>, String> {
    state
        .db
        .with_llm_endpoint_status(|repo| repo.list())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn history_list(
    state: State<'_, AppState>,
    page: u32,
    per_page: u32,
) -> Result<Vec<post_office_core::db::history::HistoryEntry>, String> {
    let account_email = active_account(&*state.config.lock().await)?;
    state
        .db
        .with_history(|repo| repo.list(&account_email, page, per_page))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rules_metrics(
    state: State<'_, AppState>,
) -> Result<Vec<post_office_core::db::history::RuleMetrics>, String> {
    let account_email = active_account(&*state.config.lock().await)?;
    state
        .db
        .with_history(|repo| repo.rules_metrics(&account_email))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rule_roi_metrics(
    state: State<'_, AppState>,
) -> Result<Vec<post_office_core::db::llm_usage::RuleRoiMetrics>, String> {
    let account_email = active_account(&*state.config.lock().await)?;
    state
        .db
        .with_llm_usage(|repo| repo.rule_roi_metrics(&account_email))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rule_request_metrics(
    state: State<'_, AppState>,
) -> Result<Vec<post_office_core::db::llm_requests::RuleRequestMetrics>, String> {
    let account_email = active_account(&*state.config.lock().await)?;
    state
        .db
        .with_llm_requests(|repo| repo.rule_metrics(&account_email))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rule_activity(
    state: State<'_, AppState>,
    rule_id: i64,
    page: u32,
    per_page: u32,
) -> Result<Vec<post_office_core::db::history::HistoryEntry>, String> {
    let account_email = active_account(&*state.config.lock().await)?;
    state
        .db
        .with_history(|repo| repo.by_rule(&account_email, rule_id, page, per_page))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn history_search(
    state: State<'_, AppState>,
    query: String,
) -> Result<Vec<post_office_core::db::history::HistoryEntry>, String> {
    let account_email = active_account(&*state.config.lock().await)?;
    state
        .db
        .with_history(|repo| repo.search(&account_email, &query))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn inference_jobs_list(
    state: State<'_, AppState>,
    page: u32,
    per_page: u32,
) -> Result<Vec<post_office_core::db::inference::InferenceJob>, String> {
    let account_email = active_account(&*state.config.lock().await)?;
    state
        .db
        .with_inference(|repo| repo.list(&account_email, page, per_page))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rule_inference_jobs(
    state: State<'_, AppState>,
    rule_id: i64,
    page: u32,
    per_page: u32,
) -> Result<Vec<post_office_core::db::inference::InferenceJob>, String> {
    let account_email = active_account(&*state.config.lock().await)?;
    state
        .db
        .with_inference(|repo| repo.list_for_rule(&account_email, rule_id, page, per_page))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn inference_job_attempts(
    state: State<'_, AppState>,
    job_id: i64,
) -> Result<Vec<post_office_core::db::inference::InferenceAttempt>, String> {
    let account_email = active_account(&*state.config.lock().await)?;
    state
        .db
        .with_inference(|repo| repo.attempts(&account_email, job_id))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn inference_job_retry(state: State<'_, AppState>, job_id: i64) -> Result<bool, String> {
    let _ = (state, job_id);
    Err("Legacy inference jobs cannot be retried by the message workflow.".into())
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
    let account_email = active_account(&*state.config.lock().await)?;
    let polling_enabled = state.config.lock().await.polling_enabled;
    let processing_state = state.processing_state_for(&account_email);
    let ps = processing_state.lock().await;
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
    let account_email = active_account(&*state.config.lock().await)?;
    state
        .db
        .with_accounts(|repo| repo.set_paused(&account_email, true))
        .map_err(|e| e.to_string())?;
    let processing_state = state.processing_state_for(&account_email);
    let ps = processing_state.lock().await;
    ps.paused.store(true, std::sync::atomic::Ordering::Relaxed);
    state.inference_runtime.cancel_all();
    Ok(())
}

#[tauri::command]
pub async fn processing_resume(state: State<'_, AppState>) -> Result<(), String> {
    let config = state.config.lock().await.clone();
    let account_email = active_account(&config)?;
    state
        .db
        .with_accounts(|repo| repo.set_paused(&account_email, false))
        .map_err(|e| e.to_string())?;
    let processing_state = state.processing_state_for(&account_email);
    let ps = processing_state.lock().await;
    ps.paused.store(false, std::sync::atomic::Ordering::Relaxed);
    drop(ps);
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
    let llm = InferenceRouter::from_config_with_runtime(&cfg, state.inference_runtime.clone())
        .with_database(state.db.clone());
    let mut gmail = GmailClient::new(auth);

    let db = Arc::new(state.db.clone());
    let processing_state = state.processing_state_for(&account_email);

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
            mark_last_successful(&db, &processing_state, &account_email, Utc::now()).await;
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
    _app: tauri::AppHandle,
    _state: State<'_, AppState>,
    _after: String,
    _before: String,
    _rule_ids: Vec<i64>,
) -> Result<BackfillResult, String> {
    Err(
        "Legacy backfill was removed. The message workflow only accepts explicit queue imports."
            .into(),
    )
}

#[derive(Debug, Serialize)]
pub struct BackfillResult {
    pub processed: usize,
    pub discovered: usize,
    pub stopped: bool,
}

#[tauri::command]
pub async fn processing_backfill_stop(state: State<'_, AppState>) -> Result<bool, String> {
    let _ = state;
    Ok(false)
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
            duration_ms: Some(resp.duration_ms),
            completion_tokens: resp.completion_tokens,
        }),
        Err(e) => Ok(LlmTestResult {
            ok: false,
            model: String::new(),
            error: Some(e.user_message()),
            duration_ms: None,
            completion_tokens: None,
        }),
    }
}

#[derive(Debug, Deserialize)]
pub struct LlmProviderTestProfile {
    pub id: String,
    pub base_url: String,
    pub model: String,
    pub api_key_ref: String,
    pub timeout_secs: u64,
    #[serde(default = "default_provider_concurrency")]
    pub max_concurrent_requests: u8,
}

#[tauri::command]
pub async fn llm_provider_test(
    state: State<'_, AppState>,
    provider: LlmProviderTestProfile,
    api_key: Option<String>,
) -> Result<LlmTestResult, String> {
    let api_key = if let Some(api_key) = api_key {
        api_key
    } else {
        let key_ref = if provider.api_key_ref.trim().is_empty() {
            &provider.id
        } else {
            &provider.api_key_ref
        };
        post_office_core::llm::credentials::load_provider_api_key(key_ref)?.unwrap_or_default()
    };

    let client = LlmClient::with_options(
        &provider.base_url,
        &api_key,
        &provider.model,
        provider.timeout_secs,
        Some(provider.id),
    );
    let _permit = state
        .inference_runtime
        .acquire(
            &provider
                .base_url
                .trim()
                .trim_end_matches('/')
                .to_ascii_lowercase(),
            &provider.model,
            provider.max_concurrent_requests as usize,
        )
        .await;
    match client.test_connection().await {
        Ok(response) => Ok(LlmTestResult {
            ok: true,
            model: response.model,
            error: None,
            duration_ms: Some(response.duration_ms),
            completion_tokens: response.completion_tokens,
        }),
        Err(error) => Ok(LlmTestResult {
            ok: false,
            model: String::new(),
            error: Some(error.user_message()),
            duration_ms: None,
            completion_tokens: None,
        }),
    }
}

fn default_provider_concurrency() -> u8 {
    1
}

#[derive(Debug, serde::Serialize)]
pub struct LlmTestResult {
    pub ok: bool,
    pub model: String,
    pub error: Option<String>,
    pub duration_ms: Option<u64>,
    pub completion_tokens: Option<u32>,
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
fn active_account(config: &AppConfig) -> Result<String, String> {
    config
        .gmail_account
        .as_deref()
        .filter(|account| !account.trim().is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| "No Gmail account selected".to_string())
}

pub fn load_gmail_auth_for_account(
    app: &tauri::AppHandle,
    config: &AppConfig,
    account: &str,
) -> Result<GmailAuth, String> {
    let client_id = resolve_client_id(app, config)?;
    let secret = resolve_client_secret(app);
    GmailAuth::load(account, &client_id, secret.as_deref())
        .ok_or_else(|| "Gmail not authenticated".to_string())
}

pub fn load_gmail_auth(app: &tauri::AppHandle, config: &AppConfig) -> Result<GmailAuth, String> {
    let account = active_account(config)?;
    load_gmail_auth_for_account(app, config, &account)
}

/// Cached labels for the active account, for the commands that need labels but
/// no other Gmail call.
async fn account_labels(
    app: &tauri::AppHandle,
    state: &AppState,
    account_email: &str,
) -> Result<Vec<Label>, String> {
    let config = state.config.lock().await;
    let auth = load_gmail_auth(app, &config)?;
    drop(config);

    let mut gmail = GmailClient::new(auth);
    post_office_core::gmail::cached_labels(&state.db, &mut gmail, account_email)
        .await
        .map_err(|e| e.to_string())
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

    state
        .db
        .with_accounts(|repo| repo.add(&profile.email_address))
        .map_err(|e| e.to_string())?;

    {
        let mut config = state.config.lock().await;
        config.gmail_account = Some(profile.email_address.clone());
        state
            .db
            .with_config(|repo| config.save(&repo))
            .map_err(|e| e.to_string())?;
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
    refresh: Option<bool>,
) -> Result<Vec<Label>, String> {
    let config = state.config.lock().await;
    let account_email = active_account(&config)?;
    let auth = load_gmail_auth(&app, &config)?;
    drop(config);

    let mut gmail = GmailClient::new(auth);
    let labels = if refresh.unwrap_or(false) {
        post_office_core::gmail::refresh_labels(&state.db, &mut gmail, &account_email).await
    } else {
        post_office_core::gmail::cached_labels(&state.db, &mut gmail, &account_email).await
    };
    labels.map_err(|e| e.to_string())
}

/// Reports the *verified* Gmail connection state: it loads the stored tokens and
/// performs a live profile fetch, so `connected` reflects working auth rather
/// than just `gmail.account` being set in config.
#[tauri::command]
pub async fn gmail_connection_status(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<post_office_core::gmail::models::GmailConnection, String> {
    let config = state.config.lock().await.clone();
    let account_email = match active_account(&config) {
        Ok(account_email) => account_email,
        Err(_) => {
            return Ok(post_office_core::gmail::models::GmailConnection {
                connected: false,
                email: None,
                messages_total: None,
                threads_total: None,
                error: None,
            });
        }
    };
    if config.gmail_account.is_none() {
        return Ok(post_office_core::gmail::models::GmailConnection {
            connected: false,
            email: None,
            messages_total: None,
            threads_total: None,
            error: None,
        });
    }

    let auth = match load_gmail_auth_for_account(&app, &config, &account_email) {
        Ok(auth) => auth,
        Err(e) => {
            let _ = state
                .db
                .with_accounts(|repo| repo.record_error(&account_email, &e));
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
        Ok(profile) => {
            let _ = state
                .db
                .with_accounts(|repo| repo.record_success(&account_email));
            Ok(post_office_core::gmail::models::GmailConnection {
                connected: true,
                email: Some(profile.email_address),
                messages_total: Some(profile.messages_total),
                threads_total: Some(profile.threads_total),
                error: None,
            })
        }
        Err(e) => {
            let error = format!(
                "Gmail auth is no longer valid (token may be expired or revoked). Reconnect to fix: {}",
                e
            );
            let _ = state
                .db
                .with_accounts(|repo| repo.record_error(&account_email, &error));
            Ok(post_office_core::gmail::models::GmailConnection {
                connected: true,
                email: None,
                messages_total: None,
                threads_total: None,
                error: Some(error),
            })
        }
    }
}

#[tauri::command]
pub async fn history_by_email(
    state: State<'_, AppState>,
    email_id: String,
) -> Result<Vec<post_office_core::db::history::HistoryEntry>, String> {
    let account_email = active_account(&*state.config.lock().await)?;
    state
        .db
        .with_history(|repo| repo.by_email(&account_email, &email_id))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn pipeline_dry_run(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    email_id: String,
    run_id: String,
) -> Result<PipelineDryRun, String> {
    let statuses = state.pipeline_dry_run_statuses.clone();
    let update_progress =
        |phase: &str, total_rules: u32, progress: Option<PipelineDryRunProgress>| {
            statuses.lock().unwrap().insert(
                run_id.clone(),
                PipelineDryRunStatus {
                    phase: phase.into(),
                    total_rules,
                    progress,
                },
            );
        };
    update_progress("loading_credentials", 0, None);
    let result = async {
        let config = state.config.lock().await.clone();
        let account_email = active_account(&config)?;
        let auth_app = app.clone();
        let auth_config = config.clone();
        let auth = tokio::time::timeout(
            Duration::from_secs(30),
            tokio::task::spawn_blocking(move || load_gmail_auth(&auth_app, &auth_config)),
        )
        .await
        .map_err(|_| "Timed out loading Gmail credentials after 30 seconds. Check that your OS keyring is available.")?
        .map_err(|error| format!("Could not load Gmail credentials: {error}"))??;
        let llm = InferenceRouter::from_config_with_runtime(&config, state.inference_runtime.clone())
            .with_database(state.db.clone());
        let mut gmail = GmailClient::new(auth);
        update_progress("loading_message", 0, None);
        let email = tokio::time::timeout(Duration::from_secs(30), gmail.get_message(&email_id))
            .await
            .map_err(|_| "Timed out loading the Gmail message after 30 seconds. Gmail may be rate limited; try again shortly.")?
            .map_err(|error| error.to_string())?;
        update_progress("loading_labels", 0, None);
        let labels = tokio::time::timeout(
            Duration::from_secs(30),
            post_office_core::gmail::cached_labels(&state.db, &mut gmail, &account_email),
        )
        .await
        .map_err(|_| "Timed out loading Gmail labels after 30 seconds. Gmail may be rate limited; try again shortly.")?
        .map_err(|error| error.to_string())?;
        update_progress("loading_rules", 0, None);
        let rules = state
            .db
            .with_rules(|repo| repo.get_enabled_rules(&account_email))
            .map_err(|error| error.to_string())?;
        let total_rules = rules.len() as u32;
        update_progress("evaluating", total_rules, None);
        let memories_by_rule = rules
            .iter()
            .map(|rule| (rule.id, rule_memories(&state, rule.id)))
            .collect::<HashMap<_, _>>();

        Ok(dry_run_pipeline(
            &llm,
            &rules,
            &email,
            &memories_by_rule,
            &labels,
            &|progress| {
                update_progress("evaluating", total_rules, Some(progress));
            },
        )
        .await)
    }
    .await;
    statuses.lock().unwrap().remove(&run_id);
    result
}

#[tauri::command]
pub fn pipeline_dry_run_status(
    state: State<'_, AppState>,
    run_id: String,
) -> Option<PipelineDryRunStatus> {
    state
        .pipeline_dry_run_statuses
        .lock()
        .unwrap()
        .get(&run_id)
        .cloned()
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
