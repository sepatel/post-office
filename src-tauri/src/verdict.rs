//! Tauri side of local decisions (rverdict shadow mode): the service handle
//! and the commands the UI uses.

use std::path::PathBuf;

use post_office_core::db::verdicts::{Feedback, VerdictRow};
use post_office_core::decision::export::ExportSummary;
use post_office_core::decision::report::{rule_reports, RuleReport};
use post_office_core::decision::VerdictSettings;
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
        let service = post_office_core::decision::service::VerdictService::spawn(
            db,
            &data_dir,
            move || config.blocking_lock().verdict_settings(),
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
    settings: VerdictSettings,
) -> Result<(), String> {
    if !["f32", "f16"].contains(&settings.precision.as_str()) {
        return Err(format!("unknown precision {:?}", settings.precision));
    }
    if !(256..=8_192).contains(&settings.max_state_tokens) {
        return Err("max_state_tokens must be between 256 and 8192".into());
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

/// Thumbs up (`true`), down (`false`) or cleared (`null`) on the LLM's
/// decision for a step.
#[tauri::command]
pub async fn verdict_rate(
    state: State<'_, AppState>,
    step_id: i64,
    up: Option<bool>,
) -> Result<(), String> {
    state
        .db
        .with_verdicts(|repo| repo.set_rating(step_id, up))
        .map_err(|e| e.to_string())
}

/// Thumbs on the LLM's answer for one label of a multiple-match rule, as
/// opposed to the step as a whole.
#[tauri::command]
pub async fn verdict_rate_row(
    state: State<'_, AppState>,
    verdict_id: i64,
    up: Option<bool>,
) -> Result<(), String> {
    state
        .db
        .with_verdicts(|repo| repo.set_verdict_rating(verdict_id, up))
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
