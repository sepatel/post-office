use post_office_core::config::AppConfig;
use post_office_core::db::rule_chat::ChatMessageRow;
use post_office_core::db::rules::{CreateRuleRequest, UpdateRuleRequest};
use post_office_core::gmail::oauth::{auth_url, exchange_code, generate_pkce};
use post_office_core::gmail::{store_tokens, GmailAuth, GmailClient};
use post_office_core::llm::LlmClient;
use post_office_core::processing::run_backfill;
use post_office_core::rules::actions::execute_action;
use post_office_core::rules::chat::{apply_proposal, chat_with_rule, ChatProposal, ChatTurn};
use post_office_core::rules::engine::{display_action, ActionDisplay, TestResult};
use post_office_core::rules::evaluation::BulkVerdict;
use post_office_core::rules::models::{Action, Condition, Rule};
use post_office_core::rules::response_parser::ParsedAction;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::Emitter;
use tauri::Manager;
use tauri::State;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use chrono::{DateTime, NaiveDate, Utc};

use crate::tray;
use crate::AppState;

#[derive(Debug, Serialize, Deserialize)]
pub struct RuleCreateRequest {
    pub name: String,
    pub description: Option<String>,
    pub conditions: Vec<Condition>,
    pub prompt: String,
    pub actions: Vec<Action>,
    pub priority: i32,
    pub enabled: bool,
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
    }
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
    let mut config = state.config.lock().await;
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
        "google.client_id" => config.google_client_id = value,
        "ui.tray_theme" => config.tray_theme = value,
        _ => return Err(format!("Unknown config key: {}", key)),
    }
    state
        .db
        .with_config(|repo| config.save(&repo))
        .map_err(|e| e.to_string())?;
    Ok(())
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
    state: State<'_, AppState>,
    rule: RuleCreateRequest,
) -> Result<post_office_core::rules::models::Rule, String> {
    let create = CreateRuleRequest {
        name: rule.name,
        description: rule.description,
        conditions: rule.conditions,
        prompt: rule.prompt,
        actions: rule.actions,
        priority: rule.priority,
        enabled: rule.enabled,
    };
    state
        .db
        .with_rules(|repo| repo.create(&create))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rules_update(
    state: State<'_, AppState>,
    id: i64,
    rule: RuleUpdateRequest,
) -> Result<post_office_core::rules::models::Rule, String> {
    let update = UpdateRuleRequest {
        name: rule.name,
        description: rule.description,
        conditions: rule.conditions,
        prompt: rule.prompt,
        actions: rule.actions,
        priority: rule.priority,
        enabled: rule.enabled,
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
        LlmClient::new(
            &config.llm_base_url,
            &config.llm_api_key,
            &config.llm_default_model,
        )
    };

    chat_with_rule(&state.db, &llm, rule_id, &message)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rule_apply_proposal(
    state: State<'_, AppState>,
    rule_id: i64,
    proposal: ChatProposal,
) -> Result<(), String> {
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
    rule: RuleCreateRequest,
    message_id: String,
) -> Result<TestResult, String> {
    let (auth, llm) = {
        let config = state.config.lock().await;
        let auth = load_gmail_auth(&app, &config)?;
        let llm = LlmClient::new(
            &config.llm_base_url,
            &config.llm_api_key,
            &config.llm_default_model,
        );
        (auth, llm)
    };

    let mut gmail = GmailClient::new(auth);
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
    rule: RuleCreateRequest,
    message_ids: Vec<String>,
) -> Result<Vec<BulkVerdict>, String> {
    let (auth, llm) = {
        let config = state.config.lock().await;
        let auth = load_gmail_auth(&app, &config)?;
        let llm = LlmClient::new(
            &config.llm_base_url,
            &config.llm_api_key,
            &config.llm_default_model,
        );
        (auth, llm)
    };

    let mut gmail = GmailClient::new(auth);
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
    rule: RuleCreateRequest,
    message_id: String,
) -> Result<ApplyResult, String> {
    let (auth, llm) = {
        let config = state.config.lock().await;
        let auth = load_gmail_auth(&app, &config)?;
        let llm = LlmClient::new(
            &config.llm_base_url,
            &config.llm_api_key,
            &config.llm_default_model,
        );
        (auth, llm)
    };

    let mut gmail = GmailClient::new(auth);
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

    let labels = gmail.list_labels().await.map_err(|e| e.to_string())?;
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
) -> Result<usize, String> {
    let after_dt = parse_day(&after).map_err(|e| format!("Invalid 'after' date: {}", e))?;
    let before_dt = parse_day_end(&before).map_err(|e| format!("Invalid 'before' date: {}", e))?;

    let (auth, llm) = {
        let config = state.config.lock().await;
        let auth = load_gmail_auth(&app, &config)?;
        let llm = LlmClient::new(
            &config.llm_base_url,
            &config.llm_api_key,
            &config.llm_default_model,
        );
        (auth, llm)
    };

    let mut gmail = GmailClient::new(auth);
    let db = Arc::new(state.db.clone());
    let state_clone = state.processing_state.clone();
    let config = state.config.lock().await.clone();
    drop(state);

    run_backfill(
        &db,
        &state_clone,
        &mut gmail,
        &llm,
        &config,
        after_dt,
        before_dt,
        &rule_ids,
        &|progress| {
            let _ = app.emit("backfill-progress", &progress);
        },
    )
    .await
    .map_err(|e| e.to_string())
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

    {
        let mut config = state.config.lock().await;
        config.gmail_account = Some(profile.email_address.clone());
        state
            .db
            .with_config(|repo| config.save(&repo))
            .map_err(|e| e.to_string())?;
    }

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
