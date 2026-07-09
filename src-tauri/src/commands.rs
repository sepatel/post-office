use post_office_core::config::AppConfig;
use post_office_core::db::rules::{CreateRuleRequest, UpdateRuleRequest};
use post_office_core::gmail::{GmailAuth, GmailClient};
use post_office_core::rules::models::{Action, Condition};
use serde::{Deserialize, Serialize};
use tauri::State;

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
pub struct ProcessingStatus {
    pub paused: bool,
    pub last_processed: Option<String>,
    pub emails_processed_today: usize,
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
        "gmail.account" => config.gmail_account = Some(value),
        "llm.base_url" => config.llm_base_url = value,
        "llm.api_key" => config.llm_api_key = value,
        "llm.default_model" => config.llm_default_model = value,
        "llm.temperature" => {
            let val: f32 = value.parse().map_err(|e: std::num::ParseFloatError| e.to_string())?;
            config.llm_temperature = val;
        }
        "llm.max_tokens" => {
            let val: u32 = value.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            config.llm_max_tokens = val;
        }
        "polling.query" => config.polling_query = value,
        "polling.interval_minutes" => {
            let val: u32 = value.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            config.polling_interval_minutes = val;
        }
        "polling.max_per_cycle" => {
            let val: u32 = value.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            config.polling_max_per_cycle = val;
        }
        "polling.enabled" => {
            let val: bool = value.parse().map_err(|e: std::str::ParseBoolError| e.to_string())?;
            config.polling_enabled = val;
        }
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
    let ps = state.processing_state.lock().await;
    Ok(ProcessingStatus {
        paused: ps.paused.load(std::sync::atomic::Ordering::Relaxed),
        last_processed: ps.last_processed.map(|dt| dt.to_rfc3339()),
        emails_processed_today: ps.emails_processed_today,
    })
}

#[tauri::command]
pub async fn processing_pause(state: State<'_, AppState>) -> Result<(), String> {
    let ps = state.processing_state.lock().await;
    ps.paused
        .store(true, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}

#[tauri::command]
pub async fn processing_resume(state: State<'_, AppState>) -> Result<(), String> {
    let ps = state.processing_state.lock().await;
    ps.paused
        .store(false, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}

#[tauri::command]
pub async fn gmail_authenticate(
    _state: State<'_, AppState>,
) -> Result<String, String> {
    let client_id = std::env::var("GOOGLE_CLIENT_ID")
        .map_err(|_| "GOOGLE_CLIENT_ID not set".to_string())?;

    let auth_url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri=http://localhost&response_type=code&scope=https://www.googleapis.com/auth/gmail.modify+https://www.googleapis.com/auth/gmail.labels&access_type=offline",
        client_id
    );

    open::that(&auth_url).map_err(|e| e.to_string())?;

    Ok(auth_url)
}

#[tauri::command]
pub async fn gmail_get_profile(
    state: State<'_, AppState>,
) -> Result<post_office_core::gmail::models::GmailProfile, String> {
    let config = state.config.lock().await;
    let account = config
        .gmail_account
        .as_ref()
        .ok_or("No Gmail account configured")?;

    let auth = GmailAuth::load(account).ok_or("Gmail not authenticated")?;
    let mut gmail = GmailClient::new(auth);
    gmail.get_profile().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn gmail_list_labels(
    state: State<'_, AppState>,
) -> Result<Vec<post_office_core::gmail::models::Label>, String> {
    let config = state.config.lock().await;
    let account = config
        .gmail_account
        .as_ref()
        .ok_or("No Gmail account configured")?;

    let auth = GmailAuth::load(account).ok_or("Gmail not authenticated")?;
    let mut gmail = GmailClient::new(auth);
    gmail.list_labels().await.map_err(|e| e.to_string())
}
