use std::sync::Arc;

use chrono::{TimeZone, Utc};
use serde::Serialize;
use tokio::sync::Mutex;

use crate::config::AppConfig;
use crate::db::sync_state::GmailSyncState;
use crate::db::Database;
use crate::gmail::models::WatchResponse;
use crate::gmail::GmailClient;
use crate::llm::InferenceRouter;
use crate::processing::{OpProgress, ProcessingState};

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
    _state: &Arc<Mutex<ProcessingState>>,
    gmail: &mut GmailClient,
    _llm: &InferenceRouter,
    config: &AppConfig,
    account_email: &str,
    _trigger: &str,
    on_progress: &impl Fn(OpProgress),
) -> Result<ReplayResult, Box<dyn std::error::Error + Send + Sync>> {
    on_progress(OpProgress {
        source: "sync".into(),
        phase: "sync-fetching".into(),
        processed: 0,
        total: None,
        detail: Some("Recording Gmail arrivals".into()),
        current_email_id: None,
        current_email_from: None,
        current_email_subject: None,
        current_email_sent_at: None,
    });

    let result = crate::workflow::ingest_history(db, gmail, account_email, config).await?;

    Ok(ReplayResult {
        from_history_id: result.from_history_id,
        to_history_id: result.to_history_id,
        touched_messages: result.queued_messages,
        processed_messages: result.queued_messages,
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

fn millis_to_rfc3339(raw: &str) -> Option<String> {
    let millis = raw.parse::<i64>().ok()?;
    Utc.timestamp_millis_opt(millis)
        .single()
        .map(|dt| dt.to_rfc3339())
}
