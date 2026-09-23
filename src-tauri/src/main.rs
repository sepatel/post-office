use post_office_core::config::AppConfig;
use post_office_core::llm::InferenceRuntime;
use post_office_core::processing::ProcessingState;
use post_office_core::rules::engine::PipelineDryRunProgress;
use serde::Serialize;
use std::sync::Arc;
use tauri::tray::TrayIcon;
use tauri::Manager;
use tokio::sync::mpsc;
use tokio::sync::Mutex;

mod commands;
mod sync_runtime;
mod tray;

pub type ProcessingStates =
    Arc<std::sync::Mutex<std::collections::HashMap<String, Arc<Mutex<ProcessingState>>>>>;
pub type PipelineDryRunStatuses =
    Arc<std::sync::Mutex<std::collections::HashMap<String, PipelineDryRunStatus>>>;

#[derive(Clone, Serialize)]
pub struct PipelineDryRunStatus {
    pub phase: String,
    pub total_rules: u32,
    pub progress: Option<PipelineDryRunProgress>,
}

pub struct AppState {
    pub db: post_office_core::db::Database,
    pub processing_states: ProcessingStates,
    pub pipeline_dry_run_statuses: PipelineDryRunStatuses,
    pub config: Arc<Mutex<AppConfig>>,
    pub inference_runtime: InferenceRuntime,
    pub sync_trigger: mpsc::UnboundedSender<sync_runtime::SyncTrigger>,
    pub tray: Arc<std::sync::Mutex<Option<TrayIcon>>>,
}

impl AppState {
    pub fn processing_state_for(&self, account_email: &str) -> Arc<Mutex<ProcessingState>> {
        commands::processing_state_for(&self.processing_states, &self.db, account_email)
    }

    pub fn remove_processing_state(&self, account_email: &str) {
        self.processing_states.lock().unwrap().remove(account_email);
    }
}

fn main() {
    tracing_subscriber::fmt::init();

    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let app_dir = app
                .path()
                .app_data_dir()
                .expect("Failed to get app data dir");
            std::fs::create_dir_all(&app_dir).expect("Failed to create app data dir");

            let db_path = app_dir.join("post-office.db");
            let db =
                post_office_core::db::Database::open(&db_path).expect("Failed to open database");
            db.migrate().expect("Failed to run migrations");

            let mut config = db.with_config(|repo| AppConfig::load(&repo));
            if !config.llm_api_key.trim().is_empty() {
                match post_office_core::llm::credentials::store_provider_api_key(
                    "legacy",
                    &config.llm_api_key,
                ) {
                    Ok(()) => {
                        config.llm_api_key.clear();
                        db.with_config(|repo| config.save(&repo))
                            .expect("Failed to save migrated LLM configuration");
                    }
                    Err(error) => tracing::warn!(
                        "Could not migrate the legacy LLM credential to the keyring: {}",
                        error
                    ),
                }
            }
            for account in db
                .with_accounts(|repo| repo.list())
                .expect("Failed to load accounts for workflow migration")
            {
                post_office_core::workflow::ensure_rule_set(&db, &account.email, &config)
                    .expect("Failed to initialize message workflow rule set");
            }
            let config_arc = Arc::new(Mutex::new(config.clone()));
            let inference_runtime = InferenceRuntime::default();
            post_office_core::llm::configure_runtime(&config, &inference_runtime);
            let processing_states =
                Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
            let sync_trigger = sync_runtime::spawn(
                app.handle().clone(),
                Arc::new(db.clone()),
                processing_states.clone(),
                config_arc.clone(),
                inference_runtime.clone(),
            );

            let app_state = AppState {
                db,
                processing_states,
                pipeline_dry_run_statuses: Arc::new(std::sync::Mutex::new(
                    std::collections::HashMap::new(),
                )),
                config: config_arc.clone(),
                inference_runtime,
                sync_trigger,
                tray: Arc::new(std::sync::Mutex::new(None)),
            };

            app.manage(app_state);

            let state_handle = app.state::<AppState>();
            commands::spawn_message_workflow_worker(
                app.handle().clone(),
                Arc::new(state_handle.db.clone()),
                state_handle.processing_states.clone(),
                state_handle.config.clone(),
                state_handle.inference_runtime.clone(),
            );

            let tray = tray::setup_tray(app, &config.tray_theme)?;
            app.state::<AppState>().tray.lock().unwrap().replace(tray);
            let window = app.get_webview_window("main").unwrap();
            window.show()?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::config_get,
            commands::config_set,
            commands::accounts_list,
            commands::accounts_select,
            commands::accounts_set_paused,
            commands::accounts_reorder,
            commands::accounts_remove,
            commands::llm_config_set,
            commands::rules_list,
            commands::rules_create,
            commands::rules_update,
            commands::rules_delete,
            commands::rules_reorder,
            commands::backup_export,
            commands::backup_import,
            commands::rule_memories_list,
            commands::rule_memory_delete,
            commands::rule_chat_history,
            commands::rule_chat_send,
            commands::rule_apply_proposal,
            commands::gmail_recent_messages,
            commands::rules_test,
            commands::rules_apply,
            commands::evaluate_messages,
            commands::workflow_queue_summary,
            commands::workflow_messages_list,
            commands::workflow_message_get,
            commands::workflow_retry_now,
            commands::workflow_rule_set_status,
            commands::workflow_endpoint_status,
            commands::history_list,
            commands::rules_metrics,
            commands::rule_roi_metrics,
            commands::rule_request_metrics,
            commands::rule_activity,
            commands::history_search,
            commands::inference_jobs_list,
            commands::rule_inference_jobs,
            commands::inference_job_attempts,
            commands::inference_job_retry,
            commands::processing_status,
            commands::processing_pause,
            commands::processing_resume,
            commands::processing_backfill,
            commands::processing_backfill_stop,
            commands::gmail_authenticate,
            commands::gmail_get_profile,
            commands::gmail_list_labels,
            commands::gmail_connection_status,
            commands::history_by_email,
            commands::pipeline_dry_run,
            commands::pipeline_dry_run_status,
            commands::sync_status,
            commands::sync_replay_now,
            commands::sync_watch_start,
            commands::sync_watch_stop,
            commands::tray_refresh,
            commands::llm_test,
            commands::llm_provider_test,
            commands::llm_list_models,
            commands::llm_provider_set_api_key,
            commands::llm_provider_status_list,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if matches!(
                event,
                tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
            ) {
                app.state::<AppState>().inference_runtime.cancel_all();
            }
        });
}
