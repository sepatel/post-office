use post_office_core::config::AppConfig;
use post_office_core::gmail::{GmailAuth, GmailClient};
use post_office_core::processing::{run_processing_loop, ProcessingState};
use std::sync::Arc;
use tauri::Manager;
use tokio::sync::Mutex;

mod commands;
mod tray;

pub struct AppState {
    pub db: post_office_core::db::Database,
    pub processing_state: Arc<Mutex<ProcessingState>>,
    pub config: Arc<Mutex<AppConfig>>,
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

            tauri::async_runtime::spawn(async move {
                if let Some(ref account) = config_clone2.gmail_account {
                    match GmailAuth::load(account) {
                        Some(auth) => {
                            let gmail = GmailClient::new(auth);
                            run_processing_loop(
                                db_clone,
                                state_clone,
                                gmail,
                                llm,
                                config_clone,
                            )
                            .await;
                        }
                        None => {
                            tracing::warn!("No Gmail auth found for account: {}", account);
                        }
                    }
                } else {
                    tracing::warn!("No Gmail account configured");
                }
            });

            tray::setup_tray(app)?;
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
            commands::history_list,
            commands::history_search,
            commands::processing_status,
            commands::processing_pause,
            commands::processing_resume,
            commands::gmail_authenticate,
            commands::gmail_get_profile,
            commands::gmail_list_labels,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
