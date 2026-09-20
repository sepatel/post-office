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
pub struct EvaluationVerdict {
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

struct DecisionContext {
    menu: Vec<Choice>,
    system_prompt: String,
    budget: usize,
}

pub async fn resolve_decision(
    llm: &InferenceRouter,
    rule: &Rule,
    email: &Message,
    memories: &[String],
    labels: &[Label],
) -> Result<Resolved, RuleError> {
    let Some(context) = decision_context(llm, rule, labels) else {
        let actions: Vec<ParsedAction> = rule.actions.iter().map(ParsedAction::from).collect();
        return Ok(Resolved::local(Outcome::Matched, actions));
    };
    let user_prompt = build_decision_prompt(rule, email, memories, context.budget)?;
    let max_tokens = llm.decision_max_tokens(rule.decision_max_tokens);
    let response = llm
        .process(
            &rule.inference_policy,
            ProcessRequest {
                system_prompt: Some(context.system_prompt),
                user_prompt,
                model: None,
                temperature: None,
                max_tokens: Some(max_tokens),
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
                    return Ok(unparsed(response.content.clone(), &diagnostic, &response));
                }
                Err(diagnostic) => {
                    return Ok(unparsed(response.content.clone(), diagnostic, &response));
                }
            };
            match parse_row(content, &context.menu) {
                Ok(row) => from_row(rule, row, &response),
                Err(error) => unparsed(content.to_string(), &error.to_string(), &response),
            }
        }
        Err(error) => unavailable(&error.to_string()),
    };
    Ok(resolved)
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
    let user_prompt = build_decision_prompt(rule, email, memories, context.budget)?;
    let input_tokens = estimated_tokens(&context.system_prompt)
        .saturating_add(estimated_tokens(&user_prompt))
        .try_into()
        .unwrap_or(u32::MAX);
    let max_completion_tokens = llm.decision_max_tokens(rule.decision_max_tokens);
    let output_tokens_per_second = llm.output_tokens_per_second(&rule.inference_policy);
    let request_slots = llm.available_request_slots(&rule.inference_policy);
    let max_generation_ms = output_tokens_per_second.map(|rate| {
        ((max_completion_tokens as f64 / rate) * 1_000.0)
            .ceil()
            .min(u64::MAX as f64) as u64
    });
    Ok(Some(DecisionEstimate {
        input_tokens,
        max_completion_tokens: Some(max_completion_tokens),
        context_reserve_tokens: max_completion_tokens,
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
    let reserved_completion_tokens = llm.decision_context_reserve_tokens(rule.decision_max_tokens);
    let budget = llm
        .input_token_budget(&rule.inference_policy, reserved_completion_tokens)
        .saturating_sub(estimated_tokens(&system_prompt));
    Some(DecisionContext {
        menu,
        system_prompt,
        budget,
    })
}

pub async fn evaluate_messages(
    llm: &InferenceRouter,
    rule: &Rule,
    emails: &[Message],
    memories: &[String],
    labels: &[Label],
) -> Result<Vec<EvaluationVerdict>, RuleError> {
    let mut verdicts: Vec<EvaluationVerdict> = emails
        .iter()
        .map(|email| EvaluationVerdict {
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
    for index in matched {
        let result = resolve_decision(llm, rule, &emails[index], memories, labels).await?;
        verdicts[index].matched = result.outcome == Outcome::Matched;
        verdicts[index].indeterminate = result.outcome == Outcome::Unparsed;
        verdicts[index].actions = result.actions.iter().map(display_action).collect();
        verdicts[index].llm_response = result.llm_response;
        verdicts[index].reason = result.reason;
        verdicts[index].diagnostic = result.diagnostic;
    }
    Ok(verdicts)
}

fn from_row(rule: &Rule, row: Row, response: &crate::llm::ProcessResponse) -> Resolved {
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
        ..metrics(response)
    }
}

fn unparsed(
    llm_response: String,
    diagnostic: &str,
    response: &crate::llm::ProcessResponse,
) -> Resolved {
    Resolved {
        outcome: Outcome::Unparsed,
        llm_response,
        diagnostic: Some(diagnostic.to_string()),
        ..metrics(response)
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
        llm_unavailable: true,
        diagnostic: Some(diagnostic.to_string()),
    }
}

fn metrics(response: &crate::llm::ProcessResponse) -> Resolved {
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
        llm_unavailable: false,
        diagnostic: None,
    }
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

fn build_decision_prompt(
    rule: &Rule,
    email: &Message,
    memories: &[String],
    budget: usize,
) -> Result<String, RuleError> {
    let prefix = decision_prefix(rule, memories);
    let suffix = "\nAnswer with exactly one decision line.\n";
    let prompt = format!("{prefix}{}{}", render_email(email), suffix);
    if estimated_tokens(&prompt) <= budget {
        return Ok(prompt);
    }
    let (headers, body) = email_parts(email);
    let email_prefix = format!("{prefix}{headers}\n\n");
    crate::rules::engine::fit_email_body(&email_prefix, &body, suffix, budget)
}

/// The menu lives in the system prompt, which is built per rule, so the payload
/// carries only the instruction, the memories, and the email.
fn decision_prefix(rule: &Rule, memories: &[String]) -> String {
    let instruction = if rule.prompt.trim().is_empty() {
        String::new()
    } else {
        format!("RULE INSTRUCTION:\n{}", rule.prompt.trim())
    };
    format!("{instruction}{}\n\nEMAIL:\n", memory_block(memories))
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
    fn reads_one_decision() {
        let row = parse_row("1: Invoices | Categorize it.", &menu()).expect("reply should parse");

        assert_eq!(labels_of(&row), vec!["Label_1".to_string()]);
    }

    #[test]
    fn a_copied_placeholder_marker_still_parses() {
        let row = parse_row("N: NO_MATCH | GitHub discussion, not music", &[])
            .expect("reply should parse");

        assert!(!row.matched);
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

        let row = parse_row(
            "1: \"Financial\" | Invoice for an existing subscription.",
            &menu,
        )
        .expect("reply should parse");

        assert_eq!(labels_of(&row), vec!["Label_3".to_string()]);
    }

    #[test]
    fn a_single_email_answer_may_span_lines() {
        let row = parse_row("MATCH\nLABELS: Invoices\nIt is a bill.", &menu())
            .expect("reply should parse");

        assert_eq!(labels_of(&row), vec!["Label_1".to_string()]);
        assert_eq!(row.reason, "It is a bill.");
    }

    #[test]
    fn a_single_email_answer_that_is_not_a_decision_is_refused() {
        let error = parse_row("I could not decide", &menu()).expect_err("should refuse");

        assert!(error.to_string().contains("choice or NO_MATCH"));
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
