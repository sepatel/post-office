use std::sync::atomic::{AtomicBool, Ordering};

use futures_util::stream::{self, StreamExt};

use crate::gmail::models::{Label, Message};
use crate::llm::{InferenceRouter, ProcessKind, ProcessRequest};
use crate::rules::engine::{
    choice_catalog, display_action, effective_actions, email_parts, estimated_tokens, memory_block,
    parse_row, ActionDisplay, Choice, Outcome, Resolved, Row, RuleError,
};
use crate::rules::matcher;
use crate::rules::models::Rule;
use crate::rules::prompts::decision_prompt;
use crate::rules::response_parser::ParsedAction;

#[derive(Debug, Clone, serde::Serialize)]
pub struct BulkVerdict {
    pub email_id: String,
    pub matched: bool,
    pub indeterminate: bool,
    pub actions: Vec<ActionDisplay>,
    pub llm_response: String,
    pub reason: String,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DecisionEstimate {
    pub input_tokens: u32,
    pub max_completion_tokens: Option<u32>,
    pub context_reserve_tokens: u32,
    pub output_tokens_per_second: Option<f64>,
    pub max_generation_ms: Option<u64>,
    pub available_request_slots: Option<u32>,
    pub max_concurrent_requests: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct BulkRuleResolution {
    pub resolved: Vec<Resolved>,
    pub stopped: bool,
}

struct ChunkResolution {
    resolved: Vec<Resolved>,
    stopped: bool,
}

struct DecisionContext {
    menu: Vec<Choice>,
    system_prompt: String,
    budget: usize,
    max_batch_size: usize,
}

pub async fn bulk_resolve_for_rule(
    llm: &InferenceRouter,
    rule: &Rule,
    emails: &[Message],
    memories: &[String],
    labels: &[Label],
    cancel_requested: Option<&AtomicBool>,
) -> Result<BulkRuleResolution, RuleError> {
    if emails.is_empty() {
        return Ok(BulkRuleResolution {
            resolved: vec![],
            stopped: false,
        });
    }

    let Some(context) = decision_context(llm, rule, labels) else {
        let actions: Vec<ParsedAction> = rule.actions.iter().map(ParsedAction::from).collect();
        return Ok(BulkRuleResolution {
            resolved: emails
                .iter()
                .map(|_| Resolved::local(Outcome::Matched, actions.clone()))
                .collect(),
            stopped: false,
        });
    };
    let chunks = batch_chunks(
        rule,
        emails,
        memories,
        context.budget,
        context.max_batch_size,
    )?;
    let mut resolved = Vec::with_capacity(emails.len());
    let chunks = stream::iter(chunks)
        .map(|chunk| {
            resolve_chunk(
                llm,
                rule,
                chunk,
                memories,
                &context.menu,
                &context.system_prompt,
                context.budget,
                cancel_requested,
            )
        })
        .buffered(llm.max_concurrent_requests(&rule.inference_policy))
        .collect::<Vec<_>>()
        .await;
    let mut stopped = false;
    for chunk in chunks {
        let chunk = chunk?;
        stopped |= chunk.stopped;
        resolved.extend(chunk.resolved);
    }

    Ok(BulkRuleResolution { resolved, stopped })
}

pub fn decision_estimate(
    llm: &InferenceRouter,
    rule: &Rule,
    email: &Message,
    memories: &[String],
    labels: &[Label],
) -> Result<Option<DecisionEstimate>, RuleError> {
    let Some(context) = decision_context(llm, rule, labels) else {
        return Ok(None);
    };
    let user_prompt = build_batch_prompt(rule, &[email], memories, context.budget)?;
    let input_tokens = estimated_tokens(&context.system_prompt)
        .saturating_add(estimated_tokens(&user_prompt))
        .try_into()
        .unwrap_or(u32::MAX);
    let max_completion_tokens = rule.decision_max_tokens;
    let output_tokens_per_second = llm.output_tokens_per_second(&rule.inference_policy);
    let request_slots = llm.available_request_slots(&rule.inference_policy);
    let max_generation_ms =
        max_completion_tokens
            .zip(output_tokens_per_second)
            .map(|(limit, rate)| {
                ((limit as f64 / rate) * 1_000.0)
                    .ceil()
                    .min(u64::MAX as f64) as u64
            });
    Ok(Some(DecisionEstimate {
        input_tokens,
        max_completion_tokens,
        context_reserve_tokens: llm.decision_context_reserve_tokens(max_completion_tokens),
        output_tokens_per_second,
        max_generation_ms,
        available_request_slots: request_slots.map(|(available, _)| available),
        max_concurrent_requests: request_slots.map(|(_, maximum)| maximum),
    }))
}

fn decision_context(
    llm: &InferenceRouter,
    rule: &Rule,
    labels: &[Label],
) -> Option<DecisionContext> {
    let has_instruction = !rule.prompt.trim().is_empty();
    let menu = choice_catalog(rule, labels);
    // A menu explicitly asks for inference, even without an instruction.
    if !has_instruction && menu.is_empty() {
        return None;
    }
    let system_prompt = decision_prompt(&menu, has_instruction);
    let max_batch_size =
        llm.max_emails_per_request(&rule.inference_policy, rule.decision_reasoning_effort);
    let reserved_completion_tokens = llm.decision_context_reserve_tokens(rule.decision_max_tokens);
    let budget = llm
        .input_token_budget(&rule.inference_policy, reserved_completion_tokens)
        .saturating_sub(estimated_tokens(&system_prompt));
    Some(DecisionContext {
        menu,
        system_prompt,
        budget,
        max_batch_size,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "A chunk needs the full rule decision context and cancellation state."
)]
async fn resolve_chunk(
    llm: &InferenceRouter,
    rule: &Rule,
    chunk: Vec<Message>,
    memories: &[String],
    menu: &[Choice],
    system_prompt: &str,
    budget: usize,
    cancel_requested: Option<&AtomicBool>,
) -> Result<ChunkResolution, RuleError> {
    if cancel_requested
        .map(|flag| flag.load(Ordering::Relaxed))
        .unwrap_or(false)
    {
        return Ok(ChunkResolution {
            resolved: vec![],
            stopped: true,
        });
    }
    let message_refs = chunk.iter().collect::<Vec<_>>();
    let user_prompt = build_batch_prompt(rule, &message_refs, memories, budget)?;
    let response = llm
        .process(
            &rule.inference_policy,
            ProcessRequest {
                system_prompt: Some(system_prompt.to_string()),
                user_prompt,
                model: None,
                temperature: None,
                max_tokens: rule.decision_max_tokens,
                kind: ProcessKind::Decision,
                reasoning_effort: rule
                    .decision_reasoning_effort
                    .request_value()
                    .map(str::to_string),
                litellm_reasoning_passthrough: false,
            },
        )
        .await;
    let resolved = match response {
        Ok(response) => {
            let content = match strip_thinking_prefix(&response.content) {
                Ok(content) if !content.is_empty() => content,
                Ok(_) => {
                    let diagnostic = missing_decision_diagnostic(&response);
                    return Ok(ChunkResolution {
                        resolved: (0..chunk.len())
                            .map(|_| {
                                unparsed(
                                    response.content.clone(),
                                    &diagnostic,
                                    &response,
                                    chunk.len(),
                                )
                            })
                            .collect(),
                        stopped: false,
                    });
                }
                Err(diagnostic) => {
                    return Ok(ChunkResolution {
                        resolved: (0..chunk.len())
                            .map(|_| {
                                unparsed(
                                    response.content.clone(),
                                    diagnostic,
                                    &response,
                                    chunk.len(),
                                )
                            })
                            .collect(),
                        stopped: false,
                    });
                }
            };
            match parse_rows(content, chunk.len(), menu) {
                Ok(rows) => rows
                    .into_iter()
                    .map(|row| from_row(rule, row, &response, chunk.len()))
                    .collect(),
                Err(diagnostic) => (0..chunk.len())
                    .map(|_| unparsed(content.to_string(), &diagnostic, &response, chunk.len()))
                    .collect(),
            }
        }
        Err(error) => (0..chunk.len())
            .map(|_| unavailable(&error.to_string()))
            .collect(),
    };
    Ok(ChunkResolution {
        resolved,
        stopped: false,
    })
}

pub async fn bulk_evaluate(
    llm: &InferenceRouter,
    rule: &Rule,
    emails: &[Message],
    memories: &[String],
    labels: &[Label],
) -> Result<Vec<BulkVerdict>, RuleError> {
    let mut verdicts: Vec<BulkVerdict> = emails
        .iter()
        .map(|email| BulkVerdict {
            email_id: email.id.clone(),
            matched: false,
            indeterminate: false,
            actions: vec![],
            llm_response: String::new(),
            reason: String::new(),
            diagnostic: None,
        })
        .collect();
    let matched: Vec<usize> = emails
        .iter()
        .enumerate()
        .filter(|(_, email)| {
            rule.conditions
                .iter()
                .all(|condition| matcher::evaluate(condition, email, &email.label_ids))
        })
        .map(|(index, _)| index)
        .collect();
    if matched.is_empty() {
        return Ok(verdicts);
    }
    let candidates: Vec<Message> = matched.iter().map(|index| emails[*index].clone()).collect();
    let resolved = bulk_resolve_for_rule(llm, rule, &candidates, memories, labels, None).await?;
    for (index, result) in matched.into_iter().zip(resolved.resolved) {
        verdicts[index].matched = result.outcome == Outcome::Matched;
        verdicts[index].indeterminate = result.outcome == Outcome::Unparsed;
        verdicts[index].actions = result.actions.iter().map(display_action).collect();
        verdicts[index].llm_response = result.llm_response;
        verdicts[index].reason = result.reason;
        verdicts[index].diagnostic = result.diagnostic;
    }
    Ok(verdicts)
}

fn from_row(
    rule: &Rule,
    row: Row,
    response: &crate::llm::ProcessResponse,
    email_count: usize,
) -> Resolved {
    let (outcome, actions) = if row.matched {
        (Outcome::Matched, effective_actions(rule, &row.chosen))
    } else {
        (Outcome::NoMatch, vec![])
    };
    Resolved {
        outcome,
        actions,
        llm_response: row.text,
        reason: row.reason,
        diagnostic: row.note,
        ..metrics(response, email_count)
    }
}

fn unparsed(
    llm_response: String,
    diagnostic: &str,
    response: &crate::llm::ProcessResponse,
    email_count: usize,
) -> Resolved {
    Resolved {
        outcome: Outcome::Unparsed,
        llm_response,
        diagnostic: Some(diagnostic.to_string()),
        ..metrics(response, email_count)
    }
}

fn unavailable(diagnostic: &str) -> Resolved {
    Resolved {
        outcome: Outcome::Unparsed,
        actions: vec![],
        llm_response: String::new(),
        reason: String::new(),
        llm_model: None,
        llm_provider: None,
        prompt_tokens: None,
        completion_tokens: None,
        total_tokens: None,
        llm_duration_ms: None,
        llm_request_key: None,
        llm_request_email_count: None,
        llm_unavailable: true,
        diagnostic: Some(diagnostic.to_string()),
    }
}

fn metrics(response: &crate::llm::ProcessResponse, email_count: usize) -> Resolved {
    Resolved {
        outcome: Outcome::Unparsed,
        actions: vec![],
        llm_response: String::new(),
        reason: String::new(),
        llm_model: Some(response.model.clone()),
        llm_provider: response.provider_id.clone(),
        prompt_tokens: response.prompt_tokens,
        completion_tokens: response.completion_tokens,
        total_tokens: response.tokens_used,
        llm_duration_ms: Some(response.duration_ms),
        llm_request_key: Some(response.request_key.clone()),
        llm_request_email_count: Some(email_count as u32),
        llm_unavailable: false,
        diagnostic: None,
    }
}

/// Reads one decision per email, positionally.
///
/// Correlation is by order, never by the number the model wrote: a model that
/// miscounts would otherwise apply one email's decision to another. A reply
/// that does not carry exactly one decision per email is refused whole, which
/// costs a re-ask but cannot mislabel anything.
fn parse_rows(response: &str, count: usize, menu: &[Choice]) -> Result<Vec<Row>, String> {
    // One email's answer is the whole reply, however the model laid it out.
    if count == 1 {
        return parse_row(response, menu)
            .map(|row| vec![row])
            .map_err(|error| error.to_string());
    }
    let rows: Vec<Row> = response
        .lines()
        .filter_map(|line| parse_row(line, menu).ok())
        .collect();
    if rows.len() == count {
        return Ok(rows);
    }
    Err(format!(
        "Expected one decision line for each of the {count} emails, found {}",
        rows.len()
    ))
}

fn strip_thinking_prefix(content: &str) -> Result<&str, &'static str> {
    let content = content.trim();
    for tag in ["think", "thinking"] {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        if let Some(after_open) = content.strip_prefix(&open) {
            if let Some(after_close) = after_open.strip_prefix(&close) {
                return Ok(after_close.trim());
            }
            if let Some(end) = after_open.find(&close) {
                return Ok(after_open[end + close.len()..].trim());
            }
            return Err("Model returned unfinished thinking without a final decision");
        }
    }
    Ok(content)
}

fn missing_decision_diagnostic(response: &crate::llm::ProcessResponse) -> String {
    let reason = if response.has_reasoning {
        "Model returned reasoning without a final decision"
    } else {
        "Model returned no final decision"
    };
    if response.finish_reason.as_deref() == Some("length") {
        format!("{reason}; output limit was reached")
    } else {
        reason.into()
    }
}

fn batch_chunks(
    rule: &Rule,
    emails: &[Message],
    memories: &[String],
    budget: usize,
    max_batch_size: usize,
) -> Result<Vec<Vec<Message>>, RuleError> {
    let base = batch_prefix(rule, memories);
    if estimated_tokens(&base) >= budget {
        return Err(RuleError::PromptTooLarge);
    }
    let mut chunks = Vec::new();
    let mut current = Vec::new();
    let mut current_tokens = estimated_tokens(&base);
    for email in emails {
        let email_tokens = estimated_tokens(&render_email(email));
        if !current.is_empty()
            && (current.len() == max_batch_size || current_tokens + email_tokens > budget)
        {
            chunks.push(current);
            current = Vec::new();
            current_tokens = estimated_tokens(&base);
        }
        current_tokens += email_tokens;
        current.push(email.clone());
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    Ok(chunks)
}

fn build_batch_prompt(
    rule: &Rule,
    emails: &[&Message],
    memories: &[String],
    budget: usize,
) -> Result<String, RuleError> {
    let prefix = batch_prefix(rule, memories);
    let suffix = format!("\nAnswer with {} decision lines.\n", emails.len());
    let emails_text = emails
        .iter()
        .enumerate()
        .map(|(index, email)| format!("--- EMAIL {} ---\n{}", index + 1, render_email(email)))
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = format!("{prefix}{emails_text}{suffix}");
    if estimated_tokens(&prompt) <= budget {
        return Ok(prompt);
    }
    if emails.len() != 1 {
        return Err(RuleError::PromptTooLarge);
    }
    let (headers, body) = email_parts(emails[0]);
    let email_prefix = format!("{prefix}--- EMAIL 1 ---\n{headers}\n\n");
    crate::rules::engine::fit_email_body(&email_prefix, &body, &suffix, budget)
}

/// The menu lives in the system prompt, which is built per rule, so the payload
/// carries only the instruction, the memories, and the emails.
fn batch_prefix(rule: &Rule, memories: &[String]) -> String {
    let instruction = if rule.prompt.trim().is_empty() {
        String::new()
    } else {
        format!("RULE INSTRUCTION:\n{}", rule.prompt.trim())
    };
    format!("{instruction}{}\n\nEMAILS:\n", memory_block(memories))
}

fn render_email(email: &Message) -> String {
    let (headers, body) = email_parts(email);
    format!("{headers}\n\n{body}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu() -> Vec<Choice> {
        vec![
            Choice::label("Label_1", "Invoices"),
            Choice::label("Label_2", "Education/Saarth"),
        ]
    }

    fn labels_of(row: &Row) -> Vec<String> {
        row.chosen
            .iter()
            .map(|action| match action {
                ParsedAction::Label(id) => id.clone(),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn reads_one_decision_per_email() {
        let rows = parse_rows(
            "1: Invoices | Categorize it.\n2: NO_MATCH | Ignore it.",
            2,
            &menu(),
        )
        .expect("reply should parse");

        assert_eq!(labels_of(&rows[0]), vec!["Label_1".to_string()]);
        assert!(!rows[1].matched);
    }

    /// The reported failure: the model copies the literal `N:` placeholder, so
    /// no line carries a usable index. Order is enough to correlate them.
    #[test]
    fn a_copied_placeholder_marker_does_not_lose_the_batch() {
        let rows = parse_rows(
            "N: NO_MATCH | tech news update, not music\nN: NO_MATCH | banking notification, not music",
            2,
            &[],
        )
        .expect("reply should parse");

        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| !row.matched));
        assert_eq!(rows[1].reason, "banking notification, not music");
    }

    /// The same failure with one email, which used to report a missing decision
    /// keyword because the marker was still attached to it.
    #[test]
    fn a_copied_placeholder_marker_does_not_lose_a_single_email() {
        let rows = parse_rows("N: NO_MATCH | GitHub discussion, not music", 1, &[])
            .expect("reply should parse");

        assert_eq!(rows.len(), 1);
        assert!(!rows[0].matched);
    }

    /// The reported rule verbatim: a menu of labels plus an action, answered
    /// with the bare choice name the prompt itself taught the model to use.
    #[test]
    fn a_menu_of_labels_and_actions_is_answered_by_name() {
        let menu = vec![
            Choice::label("Label_1", "Needs Action"),
            Choice::label("Label_2", "Cold Outreach"),
            Choice::label("Label_3", "Financial"),
            Choice::label("Label_4", "Education"),
            Choice {
                name: "TRASH".into(),
                action: ParsedAction::Trash,
            },
        ];

        let rows = parse_rows(
            "1: \"Financial\" | Invoice for an existing subscription.\n\
             2: TRASH | GitHub notification unrelated to tekniq.\n\
             3: NO_MATCH | Personal note with no action requested.",
            3,
            &menu,
        )
        .expect("reply should parse");

        assert_eq!(labels_of(&rows[0]), vec!["Label_3".to_string()]);
        assert_eq!(rows[1].chosen, vec![ParsedAction::Trash]);
        assert!(!rows[2].matched);
    }

    #[test]
    fn prose_around_the_decisions_is_ignored() {
        let rows = parse_rows(
            "Here are my decisions:\n1: Invoices | a bill\n2: NO_MATCH | spam\nLet me know if you need more.",
            2,
            &menu(),
        )
        .expect("reply should parse");

        assert_eq!(rows.len(), 2);
    }

    /// A miscount cannot be resolved without trusting the model's own numbering,
    /// which is exactly what would misassign a decision to the wrong email.
    #[test]
    fn a_miscounted_reply_is_refused_whole() {
        let error = parse_rows("1: Invoices | a bill", 3, &menu()).expect_err("should refuse");

        assert!(error.contains("3 emails"));
        assert!(error.contains("found 1"));
    }

    #[test]
    fn a_single_email_answer_may_span_lines() {
        let rows = parse_rows("MATCH\nLABELS: Invoices\nIt is a bill.", 1, &menu())
            .expect("reply should parse");

        assert_eq!(labels_of(&rows[0]), vec!["Label_1".to_string()]);
        assert_eq!(rows[0].reason, "It is a bill.");
    }

    #[test]
    fn a_single_email_answer_that_is_not_a_decision_is_refused() {
        let error = parse_rows("I could not decide", 1, &menu()).expect_err("should refuse");

        assert!(error.contains("choice or NO_MATCH"));
    }

    #[test]
    fn removes_thinking_before_reading_decisions() {
        assert_eq!(
            strip_thinking_prefix("<think>considered the message</think>\n1: NO_MATCH | unrelated"),
            Ok("1: NO_MATCH | unrelated")
        );
        assert_eq!(
            strip_thinking_prefix("1: MATCH | direct"),
            Ok("1: MATCH | direct")
        );
    }

    #[test]
    fn incomplete_thinking_is_not_read_as_a_decision() {
        assert_eq!(
            strip_thinking_prefix("<think>considering the email"),
            Err("Model returned unfinished thinking without a final decision")
        );
    }
}
