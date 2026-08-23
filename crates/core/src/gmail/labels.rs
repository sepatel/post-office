use chrono::Utc;

use super::client::GmailClient;
use super::models::Label;
use crate::db::Database;

/// Labels only change out-of-band in Gmail — the app never creates them — so a
/// plain staleness window needs no write-through invalidation.
const CACHE_TTL_MINUTES: i64 = 10;

/// Per-account label list, refetched only once the cached copy goes stale.
///
/// A failed refresh degrades to the stale copy rather than an empty list: an
/// empty catalog silently strips label actions and makes execution fail with
/// "unknown label", which is far worse than acting on slightly old names. Only
/// a failure with nothing cached at all surfaces as an error.
pub async fn cached_labels(
    db: &Database,
    gmail: &mut GmailClient,
    account_email: &str,
) -> Result<Vec<Label>, super::GmailError> {
    let cached = db
        .with_labels(|repo| repo.get(account_email))
        .ok()
        .flatten();
    if let Some(entry) = &cached {
        if crate::db::parse_stored_utc(&entry.fetched_at)
            .is_some_and(|at| Utc::now() - at < chrono::Duration::minutes(CACHE_TTL_MINUTES))
        {
            return Ok(entry.labels.clone());
        }
    }

    match refresh_labels(db, gmail, account_email).await {
        Ok(labels) => Ok(labels),
        Err(error) => match cached {
            Some(entry) => {
                tracing::warn!("Serving stale labels for {account_email}: {error}");
                Ok(entry.labels)
            }
            None => Err(error),
        },
    }
}

/// Fetches and re-caches labels regardless of staleness.
pub async fn refresh_labels(
    db: &Database,
    gmail: &mut GmailClient,
    account_email: &str,
) -> Result<Vec<Label>, super::GmailError> {
    let labels = gmail.list_labels().await?;
    if let Err(error) = db.with_labels(|repo| repo.upsert(account_email, &labels)) {
        tracing::warn!("Failed to cache labels for {account_email}: {error}");
    }
    Ok(labels)
}
