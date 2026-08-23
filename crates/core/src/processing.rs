use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use serde::Serialize;
use tokio::sync::Mutex;

use crate::config::AppConfig;
use crate::db::history::NewHistoryEntry;
use crate::db::llm_usage::NewLlmUsageEntry;
use crate::db::Database;
use crate::gmail::models::{Message, MessageRef};
use crate::rules::engine::{no_action_reason, resolve_rule, Outcome, Resolved};
use crate::rules::evaluation::bulk_resolve_for_rule;
use crate::rules::matcher;

const LAST_RUN_KEY: &str = "last_run";
const LAST_SUCCESSFUL_KEY: &str = "last_successful";

/// Live progress for an operation (a polling cycle or a one-off backfill),
/// emitted so the UI can show a status bar instead of freezing on a blocking
/// call. `phase` tells the UI which spinner/label to render.
#[derive(Debug, Clone, Serialize)]
pub struct OpProgress {
    pub source: String,
    pub phase: String,
    pub processed: usize,
    pub total: Option<usize>,
    pub detail: Option<String>,
    pub current_email_id: Option<String>,
    pub current_email_from: Option<String>,
    pub current_email_subject: Option<String>,
    pub current_email_sent_at: Option<String>,
}
use crate::gmail::GmailClient;
use crate::llm::InferenceRouter;
use crate::rules::actions::execute_actions;
use crate::rules::models::Rule;
use crate::rules::response_parser::ParsedAction;

pub struct ProcessingState {
    pub paused: Arc<AtomicBool>,
    pub stop_requested: Arc<AtomicBool>,
    pub backfill_running: Arc<AtomicBool>,
    pub backfill_cancel_requested: Arc<AtomicBool>,
    // Keep "Last Run" moving even when cycles fail.
    pub last_processed: Option<chrono::DateTime<Utc>>,
    pub last_successful: Option<chrono::DateTime<Utc>>,
    pub emails_processed_today: usize,
    pub last_cycle_count: usize,
    pub last_cycle_error: Option<String>,
    pub active_phase: String,
    pub current_progress: Option<OpProgress>,
}

#[derive(Debug, Clone, Copy)]
enum PipelineSource {
    Cycle,
    Backfill,
    Sync,
}

impl PipelineSource {
    fn as_str(self) -> &'static str {
        match self {
            PipelineSource::Cycle => "cycle",
            PipelineSource::Backfill => "backfill",
            PipelineSource::Sync => "sync",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "backfill" => PipelineSource::Backfill,
            "sync" => PipelineSource::Sync,
            _ => PipelineSource::Cycle,
        }
    }
}

impl ProcessingState {
    pub fn new() -> Self {
        Self {
            paused: Arc::new(AtomicBool::new(false)),
            stop_requested: Arc::new(AtomicBool::new(false)),
            backfill_running: Arc::new(AtomicBool::new(false)),
            backfill_cancel_requested: Arc::new(AtomicBool::new(false)),
            last_processed: None,
            last_successful: None,
            emails_processed_today: 0,
            last_cycle_count: 0,
            last_cycle_error: None,
            active_phase: "idle".into(),
            current_progress: None,
        }
    }
}

impl Default for ProcessingState {
    fn default() -> Self {
        Self::new()
    }
}

pub fn hydrate_processing_state(db: &Database, state: &mut ProcessingState, account_email: &str) {
    state.last_processed = db
        .with_config(|repo| {
            repo.get(&processing_key(account_email, LAST_RUN_KEY))
                .ok()
                .flatten()
        })
        .and_then(|v| crate::db::parse_stored_utc(&v));
    state.last_successful = db
        .with_config(|repo| {
            repo.get(&processing_key(account_email, LAST_SUCCESSFUL_KEY))
                .ok()
                .flatten()
        })
        .and_then(|v| crate::db::parse_stored_utc(&v));
}

pub async fn mark_last_successful(
    db: &Arc<Database>,
    state: &Arc<Mutex<ProcessingState>>,
    account_email: &str,
    ts: DateTime<Utc>,
) {
    persist_last_successful(db, account_email, ts);
    state.lock().await.last_successful = Some(ts);
}

/// Resolve the timestamp from which polling should fetch messages. We resume
/// from the most recent successful run so a restart never re-scans history:
/// 1. the persisted `processing.last_run` (set after each cycle), else
/// 2. the newest `history.created_at` (messages we've already handled), else
/// 3. `now` — a brand-new install anchors to the present and ignores old mail.
fn resolve_floor(db: &Arc<Database>, account_email: &str) -> DateTime<Utc> {
    if let Some(stored) = db.with_config(|repo| {
        repo.get(&processing_key(account_email, LAST_RUN_KEY))
            .ok()
            .flatten()
    }) {
        if let Some(ts) = crate::db::parse_stored_utc(&stored) {
            return ts;
        }
    }
    if let Some(newest) =
        db.with_history(|repo| repo.latest_created_at(account_email).ok().flatten())
    {
        if let Some(ts) = crate::db::parse_stored_utc(&newest) {
            return ts;
        }
    }
    Utc::now()
}

/// Persist the last successful run so restarts resume from here instead of
/// walking the entire mailbox again.
fn persist_last_run(db: &Arc<Database>, account_email: &str, ts: DateTime<Utc>) {
    let value = ts.to_rfc3339();
    let _ = db.with_config(|repo| repo.set(&processing_key(account_email, LAST_RUN_KEY), &value));
}

fn persist_last_successful(db: &Arc<Database>, account_email: &str, ts: DateTime<Utc>) {
    let value = ts.to_rfc3339();
    let _ = db
        .with_config(|repo| repo.set(&processing_key(account_email, LAST_SUCCESSFUL_KEY), &value));
}

fn processing_key(account_email: &str, suffix: &str) -> String {
    format!("processing.{account_email}.{suffix}")
}

fn should_clear_progress(phase: &str) -> bool {
    matches!(phase, "idle" | "error" | "stopped")
}

async fn publish_progress(
    state: &Arc<Mutex<ProcessingState>>,
    on_progress: &impl Fn(OpProgress),
    progress: OpProgress,
) {
    {
        let mut s = state.lock().await;
        s.active_phase = progress.phase.clone();
        if should_clear_progress(&progress.phase) {
            s.current_progress = None;
        } else {
            s.current_progress = Some(progress.clone());
        }
    }
    on_progress(progress);
}

fn op_progress(
    source: PipelineSource,
    phase: &str,
    processed: usize,
    total: Option<usize>,
    detail: Option<String>,
) -> OpProgress {
    OpProgress {
        source: source.as_str().into(),
        phase: phase.into(),
        processed,
        total,
        detail,
        current_email_id: None,
        current_email_from: None,
        current_email_subject: None,
        current_email_sent_at: None,
    }
}

fn with_current_email(
    mut progress: OpProgress,
    email_id: Option<&str>,
    email_from: Option<&str>,
    email_subject: Option<&str>,
    email_sent_at: Option<&str>,
) -> OpProgress {
    progress.current_email_id = email_id.map(ToString::to_string);
    progress.current_email_from = email_from.map(ToString::to_string);
    progress.current_email_subject = email_subject.map(ToString::to_string);
    progress.current_email_sent_at = email_sent_at.map(ToString::to_string);
    progress
}

#[derive(Debug, Clone, Copy)]
pub struct PipelineRunResult {
    pub processed: usize,
    pub discovered: usize,
    pub stopped: bool,
}

/// Build the Gmail query window, appending `after:<unix_seconds>` and
/// optionally `before:<unix_seconds>`.
fn effective_query(base: &str, floor: DateTime<Utc>, until: Option<DateTime<Utc>>) -> String {
    let after = floor.timestamp();
    let trimmed = base.trim();
    let mut query = if trimmed.is_empty() {
        format!("after:{after}")
    } else {
        format!("{trimmed} after:{after}")
    };
    if let Some(upper) = until {
        query.push_str(&format!(" before:{}", upper.timestamp() + 1));
    }
    query
}

pub async fn run_processing_loop(
    db: Arc<Database>,
    state: Arc<Mutex<ProcessingState>>,
    mut gmail: GmailClient,
    config: Arc<Mutex<AppConfig>>,
    account_email: String,
    on_progress: impl Fn(OpProgress),
) {
    // Seed "Last Run" on startup so the UI never shows "Never" after connect.
    let floor = resolve_floor(&db, &account_email);
    persist_last_run(&db, &account_email, floor);
    {
        let mut s = state.lock().await;
        s.last_processed = Some(floor);
    }

    loop {
        if state.lock().await.stop_requested.load(Ordering::Relaxed) {
            break;
        }
        let cfg = config.lock().await.clone();

        if !cfg.polling_enabled || state.lock().await.paused.load(Ordering::Relaxed) {
            let mut s = state.lock().await;
            s.active_phase = if !cfg.polling_enabled {
                "disabled".into()
            } else {
                "paused".into()
            };
            s.current_progress = None;
            tokio::time::sleep(Duration::from_secs(10)).await;
            continue;
        }

        let interval = Duration::from_secs(cfg.polling_interval_minutes as u64 * 60);

        let enabled_rules = match db.with_rules(|repo| repo.get_enabled_rules(&account_email)) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("Failed to load rules for cycle: {}", e);
                tokio::time::sleep(interval).await;
                continue;
            }
        };

        // Last Run tracks attempts; resume floor tracks last success.
        let cycle_started = Utc::now();
        {
            let mut s = state.lock().await;
            s.last_processed = Some(cycle_started);
            s.last_cycle_error = None;
            s.active_phase = "running".into();
        }

        let floor = resolve_floor(&db, &account_email);
        let llm = InferenceRouter::from_config(&cfg).with_database(db.as_ref().clone());
        let stop_requested = state.lock().await.stop_requested.clone();
        match run_pipeline(
            &db,
            &account_email,
            &state,
            &mut gmail,
            &llm,
            &cfg,
            &effective_query(&cfg.polling_query, floor, Some(cycle_started)),
            &enabled_rules,
            Some(cfg.polling_max_per_cycle),
            Some(stop_requested.as_ref()),
            true,
            PipelineSource::Cycle,
            &on_progress,
        )
        .await
        {
            Ok(result) => {
                tracing::info!("Processed {} emails this cycle", result.processed);
                // Only successful cycles move the persisted resume floor.
                persist_last_run(&db, &account_email, cycle_started);
                persist_last_successful(&db, &account_email, cycle_started);
                let _ = db.with_accounts(|repo| repo.record_success(&account_email));
                let mut s = state.lock().await;
                s.last_successful = Some(cycle_started);
                s.last_cycle_count = result.processed;
                s.active_phase = "idle".into();
                s.current_progress = None;
            }
            Err(e) => {
                tracing::error!("Processing cycle failed: {}", e);
                let _ = db.with_accounts(|repo| repo.record_error(&account_email, &e.to_string()));
                let mut s = state.lock().await;
                s.last_cycle_count = 0;
                s.last_cycle_error = Some(e.to_string());
                s.active_phase = "error".into();
                drop(s);
                publish_progress(
                    &state,
                    &on_progress,
                    op_progress(PipelineSource::Cycle, "error", 0, None, Some(e.to_string())),
                )
                .await;
            }
        }

        if let Err(e) = process_pending_inference_jobs(
            &db,
            &account_email,
            &state,
            &mut gmail,
            &llm,
            &cfg,
            &on_progress,
        )
        .await
        {
            tracing::warn!("Inference retry worker failed: {}", e);
        }

        // Hide stale progress between cycles.
        publish_progress(
            &state,
            &on_progress,
            op_progress(PipelineSource::Cycle, "idle", 0, None, None),
        )
        .await;

        state.lock().await.active_phase = "idle".into();

        tokio::time::sleep(interval).await;
    }
}

pub async fn process_pending_inference_jobs(
    db: &Arc<Database>,
    account_email: &str,
    state: &Arc<Mutex<ProcessingState>>,
    gmail: &mut GmailClient,
    llm: &InferenceRouter,
    config: &AppConfig,
    on_progress: &impl Fn(OpProgress),
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let mut processed = 0;
    while processed < 20 {
        let Some(job) = db.with_inference(|repo| repo.claim_next(account_email))? else {
            break;
        };

        let result = process_inference_job(db, account_email, gmail, llm, config, &job).await;
        match result {
            Ok(JobOutcome::Succeeded) => {
                db.with_inference(|repo| repo.mark_success(account_email, job.id))?;
            }
            Ok(JobOutcome::Skipped(reason)) => {
                db.with_inference(|repo| repo.mark_skipped(account_email, job.id, &reason))?;
            }
            Err(error) => {
                let error = error.to_string();
                let delay = retry_delay(job.attempt_count);
                db.with_inference(|repo| {
                    repo.record_attempt(job.id, None, None, "error", Some(&error))?;
                    if job.attempt_count + 1 >= MAX_INFERENCE_ATTEMPTS {
                        repo.mark_dead_letter(account_email, job.id, &error)
                    } else {
                        repo.mark_retry(account_email, job.id, &error, delay)
                    }
                })?;
            }
        }

        processed += 1;
        publish_progress(
            state,
            on_progress,
            with_current_email(
                op_progress(
                    PipelineSource::Cycle,
                    "retrying",
                    processed,
                    None,
                    Some("Retrying queued inference".into()),
                ),
                Some(&job.email_id),
                None,
                None,
                None,
            ),
        )
        .await;
    }
    Ok(processed)
}

enum JobOutcome {
    Succeeded,
    Skipped(String),
}

async fn process_inference_job(
    db: &Arc<Database>,
    account_email: &str,
    gmail: &mut GmailClient,
    llm: &InferenceRouter,
    config: &AppConfig,
    job: &crate::db::inference::InferenceJob,
) -> Result<JobOutcome, Box<dyn std::error::Error + Send + Sync>> {
    let Some(rule_id) = job.rule_id else {
        return Ok(JobOutcome::Skipped("Rule was deleted".into()));
    };
    let Some(rule) = db.with_rules(|repo| repo.get_by_id(account_email, rule_id))? else {
        return Ok(JobOutcome::Skipped("Rule was deleted".into()));
    };
    if !rule.enabled {
        return Ok(JobOutcome::Skipped("Rule is disabled".into()));
    }

    let email = gmail.get_message(&job.email_id).await?;
    if !rule
        .conditions
        .iter()
        .all(|condition| matcher::evaluate(condition, &email, &email.label_ids))
    {
        return Ok(JobOutcome::Skipped("Rule no longer matches".into()));
    }

    let memories = db.with_rule_memory(|repo| {
        repo.list_for_rule(rule.id)
            .map(|rows| rows.into_iter().map(|entry| entry.text).collect::<Vec<_>>())
            .unwrap_or_default()
    });
    let labels = crate::gmail::cached_labels(db, gmail, account_email)
        .await
        .unwrap_or_default();
    let resolved = resolve_rule(llm, &rule, &email, &memories, &labels)
        .await
        .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { Box::new(error) })?;
    match resolved.outcome {
        // A retried email still has to reach lower-priority rules, otherwise a
        // transient failure on one rule silently exempts it from all the others.
        Outcome::NoMatch => {
            let rules = db.with_rules(|repo| repo.get_enabled_rules(account_email))?;
            process_fallthrough(
                db,
                account_email,
                gmail,
                llm,
                config,
                &email,
                extract_header(&email, "From").as_deref(),
                extract_header(&email, "Subject").as_deref(),
                &rules,
                rule.id,
                &labels,
                PipelineSource::parse(&job.source),
            )
            .await?;
            return Ok(JobOutcome::Skipped("Rule did not match".into()));
        }
        // The re-ask asked about this email alone, so a reply that still cannot
        // be read is the model's answer, not a correlation problem to retry.
        Outcome::Unparsed => {
            return Ok(JobOutcome::Skipped(
                resolved
                    .diagnostic
                    .unwrap_or_else(|| "Invalid LLM decision".into()),
            ));
        }
        Outcome::Matched => {}
    }

    if resolved.actions.is_empty() {
        let history_id = insert_history_entry(
            db,
            account_email,
            NewHistoryEntry {
                email_id: email.id.clone(),
                email_from: extract_header(&email, "From"),
                email_subject: extract_header(&email, "Subject"),
                rule_id: Some(rule.id),
                rule_name: Some(rule.name.clone()),
                action: "SKIP".into(),
                status: "skipped".into(),
                llm_model: resolved.llm_model.clone(),
                llm_response: Some(resolved.llm_response.clone()),
                error: Some(no_action_reason(
                    &rule,
                    &labels,
                    resolved.diagnostic.as_deref(),
                )),
                duration_ms: resolved.llm_duration_ms.map(|value| value as i64),
                llm_provider: resolved.llm_provider.clone(),
                policy_id: Some(rule.inference_policy.clone()),
            },
        );
        if let Some(model) = resolved.llm_model.as_ref() {
            insert_llm_usage_entry(
                db,
                account_email,
                NewLlmUsageEntry {
                    history_id,
                    email_id: email.id.clone(),
                    rule_id: rule.id,
                    model: Some(model.clone()),
                    prompt_tokens: resolved.prompt_tokens.map(|value| value as i64),
                    completion_tokens: resolved.completion_tokens.map(|value| value as i64),
                    total_tokens: resolved.total_tokens.map(|value| value as i64),
                    duration_ms: resolved.llm_duration_ms.map(|value| value as i64),
                    estimated_cost_usd: estimate_llm_cost(
                        config,
                        resolved.prompt_tokens,
                        resolved.completion_tokens,
                        resolved.total_tokens,
                    ),
                    source: "retry".into(),
                },
            );
        }
        retry_fallthrough(
            db,
            account_email,
            gmail,
            llm,
            config,
            &rule,
            &email,
            &labels,
            job,
        )
        .await?;
        return Ok(JobOutcome::Succeeded);
    }

    let error = execute_actions(gmail, &email.id, &resolved.actions, &labels)
        .await
        .err()
        .map(|error| error.to_string());

    let action_str = resolved
        .actions
        .iter()
        .map(|action| history_action_label(action, &labels))
        .collect::<Vec<_>>()
        .join(", ");
    let history_id = insert_history_entry(
        db,
        account_email,
        NewHistoryEntry {
            email_id: email.id.clone(),
            email_from: extract_header(&email, "From"),
            email_subject: extract_header(&email, "Subject"),
            rule_id: Some(rule.id),
            rule_name: Some(rule.name.clone()),
            action: action_str,
            status: if error.is_none() { "success" } else { "error" }.into(),
            llm_model: resolved.llm_model.clone(),
            llm_response: Some(resolved.llm_response.clone()),
            error: error.clone(),
            duration_ms: resolved.llm_duration_ms.map(|value| value as i64),
            llm_provider: resolved.llm_provider.clone(),
            policy_id: Some(rule.inference_policy.clone()),
        },
    );
    if let Some(model) = resolved.llm_model.as_ref() {
        insert_llm_usage_entry(
            db,
            account_email,
            NewLlmUsageEntry {
                history_id,
                email_id: email.id.clone(),
                rule_id: rule.id,
                model: Some(model.clone()),
                prompt_tokens: resolved.prompt_tokens.map(|value| value as i64),
                completion_tokens: resolved.completion_tokens.map(|value| value as i64),
                total_tokens: resolved.total_tokens.map(|value| value as i64),
                duration_ms: resolved.llm_duration_ms.map(|value| value as i64),
                estimated_cost_usd: estimate_llm_cost(
                    config,
                    resolved.prompt_tokens,
                    resolved.completion_tokens,
                    resolved.total_tokens,
                ),
                source: "retry".into(),
            },
        );
    }

    if let Some(error) = error {
        return Err(error.into());
    }
    retry_fallthrough(
        db,
        account_email,
        gmail,
        llm,
        config,
        &rule,
        &email,
        &labels,
        job,
    )
    .await?;
    Ok(JobOutcome::Succeeded)
}

#[expect(
    clippy::too_many_arguments,
    reason = "Mirrors the pipeline's fallthrough context, which the retry path lacks."
)]
async fn retry_fallthrough(
    db: &Arc<Database>,
    account_email: &str,
    gmail: &mut GmailClient,
    llm: &InferenceRouter,
    config: &AppConfig,
    rule: &Rule,
    email: &Message,
    labels: &[crate::gmail::models::Label],
    job: &crate::db::inference::InferenceJob,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !continues_past(rule, Outcome::Matched) {
        return Ok(());
    }
    let rules = db.with_rules(|repo| repo.get_enabled_rules(account_email))?;
    process_fallthrough(
        db,
        account_email,
        gmail,
        llm,
        config,
        email,
        extract_header(email, "From").as_deref(),
        extract_header(email, "Subject").as_deref(),
        &rules,
        rule.id,
        labels,
        PipelineSource::parse(&job.source),
    )
    .await
}

fn retry_delay(attempt_count: i64) -> u64 {
    60u64.saturating_mul(2u64.saturating_pow(attempt_count.clamp(0, 6) as u32))
}

/// Past this the backoff is over an hour a try, so the job is parked for manual
/// retry instead of being re-run forever.
const MAX_INFERENCE_ATTEMPTS: i64 = 7;

fn insert_history_entry(
    db: &Arc<Database>,
    account_email: &str,
    entry: NewHistoryEntry,
) -> Option<i64> {
    match db.with_history(|repo| repo.insert(account_email, &entry)) {
        Ok(id) => Some(id),
        Err(e) => {
            tracing::error!("Failed to insert history entry: {}", e);
            None
        }
    }
}

fn insert_llm_usage_entry(db: &Arc<Database>, account_email: &str, entry: NewLlmUsageEntry) {
    if let Err(e) = db.with_llm_usage(|repo| repo.insert(account_email, &entry)) {
        tracing::error!("Failed to insert llm usage entry: {}", e);
    }
}

fn estimate_llm_cost(
    config: &AppConfig,
    prompt_tokens: Option<u32>,
    completion_tokens: Option<u32>,
    total_tokens: Option<u32>,
) -> Option<f64> {
    if let (Some(prompt), Some(completion)) = (prompt_tokens, completion_tokens) {
        return Some(
            (prompt as f64 * config.llm_input_cost_per_million_usd / 1_000_000.0)
                + (completion as f64 * config.llm_output_cost_per_million_usd / 1_000_000.0),
        );
    }

    let total = total_tokens? as f64;
    let blended_rate =
        (config.llm_input_cost_per_million_usd + config.llm_output_cost_per_million_usd) / 2.0;
    Some(total * blended_rate / 1_000_000.0)
}

/// Run one query against a selected rule set.
///
/// This function never updates persisted `processing.last_run`; only the live
/// loop does that so one-off backfills cannot move the resume floor.
#[allow(clippy::too_many_arguments)]
async fn run_pipeline(
    db: &Arc<Database>,
    account_email: &str,
    state: &Arc<Mutex<ProcessingState>>,
    gmail: &mut GmailClient,
    llm: &InferenceRouter,
    config: &AppConfig,
    query: &str,
    rules: &[Rule],
    message_limit: Option<u32>,
    cancel_requested: Option<&AtomicBool>,
    update_today: bool,
    source: PipelineSource,
    on_progress: &impl Fn(OpProgress),
) -> Result<PipelineRunResult, Box<dyn std::error::Error + Send + Sync>> {
    let total_limit = message_limit.map(|v| v as usize);
    publish_progress(
        state,
        on_progress,
        op_progress(
            source,
            "fetching",
            0,
            total_limit,
            Some("Discovering matching emails".into()),
        ),
    )
    .await;

    let mut messages: Vec<MessageRef> = Vec::new();
    let mut page_token: Option<String> = None;

    loop {
        if cancel_requested
            .map(|flag| flag.load(Ordering::Relaxed))
            .unwrap_or(false)
        {
            publish_progress(
                state,
                on_progress,
                op_progress(
                    source,
                    "stopped",
                    0,
                    Some(messages.len()),
                    Some("Backfill stopped".into()),
                ),
            )
            .await;
            return Ok(PipelineRunResult {
                processed: 0,
                discovered: messages.len(),
                stopped: true,
            });
        }

        let page_size = match total_limit {
            Some(limit) => {
                if messages.len() >= limit {
                    break;
                }
                (limit.saturating_sub(messages.len())).min(500) as u32
            }
            None => 500,
        };

        if page_size == 0 {
            break;
        }

        let page = gmail
            .list_messages_page(query, page_size, page_token.as_deref())
            .await?;
        let mut page_messages = page.messages.unwrap_or_default();

        if let Some(limit) = total_limit {
            let remaining = limit.saturating_sub(messages.len());
            if page_messages.len() > remaining {
                page_messages.truncate(remaining);
            }
        }

        messages.extend(page_messages);
        publish_progress(
            state,
            on_progress,
            op_progress(
                source,
                "fetching",
                messages.len(),
                total_limit,
                Some(format!(
                    "Discovered {} email{}",
                    messages.len(),
                    if messages.len() == 1 { "" } else { "s" }
                )),
            ),
        )
        .await;

        page_token = page.next_page_token;
        if page_token.is_none() {
            break;
        }
    }

    process_message_refs(
        db,
        account_email,
        state,
        gmail,
        llm,
        config,
        &messages,
        rules,
        update_today,
        source,
        cancel_requested,
        on_progress,
    )
    .await
}

#[expect(
    clippy::too_many_arguments,
    reason = "The public sync entry point accepts the pipeline dependencies directly."
)]
pub async fn run_for_message_ids(
    db: &Arc<Database>,
    account_email: &str,
    state: &Arc<Mutex<ProcessingState>>,
    gmail: &mut GmailClient,
    llm: &InferenceRouter,
    config: &AppConfig,
    message_ids: &[String],
    on_progress: &impl Fn(OpProgress),
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let rules = db.with_rules(|repo| repo.get_enabled_rules(account_email))?;
    let refs: Vec<MessageRef> = message_ids
        .iter()
        .map(|id| MessageRef {
            id: id.clone(),
            thread_id: String::new(),
        })
        .collect();

    let result = process_message_refs(
        db,
        account_email,
        state,
        gmail,
        llm,
        config,
        &refs,
        &rules,
        true,
        PipelineSource::Sync,
        None,
        on_progress,
    )
    .await?;

    Ok(result.processed)
}

#[allow(clippy::too_many_arguments)]
async fn process_message_refs(
    db: &Arc<Database>,
    account_email: &str,
    state: &Arc<Mutex<ProcessingState>>,
    gmail: &mut GmailClient,
    llm: &InferenceRouter,
    config: &AppConfig,
    messages: &[MessageRef],
    rules: &[Rule],
    update_today: bool,
    source: PipelineSource,
    cancel_requested: Option<&AtomicBool>,
    on_progress: &impl Fn(OpProgress),
) -> Result<PipelineRunResult, Box<dyn std::error::Error + Send + Sync>> {
    struct MatchedEmail {
        email: Message,
        email_from: Option<String>,
        email_subject: Option<String>,
        email_sent_at: Option<String>,
    }

    let total = messages.len();
    publish_progress(
        state,
        on_progress,
        op_progress(source, "processing", 0, Some(total), None),
    )
    .await;

    let mut count = 0;
    let mut grouped: Vec<(Rule, Vec<MatchedEmail>)> = Vec::new();

    for msg_ref in messages {
        if cancel_requested
            .map(|flag| flag.load(Ordering::Relaxed))
            .unwrap_or(false)
        {
            publish_progress(
                state,
                on_progress,
                op_progress(
                    source,
                    "stopped",
                    count,
                    Some(total),
                    Some("Backfill stopped".into()),
                ),
            )
            .await;

            if update_today {
                state.lock().await.emails_processed_today += count;
            }

            return Ok(PipelineRunResult {
                processed: count,
                discovered: total,
                stopped: true,
            });
        }

        publish_progress(
            state,
            on_progress,
            with_current_email(
                op_progress(
                    source,
                    "processing",
                    count,
                    Some(total),
                    Some("Loading email".into()),
                ),
                Some(&msg_ref.id),
                None,
                None,
                None,
            ),
        )
        .await;

        let email = match gmail.get_message(&msg_ref.id).await {
            Ok(email) => email,
            Err(e) => {
                insert_history_entry(
                    db,
                    account_email,
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
                        llm_provider: None,
                        policy_id: None,
                    },
                );
                count += 1;
                publish_progress(
                    state,
                    on_progress,
                    with_current_email(
                        op_progress(source, "processing", count, Some(total), None),
                        Some(&msg_ref.id),
                        None,
                        None,
                        None,
                    ),
                )
                .await;
                continue;
            }
        };

        let email_from = extract_header(&email, "From");
        let email_subject = extract_header(&email, "Subject");
        let email_sent_at = extract_email_sent_at(&email);

        if let Some(sent_at) = email_sent_at.as_deref() {
            if let Err(e) =
                db.with_history(|repo| repo.upsert_email_sent_at(account_email, &email.id, sent_at))
            {
                tracing::warn!("Failed to upsert email sent_at for {}: {}", email.id, e);
            }
        }

        let labels = email.label_ids.clone();
        let matched_rule: Option<Rule> = rules
            .iter()
            .filter(|r| {
                r.enabled
                    && r.conditions
                        .iter()
                        .all(|c| matcher::evaluate(c, &email, &labels))
            })
            .min_by_key(|r| r.priority)
            .cloned();

        if let Some(rule) = matched_rule {
            let item = MatchedEmail {
                email,
                email_from,
                email_subject,
                email_sent_at,
            };

            if let Some((_, bucket)) = grouped
                .iter_mut()
                .find(|(existing_rule, _)| existing_rule.id == rule.id)
            {
                bucket.push(item);
            } else {
                grouped.push((rule, vec![item]));
            }
            continue;
        }

        count += 1;
        publish_progress(
            state,
            on_progress,
            with_current_email(
                op_progress(
                    source,
                    "processing",
                    count,
                    Some(total),
                    Some("No rule matched".into()),
                ),
                Some(&email.id),
                email_from.as_deref(),
                email_subject.as_deref(),
                email_sent_at.as_deref(),
            ),
        )
        .await;
    }

    for (rule, emails) in grouped {
        if emails.is_empty() {
            continue;
        }

        if cancel_requested
            .map(|flag| flag.load(Ordering::Relaxed))
            .unwrap_or(false)
        {
            publish_progress(
                state,
                on_progress,
                op_progress(
                    source,
                    "stopped",
                    count,
                    Some(total),
                    Some("Backfill stopped".into()),
                ),
            )
            .await;

            if update_today {
                state.lock().await.emails_processed_today += count;
            }

            return Ok(PipelineRunResult {
                processed: count,
                discovered: total,
                stopped: true,
            });
        }

        if let Some(first) = emails.first() {
            publish_progress(
                state,
                on_progress,
                with_current_email(
                    op_progress(
                        source,
                        "processing",
                        count,
                        Some(total),
                        Some(format!(
                            "Evaluating {} email{} with rule \"{}\"",
                            emails.len(),
                            if emails.len() == 1 { "" } else { "s" },
                            rule.name
                        )),
                    ),
                    Some(&first.email.id),
                    first.email_from.as_deref(),
                    first.email_subject.as_deref(),
                    first.email_sent_at.as_deref(),
                ),
            )
            .await;
        }

        let memories: Vec<String> = db.with_rule_memory(|repo| {
            repo.list_for_rule(rule.id)
                .map(|rows| rows.into_iter().map(|e| e.text).collect::<Vec<String>>())
                .unwrap_or_default()
        });
        let batch_messages: Vec<Message> = emails.iter().map(|item| item.email.clone()).collect();
        let labels = crate::gmail::cached_labels(db, gmail, account_email)
            .await
            .unwrap_or_default();
        let labels_ref = labels.as_slice();
        let batch = match bulk_resolve_for_rule(
            llm,
            &rule,
            &batch_messages,
            &memories,
            labels_ref,
            cancel_requested,
        )
        .await
        {
            Ok(result) => result,
            Err(e) => {
                for item in &emails {
                    let _ = db.with_inference(|repo| {
                        repo.enqueue(account_email, &item.email.id, rule.id, source.as_str())
                    });
                    insert_history_entry(
                        db,
                        account_email,
                        NewHistoryEntry {
                            email_id: item.email.id.clone(),
                            email_from: item.email_from.clone(),
                            email_subject: item.email_subject.clone(),
                            rule_id: Some(rule.id),
                            rule_name: Some(rule.name.clone()),
                            action: "RESOLVE_RULE".into(),
                            status: "error".into(),
                            llm_model: None,
                            llm_response: None,
                            error: Some(e.to_string()),
                            duration_ms: None,
                            llm_provider: None,
                            policy_id: Some(rule.inference_policy.clone()),
                        },
                    );
                    count += 1;
                    publish_progress(
                        state,
                        on_progress,
                        with_current_email(
                            op_progress(source, "processing", count, Some(total), None),
                            Some(&item.email.id),
                            item.email_from.as_deref(),
                            item.email_subject.as_deref(),
                            item.email_sent_at.as_deref(),
                        ),
                    )
                    .await;
                }
                continue;
            }
        };

        for (item, resolved) in emails.iter().zip(batch.resolved.iter()) {
            let llm_cost = estimate_llm_cost(
                config,
                resolved.prompt_tokens,
                resolved.completion_tokens,
                resolved.total_tokens,
            );

            if resolved.outcome != Outcome::Matched || resolved.actions.is_empty() {
                let (action, status, error) = match resolved.outcome {
                    Outcome::NoMatch => (
                        "NO_MATCH".to_string(),
                        "skipped".to_string(),
                        Some("Rule did not match".to_string()),
                    ),
                    // Batching is an optimization; when its output cannot be
                    // trusted, re-ask about this email alone rather than guess
                    // or drop it. The floor moves past emails only once.
                    Outcome::Unparsed => {
                        let _ = db.with_inference(|repo| {
                            repo.enqueue(account_email, &item.email.id, rule.id, source.as_str())
                        });
                        (
                            "RETRY_QUEUED".to_string(),
                            "error".to_string(),
                            Some(
                                resolved
                                    .diagnostic
                                    .clone()
                                    .unwrap_or_else(|| "Invalid LLM decision".to_string()),
                            ),
                        )
                    }
                    Outcome::Matched => (
                        "SKIP".to_string(),
                        "skipped".to_string(),
                        Some(no_action_reason(
                            &rule,
                            labels_ref,
                            resolved.diagnostic.as_deref(),
                        )),
                    ),
                };
                let history_id = insert_history_entry(
                    db,
                    account_email,
                    NewHistoryEntry {
                        email_id: item.email.id.clone(),
                        email_from: item.email_from.clone(),
                        email_subject: item.email_subject.clone(),
                        rule_id: Some(rule.id),
                        rule_name: Some(rule.name.clone()),
                        action,
                        status,
                        llm_model: resolved.llm_model.clone(),
                        llm_response: Some(resolved.llm_response.clone()),
                        error,
                        duration_ms: resolved.llm_duration_ms.map(|v| v as i64),
                        llm_provider: resolved.llm_provider.clone(),
                        policy_id: Some(rule.inference_policy.clone()),
                    },
                );

                if resolved.llm_model.is_some() {
                    insert_llm_usage_entry(
                        db,
                        account_email,
                        NewLlmUsageEntry {
                            history_id,
                            email_id: item.email.id.clone(),
                            rule_id: rule.id,
                            model: resolved.llm_model.clone(),
                            prompt_tokens: resolved.prompt_tokens.map(|v| v as i64),
                            completion_tokens: resolved.completion_tokens.map(|v| v as i64),
                            total_tokens: resolved.total_tokens.map(|v| v as i64),
                            duration_ms: resolved.llm_duration_ms.map(|v| v as i64),
                            estimated_cost_usd: llm_cost,
                            source: source.as_str().to_string(),
                        },
                    );
                }
                if continues_past(&rule, resolved.outcome) {
                    process_fallthrough(
                        db,
                        account_email,
                        gmail,
                        llm,
                        config,
                        &item.email,
                        item.email_from.as_deref(),
                        item.email_subject.as_deref(),
                        rules,
                        rule.id,
                        labels_ref,
                        source,
                    )
                    .await?;
                }
            } else {
                let error = execute_actions(gmail, &item.email.id, &resolved.actions, labels_ref)
                    .await
                    .err()
                    .map(|err| err.to_string());

                let action_str = resolved
                    .actions
                    .iter()
                    .map(|action| history_action_label(action, labels_ref))
                    .collect::<Vec<_>>()
                    .join(", ");
                let status = if error.is_none() { "success" } else { "error" };

                let history_id = insert_history_entry(
                    db,
                    account_email,
                    NewHistoryEntry {
                        email_id: item.email.id.clone(),
                        email_from: item.email_from.clone(),
                        email_subject: item.email_subject.clone(),
                        rule_id: Some(rule.id),
                        rule_name: Some(rule.name.clone()),
                        action: action_str,
                        status: status.into(),
                        llm_model: resolved.llm_model.clone(),
                        llm_response: Some(resolved.llm_response.clone()),
                        error,
                        duration_ms: resolved.llm_duration_ms.map(|v| v as i64),
                        llm_provider: resolved.llm_provider.clone(),
                        policy_id: Some(rule.inference_policy.clone()),
                    },
                );

                if resolved.llm_model.is_some() {
                    insert_llm_usage_entry(
                        db,
                        account_email,
                        NewLlmUsageEntry {
                            history_id,
                            email_id: item.email.id.clone(),
                            rule_id: rule.id,
                            model: resolved.llm_model.clone(),
                            prompt_tokens: resolved.prompt_tokens.map(|v| v as i64),
                            completion_tokens: resolved.completion_tokens.map(|v| v as i64),
                            total_tokens: resolved.total_tokens.map(|v| v as i64),
                            duration_ms: resolved.llm_duration_ms.map(|v| v as i64),
                            estimated_cost_usd: llm_cost,
                            source: source.as_str().to_string(),
                        },
                    );
                }

                if continues_past(&rule, Outcome::Matched) {
                    process_fallthrough(
                        db,
                        account_email,
                        gmail,
                        llm,
                        config,
                        &item.email,
                        item.email_from.as_deref(),
                        item.email_subject.as_deref(),
                        rules,
                        rule.id,
                        labels_ref,
                        source,
                    )
                    .await?;
                }
            }

            count += 1;
            publish_progress(
                state,
                on_progress,
                with_current_email(
                    op_progress(source, "processing", count, Some(total), None),
                    Some(&item.email.id),
                    item.email_from.as_deref(),
                    item.email_subject.as_deref(),
                    item.email_sent_at.as_deref(),
                ),
            )
            .await;
        }

        if batch.stopped {
            publish_progress(
                state,
                on_progress,
                op_progress(
                    source,
                    "stopped",
                    count,
                    Some(total),
                    Some("Backfill stopped".into()),
                ),
            )
            .await;

            if update_today {
                state.lock().await.emails_processed_today += count;
            }

            return Ok(PipelineRunResult {
                processed: count,
                discovered: total,
                stopped: true,
            });
        }
    }

    let mut s = state.lock().await;
    if update_today {
        s.emails_processed_today += count;
    }

    Ok(PipelineRunResult {
        processed: count,
        discovered: total,
        stopped: false,
    })
}

/// Whether evaluation should keep walking lower-priority rules.
///
/// A decline always continues. A match normally claims the email, so continuing
/// is opt-in per rule. An unreadable reply continues neither way: the queued
/// re-ask resolves the email, and running lower rules first would race it.
fn continues_past(rule: &Rule, outcome: Outcome) -> bool {
    match outcome {
        Outcome::NoMatch => true,
        Outcome::Matched => rule.continue_after_match,
        Outcome::Unparsed => false,
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Fallthrough needs the active pipeline, email metadata, and rule position."
)]
async fn process_fallthrough(
    db: &Arc<Database>,
    account_email: &str,
    gmail: &mut GmailClient,
    llm: &InferenceRouter,
    config: &AppConfig,
    email: &Message,
    email_from: Option<&str>,
    email_subject: Option<&str>,
    rules: &[Rule],
    current_rule_id: i64,
    labels: &[crate::gmail::models::Label],
    source: PipelineSource,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let candidates = crate::rules::engine::rules_after(rules, current_rule_id, email);
    let considered = candidates.len();
    for rule in candidates {
        let memories = db.with_rule_memory(|repo| {
            repo.list_for_rule(rule.id)
                .map(|rows| rows.into_iter().map(|entry| entry.text).collect::<Vec<_>>())
                .unwrap_or_default()
        });
        let resolved = match resolve_rule(llm, rule, email, &memories, labels).await {
            Ok(resolved) => resolved,
            Err(error) => {
                let _ = db.with_inference(|repo| {
                    repo.enqueue(account_email, &email.id, rule.id, source.as_str())
                });
                record_fallthrough(
                    db,
                    account_email,
                    config,
                    source,
                    email,
                    email_from,
                    email_subject,
                    rule,
                    None,
                    "RESOLVE_RULE",
                    "error",
                    Some(error.to_string()),
                );
                return Ok(());
            }
        };
        match resolved.outcome {
            Outcome::NoMatch => {
                record_fallthrough(
                    db,
                    account_email,
                    config,
                    source,
                    email,
                    email_from,
                    email_subject,
                    rule,
                    Some(&resolved),
                    "NO_MATCH",
                    "skipped",
                    Some("Rule did not match".into()),
                );
            }
            Outcome::Unparsed => {
                let _ = db.with_inference(|repo| {
                    repo.enqueue(account_email, &email.id, rule.id, source.as_str())
                });
                record_fallthrough(
                    db,
                    account_email,
                    config,
                    source,
                    email,
                    email_from,
                    email_subject,
                    rule,
                    Some(&resolved),
                    "RETRY_QUEUED",
                    "error",
                    Some(
                        resolved
                            .diagnostic
                            .clone()
                            .unwrap_or_else(|| "Invalid LLM decision".into()),
                    ),
                );
                return Ok(());
            }
            Outcome::Matched => {
                let (action, status, error) = if resolved.actions.is_empty() {
                    (
                        "SKIP".into(),
                        "skipped",
                        Some(no_action_reason(
                            rule,
                            labels,
                            resolved.diagnostic.as_deref(),
                        )),
                    )
                } else {
                    let error = execute_actions(gmail, &email.id, &resolved.actions, labels)
                        .await
                        .err()
                        .map(|error| error.to_string());
                    (
                        resolved
                            .actions
                            .iter()
                            .map(|action| history_action_label(action, labels))
                            .collect::<Vec<_>>()
                            .join(", "),
                        if error.is_none() { "success" } else { "error" },
                        error,
                    )
                };
                record_fallthrough(
                    db,
                    account_email,
                    config,
                    source,
                    email,
                    email_from,
                    email_subject,
                    rule,
                    Some(&resolved),
                    &action,
                    status,
                    error,
                );
                if !continues_past(rule, Outcome::Matched) {
                    return Ok(());
                }
            }
        }
    }

    // Every remaining rule declined or was filtered out. Without this row the
    // history shows only the first rule's NO_MATCH, which is indistinguishable
    // from fallthrough never having run.
    insert_history_entry(
        db,
        account_email,
        NewHistoryEntry {
            email_id: email.id.clone(),
            email_from: email_from.map(ToString::to_string),
            email_subject: email_subject.map(ToString::to_string),
            rule_id: None,
            rule_name: None,
            action: "FALLTHROUGH_EXHAUSTED".into(),
            status: "skipped".into(),
            llm_model: None,
            llm_response: None,
            error: Some(fallthrough_summary(rules, current_rule_id, considered)),
            duration_ms: None,
            llm_provider: None,
            policy_id: None,
        },
    );
    Ok(())
}

fn fallthrough_summary(rules: &[Rule], current_rule_id: i64, considered: usize) -> String {
    let remaining = rules
        .iter()
        .skip_while(|rule| rule.id != current_rule_id)
        .count()
        .saturating_sub(1);
    if remaining == 0 {
        "No lower-priority rule to fall through to".into()
    } else if considered == 0 {
        format!("No rule claimed the email; all {remaining} lower-priority rules were excluded by their conditions")
    } else {
        format!("No rule claimed the email; {considered} of {remaining} lower-priority rules were evaluated and declined")
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "History and usage recording needs the complete evaluated rule outcome."
)]
fn record_fallthrough(
    db: &Arc<Database>,
    account_email: &str,
    config: &AppConfig,
    source: PipelineSource,
    email: &Message,
    email_from: Option<&str>,
    email_subject: Option<&str>,
    rule: &Rule,
    resolved: Option<&Resolved>,
    action: &str,
    status: &str,
    error: Option<String>,
) {
    let history_id = insert_history_entry(
        db,
        account_email,
        NewHistoryEntry {
            email_id: email.id.clone(),
            email_from: email_from.map(ToString::to_string),
            email_subject: email_subject.map(ToString::to_string),
            rule_id: Some(rule.id),
            rule_name: Some(rule.name.clone()),
            action: action.into(),
            status: status.into(),
            llm_model: resolved.and_then(|value| value.llm_model.clone()),
            llm_response: resolved.map(|value| value.llm_response.clone()),
            error,
            duration_ms: resolved
                .and_then(|value| value.llm_duration_ms.map(|duration| duration as i64)),
            llm_provider: resolved.and_then(|value| value.llm_provider.clone()),
            policy_id: Some(rule.inference_policy.clone()),
        },
    );
    let Some(resolved) = resolved else {
        return;
    };
    let Some(model) = resolved.llm_model.clone() else {
        return;
    };
    insert_llm_usage_entry(
        db,
        account_email,
        NewLlmUsageEntry {
            history_id,
            email_id: email.id.clone(),
            rule_id: rule.id,
            model: Some(model),
            prompt_tokens: resolved.prompt_tokens.map(|value| value as i64),
            completion_tokens: resolved.completion_tokens.map(|value| value as i64),
            total_tokens: resolved.total_tokens.map(|value| value as i64),
            duration_ms: resolved.llm_duration_ms.map(|value| value as i64),
            estimated_cost_usd: estimate_llm_cost(
                config,
                resolved.prompt_tokens,
                resolved.completion_tokens,
                resolved.total_tokens,
            ),
            source: source.as_str().into(),
        },
    );
}

/// One-off backfill over `[after, before]` applying only `rule_ids`.
///
/// It re-runs matching history by design and increments today's count, but it
/// leaves `processing.last_run` untouched so live polling keeps its resume point.
#[allow(clippy::too_many_arguments)]
pub async fn run_backfill(
    db: &Arc<Database>,
    account_email: &str,
    state: &Arc<Mutex<ProcessingState>>,
    gmail: &mut GmailClient,
    llm: &InferenceRouter,
    config: &AppConfig,
    after: DateTime<Utc>,
    before: DateTime<Utc>,
    rule_ids: &[i64],
    cancel_requested: &AtomicBool,
    on_progress: &impl Fn(OpProgress),
) -> Result<PipelineRunResult, Box<dyn std::error::Error + Send + Sync>> {
    let query = effective_query(config.polling_query.trim(), after, Some(before));
    let rules = db.with_rules(|repo| repo.get_by_ids(account_email, rule_ids))?;
    run_pipeline(
        db,
        account_email,
        state,
        gmail,
        llm,
        config,
        &query,
        &rules,
        None,
        Some(cancel_requested),
        true,
        PipelineSource::Backfill,
        on_progress,
    )
    .await
}

fn extract_header(email: &Message, name: &str) -> Option<String> {
    email.payload.as_ref().and_then(|p| {
        p.headers
            .iter()
            .find(|h| h.name == name)
            .map(|h| h.value.clone())
    })
}

fn extract_email_sent_at(email: &Message) -> Option<String> {
    if let Some(date_header) = extract_header(email, "Date") {
        if let Ok(dt) = DateTime::parse_from_rfc2822(date_header.trim()) {
            return Some(dt.with_timezone(&Utc).to_rfc3339());
        }
        if let Ok(dt) = DateTime::parse_from_rfc3339(date_header.trim()) {
            return Some(dt.with_timezone(&Utc).to_rfc3339());
        }
    }

    email
        .internal_date
        .trim()
        .parse::<i64>()
        .ok()
        .and_then(|millis| Utc.timestamp_millis_opt(millis).single())
        .map(|dt| dt.to_rfc3339())
}

fn history_action_label(action: &ParsedAction, labels: &[crate::gmail::models::Label]) -> String {
    match action {
        ParsedAction::Label(value) => {
            let resolved = labels
                .iter()
                .find(|l| l.id == *value)
                .or_else(|| labels.iter().find(|l| l.name.eq_ignore_ascii_case(value)));
            let name = resolved.map(|l| l.name.as_str()).unwrap_or("Unknown label");
            format!("Add label: {name}")
        }
        ParsedAction::RemoveLabel(value) => {
            let resolved = labels.iter().find(|label| label.id == *value).or_else(|| {
                labels
                    .iter()
                    .find(|label| label.name.eq_ignore_ascii_case(value))
            });
            let name = resolved
                .map(|label| label.name.as_str())
                .unwrap_or("Unknown label");
            format!("Remove label: {name}")
        }
        _ => format!("{:?}", action),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(continue_after_match: bool) -> Rule {
        Rule {
            id: 1,
            name: "r".into(),
            description: None,
            conditions: vec![],
            prompt: "p".into(),
            choices: vec![],
            choose_from_all_labels: false,
            actions: vec![],
            priority: 0,
            enabled: true,
            parent_id: None,
            inference_policy: "default".into(),
            continue_after_match,
        }
    }

    #[test]
    fn a_decline_always_continues() {
        assert!(continues_past(&rule(false), Outcome::NoMatch));
        assert!(continues_past(&rule(true), Outcome::NoMatch));
    }

    #[test]
    fn a_match_continues_only_when_the_rule_opts_in() {
        assert!(!continues_past(&rule(false), Outcome::Matched));
        assert!(continues_past(&rule(true), Outcome::Matched));
    }

    /// The queued re-ask owns the email from here, so opting in must not race it
    /// down the rule chain.
    #[test]
    fn an_unreadable_reply_never_continues() {
        assert!(!continues_past(&rule(false), Outcome::Unparsed));
        assert!(!continues_past(&rule(true), Outcome::Unparsed));
    }
}
