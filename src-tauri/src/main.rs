use post_office_core::config::AppConfig;
use post_office_core::gmail::GmailClient;
use post_office_core::processing::{run_processing_loop, ProcessingState};
use std::sync::Arc;
use tauri::Manager;
use tauri::tray::TrayIcon;
use tauri::Emitter;
use tokio::sync::Mutex;

mod commands;
mod tray;

pub struct AppState {
    pub db: post_office_core::db::Database,
    pub processing_state: Arc<Mutex<ProcessingState>>,
    pub config: Arc<Mutex<AppConfig>>,
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
            let processing_state = Arc::new(Mutex::new(ProcessingState::new()));
            let config_arc = Arc::new(Mutex::new(config.clone()));

            let app_state = AppState {
                db,
                processing_state: processing_state.clone(),
                config: config_arc.clone(),
                tray: Arc::new(std::sync::Mutex::new(None)),
            };

            app.manage(app_state);

            let llm = post_office_core::llm::LlmClient::new(
                &config.llm_base_url,
                &config.llm_api_key,
                &config.llm_default_model,
            );

            let state_handle = app.state::<AppState>();
            let db_clone = Arc::new(state_handle.db.clone());
            let state_clone = state_handle.processing_state.clone();
            let config_clone = state_handle.config.clone();
            let config_clone2 = config.clone();
            let app_handle = app.handle().clone();

            tauri::async_runtime::spawn(async move {
                match crate::commands::load_gmail_auth(&app_handle, &config_clone2) {
                    Ok(auth) => {
                        let gmail = GmailClient::new(auth);
                        let emit_handle = app_handle.clone();
                        run_processing_loop(
                            db_clone,
                            state_clone,
                            gmail,
                            llm,
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
            });

            let tray = tray::setup_tray(app, &config.tray_theme)?;
            app.state::<AppState>().tray.lock().unwrap().replace(tray);
            let window = app.get_webview_window("main").unwrap();
            window.show()?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::config_get,
            commands::config_set,
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
            commands::history_search,
            commands::processing_status,
            commands::processing_pause,
            commands::processing_resume,
            commands::processing_backfill,
            commands::gmail_authenticate,
            commands::gmail_get_profile,
            commands::gmail_list_labels,
            commands::gmail_connection_status,
            commands::history_by_email,
            commands::tray_refresh,
            commands::llm_test,
            commands::llm_list_models,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
