use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::sync::Mutex;

use crate::config::AppConfig;
use crate::db::history::NewHistoryEntry;
use crate::db::Database;
use crate::gmail::models::Message;
use crate::gmail::GmailClient;
use crate::llm::LlmClient;
use crate::rules::actions::execute_action;
use crate::rules::engine::{ProcessingResult, RuleEngine};

pub struct ProcessingState {
    pub paused: Arc<AtomicBool>,
    pub last_processed: Option<chrono::DateTime<Utc>>,
    pub emails_processed_today: usize,
}

impl ProcessingState {
    pub fn new() -> Self {
        Self {
            paused: Arc::new(AtomicBool::new(false)),
            last_processed: None,
            emails_processed_today: 0,
        }
    }
}

pub async fn run_processing_loop(
    db: Arc<Database>,
    state: Arc<Mutex<ProcessingState>>,
    mut gmail: GmailClient,
    llm: LlmClient,
    config: Arc<Mutex<AppConfig>>,
) {
    loop {
        let cfg = config.lock().await.clone();

        if !cfg.polling_enabled || state.lock().await.paused.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_secs(10)).await;
            continue;
        }

        let interval = Duration::from_secs(cfg.polling_interval_minutes as u64 * 60);

        match process_one_cycle(&db, &state, &mut gmail, &llm, &cfg).await {
            Ok(count) => {
                tracing::info!("Processed {} emails this cycle", count);
            }
            Err(e) => {
                tracing::error!("Processing cycle failed: {}", e);
            }
        }

        tokio::time::sleep(interval).await;
    }
}

async fn process_one_cycle(
    db: &Arc<Database>,
    state: &Arc<Mutex<ProcessingState>>,
    gmail: &mut GmailClient,
    llm: &LlmClient,
    config: &AppConfig,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let messages = gmail
        .list_messages(&config.polling_query, config.polling_max_per_cycle)
        .await?;

    let mut count = 0;

    for msg_ref in &messages {
        let email = gmail.get_message(&msg_ref.id).await?;

        let labels = email.label_ids.clone();

        let result = db.with_rules(|rule_repo| {
            let engine = RuleEngine::new(&rule_repo, llm);
            let rt = tokio::runtime::Handle::current();
            rt.block_on(engine.process_email(&email, &labels))
        });

        match result {
            Ok(ProcessingResult::RuleMatched {
                rule_id,
                rule_name,
                action,
                llm_response,
                duration_ms,
            }) => {
                let action_str = action
                    .as_ref()
                    .map(|a| format!("{:?}", a))
                    .unwrap_or_else(|| "SKIP".into());

                if let Some(ref parsed_action) = action {
                    let gmail_labels = gmail.list_labels().await.unwrap_or_default();
                    match execute_action(gmail, &msg_ref.id, parsed_action, &gmail_labels).await {
                        Ok(()) => {
                            db.with_history(|repo| {
                                repo.insert(&NewHistoryEntry {
                                    email_id: msg_ref.id.clone(),
                                    email_from: extract_header(&email, "From"),
                                    email_subject: extract_header(&email, "Subject"),
                                    rule_id: Some(rule_id),
                                    rule_name: Some(rule_name),
                                    action: action_str,
                                    status: "success".into(),
                                    llm_model: None,
                                    llm_response: Some(llm_response),
                                    error: None,
                                    duration_ms: Some(duration_ms as i64),
                                })
                            })?;
                        }
                        Err(e) => {
                            db.with_history(|repo| {
                                repo.insert(&NewHistoryEntry {
                                    email_id: msg_ref.id.clone(),
                                    email_from: extract_header(&email, "From"),
                                    email_subject: extract_header(&email, "Subject"),
                                    rule_id: Some(rule_id),
                                    rule_name: Some(rule_name),
                                    action: action_str,
                                    status: "error".into(),
                                    llm_model: None,
                                    llm_response: Some(llm_response),
                                    error: Some(e.to_string()),
                                    duration_ms: Some(duration_ms as i64),
                                })
                            })?;
                        }
                    }
                } else {
                    db.with_history(|repo| {
                        repo.insert(&NewHistoryEntry {
                            email_id: msg_ref.id.clone(),
                            email_from: extract_header(&email, "From"),
                            email_subject: extract_header(&email, "Subject"),
                            rule_id: Some(rule_id),
                            rule_name: Some(rule_name),
                            action: action_str,
                            status: "skipped".into(),
                            llm_model: None,
                            llm_response: Some(llm_response),
                            error: Some("LLM returned no actionable response".into()),
                            duration_ms: Some(duration_ms as i64),
                        })
                    })?;
                }
            }
            Ok(ProcessingResult::NoMatch) => {}
            Err(e) => {
                tracing::warn!("Failed to process email {}: {}", msg_ref.id, e);
            }
        }

        count += 1;
    }

    let mut s = state.lock().await;
    s.last_processed = Some(Utc::now());
    s.emails_processed_today += count;

    Ok(count)
}

fn extract_header(email: &Message, name: &str) -> Option<String> {
    email
        .payload
        .as_ref()
        .and_then(|p| {
            p.headers
                .iter()
                .find(|h| h.name == name)
                .map(|h| h.value.clone())
        })
}
