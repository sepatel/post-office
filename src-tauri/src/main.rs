use post_office_core::config::AppConfig;
use post_office_core::processing::ProcessingState;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tauri::tray::TrayIcon;
use tauri::Manager;
use tokio::sync::mpsc;
use tokio::sync::Mutex;

mod commands;
mod sync_runtime;
mod tray;

pub struct AppState {
    pub db: post_office_core::db::Database,
    pub processing_state: Arc<Mutex<ProcessingState>>,
    pub config: Arc<Mutex<AppConfig>>,
    pub sync_trigger: mpsc::UnboundedSender<sync_runtime::SyncTrigger>,
    pub poller_started: Arc<AtomicBool>,
    pub tray: Arc<std::sync::Mutex<Option<TrayIcon>>>,
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

            let config = db.with_config(|repo| AppConfig::load(&repo));
            let mut processing = ProcessingState::new();
            post_office_core::processing::hydrate_processing_state(&db, &mut processing);
            let processing_state = Arc::new(Mutex::new(processing));
            let config_arc = Arc::new(Mutex::new(config.clone()));
            let sync_trigger = sync_runtime::spawn(
                app.handle().clone(),
                Arc::new(db.clone()),
                processing_state.clone(),
                config_arc.clone(),
            );

            let app_state = AppState {
                db,
                processing_state: processing_state.clone(),
                config: config_arc.clone(),
                sync_trigger,
                poller_started: Arc::new(AtomicBool::new(false)),
                tray: Arc::new(std::sync::Mutex::new(None)),
            };

            app.manage(app_state);

            if !config.sync_enabled {
                let state_handle = app.state::<AppState>();
                crate::commands::ensure_polling_started(
                    app.handle().clone(),
                    &state_handle,
                    &config,
                );
            }

            let tray = tray::setup_tray(app, &config.tray_theme)?;
            app.state::<AppState>().tray.lock().unwrap().replace(tray);
            let window = app.get_webview_window("main").unwrap();
            window.show()?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::config_get,
            commands::config_set,
            commands::llm_config_set,
            commands::rules_list,
            commands::rules_create,
            commands::rules_update,
            commands::rules_delete,
            commands::rule_chat_history,
            commands::rule_chat_send,
            commands::rule_apply_proposal,
            commands::gmail_recent_messages,
            commands::rules_test,
            commands::rules_apply,
            commands::bulk_evaluate,
            commands::history_list,
            commands::rules_metrics,
            commands::rule_roi_metrics,
            commands::history_search,
            commands::inference_jobs_list,
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
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
