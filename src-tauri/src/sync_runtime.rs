use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
use post_office_core::config::AppConfig;
use post_office_core::db::Database;
use post_office_core::llm::InferenceRouter;
use post_office_core::llm::InferenceRuntime;
use post_office_core::processing::{mark_last_successful, OpProgress, ProcessingState};
use post_office_core::sync::{queue_notification, replay_history, start_watch};
use serde::{Deserialize, Serialize};
use tauri::Emitter;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::Message;

use crate::ProcessingStates;

#[derive(Debug, Clone)]
pub enum SyncTrigger {
    Startup,
    Sweep,
    Manual,
    Relay {
        account_email: String,
        event_key: String,
    },
}

impl SyncTrigger {
    fn as_str(&self) -> &'static str {
        match self {
            SyncTrigger::Startup => "startup",
            SyncTrigger::Sweep => "sweep",
            SyncTrigger::Manual => "manual",
            SyncTrigger::Relay { .. } => "websocket",
        }
    }
}

#[derive(Debug, Deserialize)]
struct RelayInbound {
    #[serde(rename = "type")]
    kind: String,
    notification_id: Option<String>,
    account_email: Option<String>,
    history_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct RelayHello<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    account_email: &'a str,
    auth_token: &'a str,
}

#[derive(Clone, Serialize)]
struct AccountProgress {
    account_email: String,
    #[serde(flatten)]
    progress: OpProgress,
}

pub fn spawn(
    app: tauri::AppHandle,
    db: Arc<Database>,
    states: ProcessingStates,
    config: Arc<Mutex<AppConfig>>,
    inference_runtime: InferenceRuntime,
) -> mpsc::UnboundedSender<SyncTrigger> {
    let (tx, mut rx) = mpsc::unbounded_channel::<SyncTrigger>();

    let loop_tx = tx.clone();
    let app_loop = app.clone();
    let db_loop = db.clone();
    let states_loop = states.clone();
    let config_loop = config.clone();
    let inference_runtime_loop = inference_runtime.clone();

    tauri::async_runtime::spawn(async move {
        let _ = loop_tx.send(SyncTrigger::Startup);
        loop {
            let cfg = config_loop.lock().await.clone();
            let sweep_secs = (cfg.sync_reconcile_interval_minutes.max(1) as u64) * 60;
            let sleeper = tokio::time::sleep(Duration::from_secs(sweep_secs));
            tokio::pin!(sleeper);

            let trigger = tokio::select! {
                Some(trigger) = rx.recv() => trigger,
                _ = &mut sleeper => SyncTrigger::Sweep,
            };

            let (account_emails, relay_event_key) = match &trigger {
                SyncTrigger::Relay {
                    account_email,
                    event_key,
                } => (vec![account_email.clone()], Some(event_key.clone())),
                _ => (
                    db_loop
                        .with_accounts(|repo| repo.list())
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|account| !account.paused)
                        .map(|account| account.email)
                        .collect::<Vec<_>>(),
                    None,
                ),
            };

            for account_email in account_emails {
                if db_loop
                    .with_accounts(|repo| repo.get(&account_email))
                    .ok()
                    .flatten()
                    .is_some_and(|account| account.paused)
                {
                    continue;
                }
                let state_loop = processing_state_for(&states_loop, &db_loop, &account_email);
                if state_loop
                    .lock()
                    .await
                    .stop_requested
                    .load(std::sync::atomic::Ordering::Relaxed)
                {
                    continue;
                }

                let auth = match crate::commands::load_gmail_auth_for_account(
                    &app_loop,
                    &cfg,
                    &account_email,
                ) {
                    Ok(auth) => auth,
                    Err(e) => {
                        let _ = db_loop
                            .with_sync_state(|repo| repo.set_sync_error(&account_email, Some(&e)));
                        let _ = db_loop.with_accounts(|repo| repo.record_error(&account_email, &e));
                        continue;
                    }
                };

                let llm =
                    InferenceRouter::from_config_with_runtime(&cfg, inference_runtime_loop.clone())
                        .with_database(db_loop.as_ref().clone());
                let mut gmail = post_office_core::gmail::GmailClient::new(auth);

                if let Err(e) =
                    maybe_refresh_watch(&db_loop, &mut gmail, &account_email, &cfg).await
                {
                    let _ = db_loop.with_sync_state(|repo| {
                        repo.set_sync_error(&account_email, Some(&e.to_string()))
                    });
                }

                {
                    let mut s = state_loop.lock().await;
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

                let emit = app_loop.clone();
                let progress_account = account_email.clone();
                match replay_history(
                    &db_loop,
                    &state_loop,
                    &mut gmail,
                    &llm,
                    &cfg,
                    &account_email,
                    trigger.as_str(),
                    &|progress: OpProgress| {
                        let _ = emit.emit(
                            "sync-progress",
                            AccountProgress {
                                account_email: progress_account.clone(),
                                progress,
                            },
                        );
                    },
                )
                .await
                {
                    Ok(result) => {
                        let _ = app_loop.emit("sync-cycle", &result);
                        mark_last_successful(&db_loop, &state_loop, &account_email, Utc::now())
                            .await;
                        let _ = db_loop.with_accounts(|repo| repo.record_success(&account_email));
                        {
                            let mut s = state_loop.lock().await;
                            s.current_progress = None;
                            s.active_phase = "idle".into();
                        }
                        let _ = app_loop.emit(
                            "sync-progress",
                            AccountProgress {
                                account_email: account_email.clone(),
                                progress: OpProgress {
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
                            },
                        );
                        if let Some(event_key) = relay_event_key.as_deref() {
                            let _ = db_loop.with_sync_state(|repo| {
                                repo.mark_event_status(&account_email, event_key, "applied")
                            });
                        }
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        let _ = db_loop.with_sync_state(|repo| {
                            repo.set_sync_error(&account_email, Some(&msg))
                        });
                        let _ =
                            db_loop.with_accounts(|repo| repo.record_error(&account_email, &msg));
                        {
                            let mut s = state_loop.lock().await;
                            s.current_progress = None;
                            s.active_phase = "error".into();
                        }
                        let _ = app_loop.emit(
                            "sync-progress",
                            AccountProgress {
                                account_email: account_email.clone(),
                                progress: OpProgress {
                                    source: "sync".into(),
                                    phase: "error".into(),
                                    processed: 0,
                                    total: None,
                                    detail: Some(msg),
                                    current_email_id: None,
                                    current_email_from: None,
                                    current_email_subject: None,
                                    current_email_sent_at: None,
                                },
                            },
                        );
                        if let Some(event_key) = relay_event_key.as_deref() {
                            let _ = db_loop.with_sync_state(|repo| {
                                repo.mark_event_status(&account_email, event_key, "error")
                            });
                        }
                    }
                }
            }
        }
    });

    spawn_relay_listener(app, db, config, tx.clone());
    tx
}

fn processing_state_for(
    states: &ProcessingStates,
    db: &Database,
    account_email: &str,
) -> Arc<Mutex<ProcessingState>> {
    let mut states = states.lock().unwrap();
    states
        .entry(account_email.to_string())
        .or_insert_with(|| {
            let mut state = ProcessingState::new();
            post_office_core::processing::hydrate_processing_state(db, &mut state, account_email);
            Arc::new(Mutex::new(state))
        })
        .clone()
}

fn spawn_relay_listener(
    app: tauri::AppHandle,
    db: Arc<Database>,
    config: Arc<Mutex<AppConfig>>,
    trigger_tx: mpsc::UnboundedSender<SyncTrigger>,
) {
    tauri::async_runtime::spawn(async move {
        loop {
            let cfg = config.lock().await.clone();
            if !cfg.sync_enabled || !cfg.relay_enabled {
                tokio::time::sleep(Duration::from_secs(10)).await;
                continue;
            }
            if cfg.relay_ws_url.trim().is_empty() || cfg.relay_auth_token.trim().is_empty() {
                tokio::time::sleep(Duration::from_secs(10)).await;
                continue;
            }

            let account_email = match cfg.gmail_account.clone() {
                Some(v) => v,
                None => {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    continue;
                }
            };

            let ws_url = cfg.relay_ws_url.clone();
            let auth_token = cfg.relay_auth_token.clone();
            match tokio_tungstenite::connect_async(&ws_url).await {
                Ok((socket, _)) => {
                    let (mut write, mut read) = socket.split();
                    let hello = RelayHello {
                        kind: "HELLO",
                        account_email: &account_email,
                        auth_token: &auth_token,
                    };
                    if let Ok(payload) = serde_json::to_string(&hello) {
                        let _ = write.send(Message::Text(payload)).await;
                    }

                    while let Some(next) = read.next().await {
                        let msg = match next {
                            Ok(msg) => msg,
                            Err(_) => break,
                        };
                        let text = match msg {
                            Message::Text(text) => text,
                            _ => continue,
                        };
                        let inbound: RelayInbound = match serde_json::from_str(&text) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };

                        match inbound.kind.as_str() {
                            "NOTIFY" => {
                                if let Some(history_id) = inbound.history_id.as_deref() {
                                    let event_key = inbound
                                        .notification_id
                                        .unwrap_or_else(|| format!("relay:{}", history_id));
                                    let account = inbound
                                        .account_email
                                        .as_deref()
                                        .unwrap_or(&account_email)
                                        .to_string();
                                    let _ =
                                        queue_notification(&db, &account, &event_key, history_id);
                                    let _ = trigger_tx.send(SyncTrigger::Relay {
                                        account_email: account,
                                        event_key,
                                    });
                                }
                            }
                            "RESYNC_REQUIRED" => {
                                let _ = trigger_tx.send(SyncTrigger::Manual);
                            }
                            _ => {}
                        }
                    }
                }
                Err(e) => {
                    let _ = app.emit("sync-relay-error", format!("{}", e));
                }
            }

            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}

async fn maybe_refresh_watch(
    db: &Arc<Database>,
    gmail: &mut post_office_core::gmail::GmailClient,
    account_email: &str,
    cfg: &AppConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let topic = cfg.sync_watch_topic.trim();
    if topic.is_empty() {
        return Ok(());
    }

    let state = db.with_sync_state(|repo| repo.get(account_email))?;
    let should_refresh = state
        .as_ref()
        .and_then(|s| s.watch_expiration.as_deref())
        .and_then(parse_rfc3339)
        .map(|dt| dt <= Utc::now() + chrono::Duration::hours(24))
        .unwrap_or(true)
        || state
            .as_ref()
            .map(|s| s.watch_status != "watching")
            .unwrap_or(true);

    if !should_refresh {
        return Ok(());
    }

    let labels = cfg
        .sync_watch_label_ids
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<String>>();

    start_watch(db, gmail, account_email, topic, &labels).await?;
    Ok(())
}

fn parse_rfc3339(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|v| v.with_timezone(&Utc))
}
