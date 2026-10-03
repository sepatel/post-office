//! Offline token/quality report for the `EmailView` pre-processing pass.
//!
//! Reads persisted Gmail message snapshots and compares the email block the
//! model sees today against the pre-`EmailView` rendering, using the same
//! char/4 estimate the production budget uses. It never calls a model: for
//! messages whose rendered content changed, it prints the historical decision
//! already recorded in `workflow_steps` so a human can adjudicate whether the
//! change was safe.
//!
//!   cargo run -p post-office-core --example email_eval -- /path/to/app.db
//!
//! The database path may also be supplied via `POST_OFFICE_EVAL_DB`.

use std::path::Path;
use std::sync::LazyLock;

use post_office_core::db::Database;
use post_office_core::gmail::models::{Message, MessagePayload};
use post_office_core::rules::email_view::EmailView;

const PER_PAGE: u32 = 500;

fn estimated_tokens(value: &str) -> usize {
    value.chars().count().div_ceil(4)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Plain,
    Html,
    Snippet,
}

impl Source {
    fn label(self) -> &'static str {
        match self {
            Source::Plain => "plain",
            Source::Html => "html",
            Source::Snippet => "snippet",
        }
    }
}

fn main() {
    let path = std::env::args().nth(1).or_else(|| {
        std::env::var("POST_OFFICE_EVAL_DB")
            .ok()
            .filter(|value| !value.is_empty())
    });
    let Some(path) = path else {
        eprintln!("usage: cargo run -p post-office-core --example email_eval -- <app.db>");
        std::process::exit(2);
    };

    let db = Database::open(Path::new(&path)).expect("open database");
    let accounts = db
        .with_accounts(|repo| repo.list())
        .expect("list accounts")
        .into_iter()
        .map(|account| account.email)
        .collect::<Vec<_>>();
    if accounts.is_empty() {
        eprintln!("no accounts in {path}");
        return;
    }

    let mut report = Report::default();
    for account in &accounts {
        let mut page = 0;
        loop {
            let runs = db
                .with_workflow(|repo| repo.list_runs(account, None, page, PER_PAGE))
                .expect("list runs");
            if runs.is_empty() {
                break;
            }
            let full_page = runs.len() == PER_PAGE as usize;
            for run in runs {
                let Some(json) = run.message_json.as_deref() else {
                    continue;
                };
                let Ok(message) = serde_json::from_str::<Message>(json) else {
                    continue;
                };
                let decision = db
                    .with_workflow(|repo| repo.steps(run.run_id))
                    .ok()
                    .and_then(|steps| {
                        steps
                            .into_iter()
                            .find_map(|step| step.decision.filter(|value| !value.trim().is_empty()))
                    });
                report.observe(&message, decision.as_deref());
            }
            if !full_page {
                break;
            }
            page += 1;
        }
    }

    report.print();
}

#[derive(Default)]
struct Report {
    rows: Vec<Row>,
}

struct Row {
    id: String,
    old_tokens: usize,
    new_tokens: usize,
    old_header_tokens: usize,
    new_header_tokens: usize,
    old_source: Source,
    new_source: Source,
    body_changed: bool,
    decision: Option<String>,
}

impl Report {
    fn observe(&mut self, message: &Message, decision: Option<&str>) {
        let (old_headers, old_body) = old_email_block(message);
        let old_header_tokens = estimated_tokens(&old_headers);

        let view = EmailView::from_message(message);
        let new_header_tokens = estimated_tokens(&view.rendered_headers());

        self.rows.push(Row {
            id: message.id.clone(),
            old_tokens: old_header_tokens + estimated_tokens(&old_body),
            new_tokens: new_header_tokens + estimated_tokens(&view.body),
            old_header_tokens,
            new_header_tokens,
            old_source: old_body_source(message),
            new_source: body_source(message),
            body_changed: old_body != view.body,
            decision: decision.map(str::to_string),
        });
    }

    fn print(&self) {
        let total = self.rows.len();
        if total == 0 {
            println!("no stored message snapshots found");
            return;
        }
        let old = self
            .rows
            .iter()
            .map(|row| row.old_tokens)
            .collect::<Vec<_>>();
        let new = self
            .rows
            .iter()
            .map(|row| row.new_tokens)
            .collect::<Vec<_>>();

        println!("messages: {total}");
        println!(
            "{:<8} {:>8} {:>10} {:>8} {:>8} {:>8}",
            "render", "count", "total", "p50", "p90", "max"
        );
        print_row("old", &old);
        print_row("new", &new);

        let saved: usize = self
            .rows
            .iter()
            .map(|row| row.old_tokens.saturating_sub(row.new_tokens))
            .sum();
        let pct = (saved as f64 / old.iter().sum::<usize>().max(1) as f64) * 100.0;
        println!("email-block tokens saved: {saved} ({pct:.1}%)");
        let header_saved: usize = self
            .rows
            .iter()
            .map(|row| row.old_header_tokens.saturating_sub(row.new_header_tokens))
            .sum();
        let body_saved = self
            .rows
            .iter()
            .map(|row| {
                let old_body = row.old_tokens - row.old_header_tokens;
                let new_body = row.new_tokens - row.new_header_tokens;
                (old_body as isize - new_body as isize).max(0) as usize
            })
            .sum::<usize>();
        println!("  header noise removed: {header_saved}");
        println!("  body tokens saved: {body_saved}");

        self.print_source_changes();
        self.print_changed();
    }

    fn print_source_changes(&self) {
        let snippet_recovered = self
            .rows
            .iter()
            .filter(|row| row.old_source == Source::Snippet && row.new_source != Source::Snippet)
            .count();
        let lost = self
            .rows
            .iter()
            .filter(|row| row.old_source != Source::Snippet && row.new_source == Source::Snippet)
            .count();
        println!("snippet fallbacks recovered: {snippet_recovered} (lost: {lost})");
    }

    fn print_changed(&self) {
        let changed = self
            .rows
            .iter()
            .filter(|row| row.body_changed)
            .collect::<Vec<_>>();
        println!("messages whose body changed: {}", changed.len());
        for row in changed.iter().take(25) {
            println!(
                "  {} old={}tok/{} new={}tok/{} decision={:?}",
                row.id,
                row.old_tokens - row.old_header_tokens,
                row.old_source.label(),
                row.new_tokens - row.new_header_tokens,
                row.new_source.label(),
                row.decision
                    .as_deref()
                    .unwrap_or("-")
                    .lines()
                    .next()
                    .unwrap_or("-"),
            );
        }
    }
}

fn print_row(label: &str, values: &[usize]) {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let percentile = |p: f64| -> usize {
        if sorted.is_empty() {
            0
        } else {
            sorted[((sorted.len() as f64 - 1.0) * p).round() as usize]
        }
    };
    println!(
        "{:<8} {:>8} {:>10} {:>8} {:>8} {:>8}",
        label,
        sorted.len(),
        sorted.iter().sum::<usize>(),
        percentile(0.5),
        percentile(0.9),
        sorted.last().copied().unwrap_or(0),
    );
}

fn body_source(message: &Message) -> Source {
    match message.payload.as_ref() {
        Some(payload) if has_leaf(payload, "text/plain") => Source::Plain,
        Some(payload) if has_leaf(payload, "text/html") => Source::Html,
        _ => Source::Snippet,
    }
}

fn has_leaf(payload: &MessagePayload, mime: &str) -> bool {
    if payload
        .filename
        .as_deref()
        .is_some_and(|name| !name.is_empty())
    {
        return false;
    }
    if payload.mime_type.eq_ignore_ascii_case(mime)
        && payload
            .body
            .as_ref()
            .and_then(|body| body.data.as_deref())
            .is_some_and(|data| !decode(data).trim().is_empty())
    {
        return true;
    }
    payload
        .parts
        .as_deref()
        .is_some_and(|parts| parts.iter().any(|part| has_leaf(part, mime)))
}

/// The pre-`EmailView` rendering, kept frozen as the baseline: every header,
/// and a one-level body scan that falls back to the snippet.
fn old_email_block(message: &Message) -> (String, String) {
    let headers = message
        .payload
        .as_ref()
        .map(|payload| {
            payload
                .headers
                .iter()
                .map(|header| format!("{}: {}", header.name, header.value))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let body = message
        .payload
        .as_ref()
        .and_then(|payload| {
            let find = |mime: &str| {
                payload
                    .parts
                    .as_deref()?
                    .iter()
                    .find(|part| part.mime_type == mime)
                    .and_then(|part| part.body.as_ref())
                    .and_then(|body| body.data.as_deref())
                    .map(decode)
            };
            payload
                .body
                .as_ref()
                .and_then(|body| body.data.as_deref())
                .filter(|_| payload.mime_type == "text/plain")
                .map(decode)
                .or_else(|| find("text/plain"))
                .or_else(|| find("text/html").map(|html| old_strip_html(&html)))
        })
        .unwrap_or_else(|| message.snippet.clone());
    (headers, body)
}

fn old_body_source(message: &Message) -> Source {
    let Some(payload) = message.payload.as_ref() else {
        return Source::Snippet;
    };
    if payload.mime_type == "text/plain"
        && payload
            .body
            .as_ref()
            .and_then(|body| body.data.as_deref())
            .is_some()
    {
        return Source::Plain;
    }
    if payload
        .parts
        .as_deref()
        .is_some_and(|parts| parts.iter().any(|part| part.mime_type == "text/plain"))
    {
        return Source::Plain;
    }
    if payload
        .parts
        .as_deref()
        .is_some_and(|parts| parts.iter().any(|part| part.mime_type == "text/html"))
    {
        return Source::Html;
    }
    Source::Snippet
}

fn decode(data: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE
        .decode(data)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}

fn old_strip_html(html: &str) -> String {
    static TAG_RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"<[^>]+>").unwrap());
    TAG_RE
        .replace_all(html, "")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}
