use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::sync::Mutex;

use crate::config::AppConfig;
use crate::db::history::NewHistoryEntry;
use crate::db::Database;
use crate::gmail::models::Message;
use crate::rules::matcher;
use crate::rules::engine::{resolve_rule, Resolved};

const LAST_RUN_KEY: &str = "processing.last_run";

/// Live progress for an operation (a polling cycle or a one-off backfill),
/// emitted so the UI can show a status bar instead of freezing on a blocking
/// call. `phase` tells the UI which spinner/label to render.
#[derive(Debug, Clone, Serialize)]
pub struct OpProgress {
    pub phase: String,
    pub processed: usize,
    pub total: Option<usize>,
    pub detail: Option<String>,
}
use crate::gmail::GmailClient;
use crate::llm::LlmClient;
use crate::rules::actions::execute_action;
use crate::rules::models::Rule;
use crate::rules::response_parser::ParsedAction;

pub struct ProcessingState {
    pub paused: Arc<AtomicBool>,
    // Keep "Last Run" moving even when cycles fail.
    pub last_processed: Option<chrono::DateTime<Utc>>,
    pub last_successful: Option<chrono::DateTime<Utc>>,
    pub emails_processed_today: usize,
    pub last_cycle_count: usize,
    pub last_cycle_error: Option<String>,
    pub active_phase: String,
}

impl ProcessingState {
    pub fn new() -> Self {
        Self {
            paused: Arc::new(AtomicBool::new(false)),
            last_processed: None,
            last_successful: None,
            emails_processed_today: 0,
            last_cycle_count: 0,
            last_cycle_error: None,
            active_phase: "idle".into(),
        }
    }
}

/// Resolve the timestamp from which polling should fetch messages. We resume
/// from the most recent successful run so a restart never re-scans history:
/// 1. the persisted `processing.last_run` (set after each cycle), else
/// 2. the newest `history.created_at` (messages we've already handled), else
/// 3. `now` — a brand-new install anchors to the present and ignores old mail.
fn resolve_floor(db: &Arc<Database>) -> DateTime<Utc> {
    if let Some(stored) = db.with_config(|repo| repo.get(LAST_RUN_KEY).ok().flatten()) {
        if let Ok(ts) = DateTime::parse_from_rfc3339(&stored) {
            return ts.with_timezone(&Utc);
        }
    }
    if let Some(newest) = db.with_history(|repo| repo.latest_created_at().ok().flatten()) {
        if let Ok(ts) = DateTime::parse_from_rfc3339(&newest) {
            return ts.with_timezone(&Utc);
        }
    }
    Utc::now()
}

/// Persist the last successful run so restarts resume from here instead of
/// walking the entire mailbox again.
fn persist_last_run(db: &Arc<Database>, ts: DateTime<Utc>) {
    let value = ts.to_rfc3339();
    let _ = db.with_config(|repo| repo.set(LAST_RUN_KEY, &value));
}

/// Build the Gmail query, appending `after:<unix_seconds>` so we only fetch
/// mail newer than the resume floor.
fn effective_query(base: &str, floor: DateTime<Utc>) -> String {
    let after = floor.timestamp();
    let trimmed = base.trim();
    if trimmed.is_empty() {
        format!("after:{after}")
    } else {
        format!("{trimmed} after:{after}")
    }
}

pub async fn run_processing_loop(
    db: Arc<Database>,
    state: Arc<Mutex<ProcessingState>>,
    mut gmail: GmailClient,
    llm: LlmClient,
    config: Arc<Mutex<AppConfig>>,
    on_progress: impl Fn(OpProgress),
) {
    // Seed "Last Run" on startup so the UI never shows "Never" after connect.
    let floor = resolve_floor(&db);
    persist_last_run(&db, floor);
    {
        let mut s = state.lock().await;
        s.last_processed = Some(floor);
    }

    loop {
        let cfg = config.lock().await.clone();

        if !cfg.polling_enabled || state.lock().await.paused.load(Ordering::Relaxed) {
            let mut s = state.lock().await;
            s.active_phase = if !cfg.polling_enabled {
                "disabled".into()
            } else {
                "paused".into()
            };
            tokio::time::sleep(Duration::from_secs(10)).await;
            continue;
        }

        let interval = Duration::from_secs(cfg.polling_interval_minutes as u64 * 60);

        let enabled_rules = match db.with_rules(|repo| repo.get_enabled_rules()) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("Failed to load rules for cycle: {}", e);
                tokio::time::sleep(interval).await;
                continue;
            }
        };

        // Last Run tracks attempts; resume floor tracks last success.
        {
            let mut s = state.lock().await;
            s.last_processed = Some(Utc::now());
            s.last_cycle_error = None;
            s.active_phase = "running".into();
        }

        let floor = resolve_floor(&db);
        match run_pipeline(
            &db,
            &state,
            &mut gmail,
            &llm,
            &cfg,
            &effective_query(&cfg.polling_query, floor),
            &enabled_rules,
            true,
            &on_progress,
        )
        .await
        {
            Ok(count) => {
                tracing::info!("Processed {} emails this cycle", count);
                // Only successful cycles move the persisted resume floor.
                let now = Utc::now();
                persist_last_run(&db, now);
                let mut s = state.lock().await;
                s.last_successful = Some(now);
                s.last_cycle_count = count;
                s.active_phase = "idle".into();
            }
            Err(e) => {
                tracing::error!("Processing cycle failed: {}", e);
                let mut s = state.lock().await;
                s.last_cycle_count = 0;
                s.last_cycle_error = Some(e.to_string());
                s.active_phase = "error".into();
                on_progress(OpProgress {
                    phase: "error".into(),
                    processed: 0,
                    total: None,
                    detail: Some(e.to_string()),
                });
            }
        }

        // Hide stale progress between cycles.
        on_progress(OpProgress {
            phase: "idle".into(),
            processed: 0,
            total: None,
            detail: None,
        });

        state.lock().await.active_phase = "idle".into();

        tokio::time::sleep(interval).await;
    }
}

fn insert_history_entry(db: &Arc<Database>, entry: NewHistoryEntry) {
    if let Err(e) = db.with_history(|repo| repo.insert(&entry)) {
        tracing::error!("Failed to insert history entry: {}", e);
    }
}

/// Run one query against a selected rule set.
///
/// This function never updates persisted `processing.last_run`; only the live
/// loop does that so one-off backfills cannot move the resume floor.
async fn run_pipeline(
    db: &Arc<Database>,
    state: &Arc<Mutex<ProcessingState>>,
    gmail: &mut GmailClient,
    llm: &LlmClient,
    config: &AppConfig,
    query: &str,
    rules: &[Rule],
    update_today: bool,
    on_progress: &impl Fn(OpProgress),
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    on_progress(OpProgress {
        phase: "fetching".into(),
        processed: 0,
        total: None,
        detail: Some(query.to_string()),
    });
    let messages = gmail
        .list_messages(query, config.polling_max_per_cycle)
        .await?;
    let total = messages.len();
    on_progress(OpProgress {
        phase: "processing".into(),
        processed: 0,
        total: Some(total),
        detail: None,
    });

    let mut count = 0;

    for msg_ref in &messages {
        let email = match gmail.get_message(&msg_ref.id).await {
            Ok(email) => email,
            Err(e) => {
                insert_history_entry(
                    db,
                    NewHistoryEntry {
                        email_id: msg_ref.id.clone(),
                        email_from: None,
                        email_subject: None,
                        rule_id: None,
                        rule_name: None,
                        action: "FETCH_MESSAGE".into(),
                        status: "error".into(),
                        llm_model: None,
                        llm_response: None,
                        error: Some(e.to_string()),
                        duration_ms: None,
                    },
                );
                count += 1;
                on_progress(OpProgress {
                    phase: "processing".into(),
                    processed: count,
                    total: Some(total),
                    detail: None,
                });
                continue;
            }
        };

        let labels = email.label_ids.clone();

        // Backfills pass a selected subset; live polling passes all enabled rules.
        let matched_rule: Option<Rule> = rules
            .iter()
            .filter(|r| r.enabled && r.conditions.iter().all(|c| matcher::evaluate(c, &email, &labels)))
            .min_by_key(|r| r.priority)
            .cloned();

        let memories: Vec<String> = match &matched_rule {
            Some(rule) => db.with_rule_memory(|repo| {
                repo.list_for_rule(rule.id)
                    .map(|rows| rows.into_iter().map(|e| e.text).collect::<Vec<String>>())
                    .unwrap_or_default()
            }),
            None => Vec::new(),
        };

        let resolved: Option<Resolved> = match &matched_rule {
            Some(rule) => match resolve_rule(llm, rule, &email, &memories).await {
                Ok(v) => v,
                Err(e) => {
                    insert_history_entry(
                        db,
                        NewHistoryEntry {
                            email_id: msg_ref.id.clone(),
                            email_from: extract_header(&email, "From"),
                            email_subject: extract_header(&email, "Subject"),
                            rule_id: Some(rule.id),
                            rule_name: Some(rule.name.clone()),
                            action: "RESOLVE_RULE".into(),
                            status: "error".into(),
                            llm_model: None,
                            llm_response: None,
                            error: Some(e.to_string()),
                            duration_ms: None,
                        },
                    );
                    count += 1;
                    on_progress(OpProgress {
                        phase: "processing".into(),
                        processed: count,
                        total: Some(total),
                        detail: None,
                    });
                    continue;
                }
            },
            None => None,
        };

        match resolved {
            Some(Resolved {
                actions,
                llm_response,
                reasoning: _,
            }) => {
                let rule_id = matched_rule.as_ref().unwrap().id;
                let rule_name = matched_rule.as_ref().unwrap().name.clone();

                if actions.is_empty() {
                    insert_history_entry(
                        db,
                        NewHistoryEntry {
                            email_id: msg_ref.id.clone(),
                            email_from: extract_header(&email, "From"),
                            email_subject: extract_header(&email, "Subject"),
                            rule_id: Some(rule_id),
                            rule_name: Some(rule_name),
                            action: "SKIP".into(),
                            status: "skipped".into(),
                            llm_model: None,
                            llm_response: Some(llm_response),
                            error: Some("No actionable response".into()),
                            duration_ms: None,
                        },
                    );
                } else {
                    let gmail_labels = gmail.list_labels().await.unwrap_or_default();
                    let mut outcomes: Vec<(&ParsedAction, Option<String>)> = Vec::new();
                    for action in &actions {
                        let error = execute_action(gmail, &msg_ref.id, action, &gmail_labels)
                            .await
                            .err()
                            .map(|e| e.to_string());
                        outcomes.push((action, error));
                    }

                    let action_str = actions
                        .iter()
                        .map(|a| format!("{:?}", a))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let errors: Vec<String> = outcomes
                        .iter()
                        .filter_map(|(_, e)| e.clone())
                        .collect();
                    let status = if errors.is_empty() { "success" } else { "error" };

                    insert_history_entry(
                        db,
                        NewHistoryEntry {
                            email_id: msg_ref.id.clone(),
                            email_from: extract_header(&email, "From"),
                            email_subject: extract_header(&email, "Subject"),
                            rule_id: Some(rule_id),
                            rule_name: Some(rule_name),
                            action: action_str,
                            status: status.into(),
                            llm_model: None,
                            llm_response: Some(llm_response),
                            error: if errors.is_empty() {
                                None
                            } else {
                                Some(errors.join("; "))
                            },
                            duration_ms: None,
                        },
                    );
                }
            }
            None => {}
        }

        count += 1;
        on_progress(OpProgress {
            phase: "processing".into(),
            processed: count,
            total: Some(total),
            detail: None,
        });
    }

    let mut s = state.lock().await;
    if update_today {
        s.emails_processed_today += count;
    }

    Ok(count)
}

/// One-off backfill over `[after, before]` applying only `rule_ids`.
///
/// It re-runs matching history by design and increments today's count, but it
/// leaves `processing.last_run` untouched so live polling keeps its resume point.
pub async fn run_backfill(
    db: &Arc<Database>,
    state: &Arc<Mutex<ProcessingState>>,
    gmail: &mut GmailClient,
    llm: &LlmClient,
    config: &AppConfig,
    after: DateTime<Utc>,
    before: DateTime<Utc>,
    rule_ids: &[i64],
    on_progress: &impl Fn(OpProgress),
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let query = format!("{} after:{} before:{}", config.polling_query.trim(), after.timestamp(), before.timestamp());
    let rules = db.with_rules(|repo| repo.get_by_ids(rule_ids))?;
    run_pipeline(db, state, gmail, llm, config, &query, &rules, true, on_progress).await
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
