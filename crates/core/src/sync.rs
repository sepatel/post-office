use std::collections::BTreeSet;
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use serde::Serialize;
use tokio::sync::Mutex;

use crate::config::AppConfig;
use crate::db::sync_state::GmailSyncState;
use crate::db::Database;
use crate::gmail::models::{HistoryRecord, WatchResponse};
use crate::gmail::{GmailClient, GmailError};
use crate::llm::InferenceRouter;
use crate::processing::{
    process_pending_inference_jobs, run_for_message_ids, OpProgress, ProcessingState,
};

#[derive(Debug, Clone, Serialize)]
pub struct ReplayResult {
    pub from_history_id: String,
    pub to_history_id: String,
    pub touched_messages: usize,
    pub processed_messages: usize,
}

pub async fn ensure_cursor(
    db: &Arc<Database>,
    gmail: &mut GmailClient,
    account_email: &str,
) -> Result<GmailSyncState, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(existing) = db.with_sync_state(|repo| repo.get(account_email))? {
        return Ok(existing);
    }

    let profile = gmail.get_profile().await?;
    db.with_sync_state(|repo| {
        repo.upsert_cursor(account_email, &profile.history_id, Some("seeded"))
    })?;

    db.with_sync_state(|repo| repo.get(account_email))?
        .ok_or_else(|| "failed to load seeded sync cursor".into())
}

#[allow(clippy::too_many_arguments)]
pub async fn replay_history(
    db: &Arc<Database>,
    state: &Arc<Mutex<ProcessingState>>,
    gmail: &mut GmailClient,
    llm: &InferenceRouter,
    config: &AppConfig,
    account_email: &str,
    trigger: &str,
    on_progress: &impl Fn(OpProgress),
) -> Result<ReplayResult, Box<dyn std::error::Error + Send + Sync>> {
    let sync_state = ensure_cursor(db, gmail, account_email).await?;
    let from_history_id = sync_state.last_history_id;

    let mut page_token: Option<String> = None;
    let mut max_history_id = from_history_id.clone();
    let mut message_ids: BTreeSet<String> = BTreeSet::new();

    on_progress(OpProgress {
        source: "sync".into(),
        phase: "sync-fetching".into(),
        processed: 0,
        total: None,
        detail: Some(format!("history from {}", from_history_id)),
        current_email_id: None,
        current_email_from: None,
        current_email_subject: None,
        current_email_sent_at: None,
    });

    loop {
        let page = match gmail
            .list_history_page(&from_history_id, page_token.as_deref())
            .await
        {
            Ok(page) => page,
            Err(GmailError::Api { code: 404, .. }) => {
                let profile = gmail.get_profile().await?;
                db.with_sync_state(|repo| {
                    repo.upsert_cursor(account_email, &profile.history_id, Some("resynced"))?;
                    repo.set_sync_error(
                        account_email,
                        Some("history cursor expired; reseeded to current mailbox cursor"),
                    )?;
                    Ok::<(), rusqlite::Error>(())
                })?;
                return Ok(ReplayResult {
                    from_history_id,
                    to_history_id: profile.history_id,
                    touched_messages: 0,
                    processed_messages: 0,
                });
            }
            Err(e) => return Err(e.into()),
        };
        if let Some(latest) = page.history_id.as_deref() {
            max_history_id = max_history(max_history_id, latest);
        }
        if let Some(records) = page.history.as_ref() {
            for record in records {
                max_history_id = max_history(max_history_id, &record.id);
                collect_message_ids(&mut message_ids, record);
            }
        }
        page_token = page.next_page_token;
        if page_token.is_none() {
            break;
        }
    }

    let ids: Vec<String> = message_ids.into_iter().collect();
    let processed_messages = if ids.is_empty() {
        0
    } else {
        run_for_message_ids(db, state, gmail, llm, config, &ids, on_progress).await?
    };

    if let Err(error) =
        process_pending_inference_jobs(db, state, gmail, llm, config, on_progress).await
    {
        tracing::warn!(
            "Queued inference worker failed during sync replay: {}",
            error
        );
    }

    db.with_sync_state(|repo| {
        repo.upsert_cursor(account_email, &max_history_id, Some("active"))?;
        repo.mark_history_pull(account_email)?;
        repo.set_sync_error(account_email, None)?;
        repo.insert_cursor_log(
            account_email,
            Some(&from_history_id),
            &max_history_id,
            trigger,
            processed_messages,
        )?;
        Ok::<(), rusqlite::Error>(())
    })?;

    Ok(ReplayResult {
        from_history_id,
        to_history_id: max_history_id,
        touched_messages: ids.len(),
        processed_messages,
    })
}

pub async fn start_watch(
    db: &Arc<Database>,
    gmail: &mut GmailClient,
    account_email: &str,
    topic_name: &str,
    label_ids: &[String],
) -> Result<WatchResponse, Box<dyn std::error::Error + Send + Sync>> {
    let watch = gmail.watch(topic_name, label_ids).await?;
    let expiration = millis_to_rfc3339(&watch.expiration);
    db.with_sync_state(|repo| {
        repo.upsert_cursor(account_email, &watch.history_id, Some("watching"))?;
        repo.update_watch(account_email, expiration.as_deref(), "watching")?;
        repo.set_sync_error(account_email, None)?;
        Ok::<(), rusqlite::Error>(())
    })?;
    Ok(watch)
}

pub async fn stop_watch(
    db: &Arc<Database>,
    gmail: &mut GmailClient,
    account_email: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    gmail.stop_watch().await?;
    db.with_sync_state(|repo| {
        repo.update_watch(account_email, None, "stopped")?;
        Ok::<(), rusqlite::Error>(())
    })?;
    Ok(())
}

pub fn queue_notification(
    db: &Arc<Database>,
    account_email: &str,
    event_key: &str,
    min_history_id: &str,
) -> Result<(), rusqlite::Error> {
    db.with_sync_state(|repo| {
        repo.upsert_pending_event(account_email, event_key, min_history_id)?;
        repo.mark_notification_received(account_email)?;
        Ok(())
    })
}

fn max_history(current: String, candidate: &str) -> String {
    match (current.parse::<u128>(), candidate.parse::<u128>()) {
        (Ok(left), Ok(right)) if right > left => candidate.to_string(),
        _ => current,
    }
}

fn collect_message_ids(ids: &mut BTreeSet<String>, record: &HistoryRecord) {
    if let Some(messages) = record.messages.as_ref() {
        for m in messages {
            ids.insert(m.id.clone());
        }
    }
    if let Some(messages) = record.messages_added.as_ref() {
        for m in messages {
            ids.insert(m.message.id.clone());
        }
    }
    if let Some(messages) = record.labels_added.as_ref() {
        for m in messages {
            ids.insert(m.message.id.clone());
        }
    }
    if let Some(messages) = record.labels_removed.as_ref() {
        for m in messages {
            ids.insert(m.message.id.clone());
        }
    }
}

fn millis_to_rfc3339(raw: &str) -> Option<String> {
    let millis = raw.parse::<i64>().ok()?;
    Utc.timestamp_millis_opt(millis)
        .single()
        .map(|dt| dt.to_rfc3339())
}
