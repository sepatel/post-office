//! Tauri side of local decisions (rverdict shadow mode): the service handle
//! and the commands the UI uses.

use std::path::PathBuf;

use post_office_core::db::verdicts::{
    Adjudication, DisagreementPage, DisagreementQuery, Feedback, VerdictRow,
};
use post_office_core::decision::export::ExportSummary;
use post_office_core::decision::report::{rule_reports, RuleReport, ACT_THRESHOLD};
use post_office_core::decision::worker::SHADOW_BATCH_RANGE;
use post_office_core::decision::{StartupMode, VerdictSettings};
use serde::Serialize;
use tauri::State;

use crate::AppState;

/// The running local-decision service (rverdict shadow mode).
pub struct Verdict {
    pub service: post_office_core::decision::service::VerdictService,
    pub data_dir: PathBuf,
}

impl Verdict {
    pub fn start(
        app: &tauri::AppHandle,
        db: post_office_core::db::Database,
        config: std::sync::Arc<tokio::sync::Mutex<post_office_core::config::AppConfig>>,
        data_dir: PathBuf,
    ) -> Self {
        use tauri::Emitter;
        let handle = app.clone();
        let accounts_db = db.clone();
        let service = post_office_core::decision::service::VerdictService::spawn(
            db,
            &data_dir,
            move || {
                let mut settings = config.blocking_lock().verdict_settings();
                // Shadow work reads only locally recorded steps, so a Gmail
                // outage would not block it — but an outage is exactly when
                // the user stops expecting any background inference, so
                // suspend until every account is back. The user's own
                // setting is not consulted because a resume click while an
                // account still cannot connect would silently do nothing.
                let gmail_down = match accounts_db.with_accounts(|repo| {
                    repo.list()
                        .map(|accounts| accounts.iter().any(|account| account.status == "error"))
                }) {
                    Ok(any_error) => any_error,
                    Err(_) => false,
                };
                if gmail_down && settings.enabled {
                    settings.shadow_paused = true;
                }
                settings
            },
            move |status| {
                let _ = handle.emit("verdict-status", status);
            },
        );
        Self { service, data_dir }
    }

    fn wake(&self) {
        self.service.wake();
    }

    fn model(&self) -> Option<String> {
        post_office_core::decision::service::installed_model_id(self.service.models_dir())
    }

    fn calibration(&self) -> post_office_core::decision::Calibration {
        post_office_core::decision::service::installed_calibration(self.service.models_dir())
    }
}

#[derive(Serialize)]
pub struct VerdictOverview {
    pub settings: VerdictSettings,
    pub status: post_office_core::decision::service::VerdictStatus,
}

#[tauri::command]
pub async fn verdict_overview(state: State<'_, AppState>) -> Result<VerdictOverview, String> {
    let settings = state.config.lock().await.verdict_settings();
    Ok(VerdictOverview {
        settings,
        status: state.verdict.service.status(),
    })
}

#[tauri::command]
pub async fn verdict_settings_set(
    state: State<'_, AppState>,
    mut settings: VerdictSettings,
) -> Result<(), String> {
    settings.backend = settings.backend.trim().to_ascii_lowercase();
    if !["f32", "f16"].contains(&settings.precision.as_str()) {
        return Err(format!("unknown precision {:?}", settings.precision));
    }
    if !["auto", "cpu", "wgpu", "cuda", "rocm"].contains(&settings.backend.as_str()) {
        return Err(format!("unknown backend {:?}", settings.backend));
    }
    if !(256..=8_192).contains(&settings.max_state_tokens) {
        return Err("max_state_tokens must be between 256 and 8192".into());
    }
    match settings.startup {
        StartupMode::Immediate | StartupMode::Delayed | StartupMode::Manual => {}
    }
    if !SHADOW_BATCH_RANGE.contains(&settings.shadow_batch_size) {
        return Err(format!(
            "shadow_batch_size must be between {} and {}",
            SHADOW_BATCH_RANGE.start(),
            SHADOW_BATCH_RANGE.end()
        ));
    }
    {
        let mut config = state.config.lock().await;
        config.set_verdict_settings(&settings);
        state
            .db
            .with_config(|repo| config.save(&repo))
            .map_err(|e| e.to_string())?;
    }
    state.verdict.wake();
    Ok(())
}

/// Starts deferred or manual shadow work after an explicit user action.
#[tauri::command]
pub async fn verdict_start(state: State<'_, AppState>) -> Result<(), String> {
    let settings = state.config.lock().await.verdict_settings();
    if !settings.enabled {
        return Err("turn on local decisions before starting shadow work".into());
    }
    if settings.shadow_paused {
        return Err("resume shadowing before explicitly starting it".into());
    }
    state.verdict.service.request_start();
    Ok(())
}

/// Runs one explicit f16 check. The requested precision stays f16, but the
/// service only uses it after this check passes for the loaded model.
#[tauri::command]
pub async fn verdict_qualify_f16(state: State<'_, AppState>) -> Result<(), String> {
    let settings = state.config.lock().await.verdict_settings();
    if !settings.enabled {
        return Err("turn on local decisions before checking f16".into());
    }
    if settings.precision != "f16" {
        return Err("select f16 before checking it".into());
    }
    if ["cpu", "reference"].contains(&settings.backend.trim().to_ascii_lowercase().as_str()) {
        return Err("f16 checks apply to GPU backends".into());
    }
    if settings.shadow_paused {
        return Err("resume shadowing before checking f16".into());
    }
    if state.verdict.service.status().state == "qualifying" {
        return Err("an f16 check is already running".into());
    }
    state.verdict.service.request_f16_qualification();
    Ok(())
}

#[tauri::command]
pub async fn verdict_report(state: State<'_, AppState>) -> Result<Vec<RuleReport>, String> {
    let model = state.verdict.model();
    let calibration = state.verdict.calibration();
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || rule_reports(&db, model.as_deref(), &calibration))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[derive(Serialize)]
pub struct MessageVerdicts {
    pub verdicts: Vec<VerdictRow>,
    pub feedback: Vec<Feedback>,
}

#[tauri::command]
pub async fn verdict_message(
    state: State<'_, AppState>,
    message_id: i64,
) -> Result<MessageVerdicts, String> {
    let verdicts = state
        .db
        .with_verdicts(|repo| repo.for_message(message_id))
        .map_err(|e| e.to_string())?;
    let feedback = state
        .db
        .with_verdicts(|repo| repo.feedback_for_message(message_id))
        .map_err(|e| e.to_string())?;
    Ok(MessageVerdicts { verdicts, feedback })
}

/// The recorded disagreements for the review queue, newest first.
#[tauri::command]
pub async fn verdict_disagreements(
    state: State<'_, AppState>,
    mut query: DisagreementQuery,
) -> Result<DisagreementPage, String> {
    // The threshold lives with the report that promotes on it, so the queue
    // and the table cannot drift apart on what counts as confident.
    query.model = state.verdict.model();
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with_verdicts(|repo| repo.disagreements(&query, ACT_THRESHOLD))
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

/// Records who was right about one decision, or clears the ruling.
#[tauri::command]
pub async fn verdict_adjudicate(
    state: State<'_, AppState>,
    verdict_id: i64,
    right: Option<String>,
) -> Result<(), String> {
    let right = right.as_deref().map(Adjudication::from_name).transpose()?;
    state
        .db
        .with_verdicts(|repo| repo.set_adjudication(verdict_id, right))
        .map_err(|e| e.to_string())
}

/// Writes the decision export under the app data directory and returns
/// where. The files contain email text and stay on this machine.
#[tauri::command]
pub async fn verdict_export(state: State<'_, AppState>) -> Result<ExportSummary, String> {
    let dir = state.verdict.data_dir.join("exports").join(
        chrono::Utc::now()
            .format("decisions-%Y%m%d-%H%M%S")
            .to_string(),
    );
    let model = state.verdict.model();
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        post_office_core::decision::export::export(&db, &dir, model.as_deref())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}
