use std::collections::HashMap;
use std::str::FromStr;
use std::sync::LazyLock;

use regex::Regex;

use super::matcher;
use super::models::{Action, Rule};
use super::response_parser::ParsedAction;
use crate::db::rules::RuleRepository;
use crate::gmail::models::{Label, Message, MessagePayload};
use crate::llm::InferenceRouter;
use crate::rules::evaluation::{decision_estimate, DecisionEstimate};

const TRUNCATION_MARKER: &str = "\n\n[Body truncated; middle omitted]\n\n";

pub struct RuleEngine<'a> {
    rule_repo: &'a RuleRepository<'a>,
}

impl<'a> RuleEngine<'a> {
    pub fn new(rule_repo: &'a RuleRepository<'a>) -> Self {
        Self { rule_repo }
    }

    pub fn find_matching_rule(
        &self,
        account_email: &str,
        email: &Message,
        current_labels: &[String],
    ) -> Option<Rule> {
        let rules = self.rule_repo.get_enabled_rules(account_email).ok()?;
        rules
            .into_iter()
            .filter(|rule| {
                rule.conditions
                    .iter()
                    .all(|condition| matcher::evaluate(condition, email, current_labels))
            })
            .min_by_key(|rule| rule.priority)
    }
}

/// Rules that may still claim `email` after `current_rule_id` declined it, in
/// priority order, with the deterministic pre-filter already applied.
///
/// Shared so the live pipeline and the rule tester agree on what "the next rule"
/// means; `rules` must be ordered by ascending priority.
pub fn rules_after<'a>(rules: &'a [Rule], current_rule_id: i64, email: &Message) -> Vec<&'a Rule> {
    rules
        .iter()
        .skip_while(|rule| rule.id != current_rule_id)
        .skip(1)
        .filter(|rule| {
            rule.enabled
                && rule
                    .conditions
                    .iter()
                    .all(|condition| matcher::evaluate(condition, email, &email.label_ids))
        })
        .collect()
}

/// Resolves one email against one rule.
///
/// Rule decisions always use a single-email prompt and response contract.
pub async fn resolve_rule(
    llm: &InferenceRouter,
    rule: &Rule,
    email: &Message,
    memories: &[String],
    labels: &[Label],
) -> Result<Resolved, RuleError> {
    let conditions_match = rule
        .conditions
        .iter()
        .all(|condition| matcher::evaluate(condition, email, &email.label_ids));
    if !conditions_match {
        return Ok(Resolved::declined());
    }
    let resolved =
        crate::rules::evaluation::resolve_decision(llm, rule, email, memories, labels).await?;
    if resolved.llm_unavailable {
        return Err(RuleError::Llm(crate::llm::LlmError::Routing(
            resolved
                .diagnostic
                .unwrap_or_else(|| "No provider completed the decision".into()),
        )));
    }
    Ok(resolved)
}

pub async fn test_rule(
    llm: &InferenceRouter,
    rule: &Rule,
    email: &Message,
    memories: &[String],
    labels: &[Label],
) -> Result<TestResult, RuleError> {
    let resolved = resolve_rule(llm, rule, email, memories, labels).await?;
    Ok(TestResult {
        matched: resolved.outcome == Outcome::Matched,
        indeterminate: resolved.outcome == Outcome::Unparsed,
        actions: resolved.actions.iter().map(display_action).collect(),
        llm_response: resolved.llm_response,
        reasoning: resolved.reason,
        diagnostic: resolved.diagnostic,
        fallthrough: vec![],
    })
}

#[derive(Debug, thiserror::Error)]
pub enum RuleError {
    #[error(transparent)]
    Database(#[from] rusqlite::Error),

    #[error(transparent)]
    Llm(#[from] crate::llm::LlmError),

    #[error("Rule prompt and context exceed the model input budget")]
    PromptTooLarge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    NoMatch,
    Matched,
    /// The reply could not be read as a decision for this email, so nothing is
    /// known about it. Acting on it would be a guess.
    Unparsed,
}

#[derive(Debug, Clone)]
pub struct Resolved {
    pub outcome: Outcome,
    pub actions: Vec<ParsedAction>,
    pub llm_response: String,
    pub reason: String,
    pub llm_model: Option<String>,
    pub llm_provider: Option<String>,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
    pub llm_duration_ms: Option<u64>,
    pub llm_request_key: Option<String>,
    pub llm_unavailable: bool,
    /// A non-fatal deviation from the response contract, kept so it surfaces in
    /// the UI without blocking the decision.
    pub diagnostic: Option<String>,
}

impl Resolved {
    pub(crate) fn declined() -> Self {
        Self::local(Outcome::NoMatch, vec![])
    }

    /// A decision reached without asking the model.
    pub(crate) fn local(outcome: Outcome, actions: Vec<ParsedAction>) -> Self {
        Self {
            outcome,
            actions,
            llm_response: String::new(),
            reason: String::new(),
            llm_model: None,
            llm_provider: None,
            prompt_tokens: None,
            completion_tokens: None,
            total_tokens: None,
            llm_duration_ms: None,
            llm_request_key: None,
            llm_unavailable: false,
            diagnostic: None,
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct TestResult {
    pub matched: bool,
    pub indeterminate: bool,
    pub actions: Vec<ActionDisplay>,
    pub llm_response: String,
    pub reasoning: String,
    pub diagnostic: Option<String>,
    /// Lower-priority rules evaluated after this one declined, in the order the
    /// pipeline would try them. Ends at the rule that claims the email.
    #[serde(default)]
    pub fallthrough: Vec<FallthroughStep>,
}

#[derive(Debug, serde::Serialize)]
pub struct PipelineDryRun {
    pub steps: Vec<PipelineDryRunStep>,
    pub status: String,
    pub summary: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PipelineDryRunStep {
    pub rule_id: i64,
    pub rule_name: String,
    pub priority: i32,
    pub status: String,
    pub actions: Vec<ActionDisplay>,
    pub reasoning: String,
    pub diagnostic: Option<String>,
    pub llm_response: String,
    pub llm_model: Option<String>,
    pub llm_provider: Option<String>,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
    pub duration_ms: Option<u64>,
    pub continued: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PipelineDryRunProgress {
    pub phase: String,
    pub rule_id: i64,
    pub rule_name: String,
    pub priority: i32,
    pub decision_estimate: Option<DecisionEstimate>,
    pub step: Option<PipelineDryRunStep>,
}

pub async fn dry_run_pipeline(
    llm: &InferenceRouter,
    rules: &[Rule],
    email: &Message,
    memories_by_rule: &HashMap<i64, Vec<String>>,
    labels: &[Label],
    on_progress: &impl Fn(PipelineDryRunProgress),
) -> PipelineDryRun {
    let mut steps = Vec::new();
    for rule in rules.iter().filter(|rule| rule.enabled) {
        on_progress(PipelineDryRunProgress {
            phase: "evaluating".into(),
            rule_id: rule.id,
            rule_name: rule.name.clone(),
            priority: rule.priority,
            decision_estimate: None,
            step: None,
        });
        if !rule
            .conditions
            .iter()
            .all(|condition| matcher::evaluate(condition, email, &email.label_ids))
        {
            steps.push(PipelineDryRunStep {
                rule_id: rule.id,
                rule_name: rule.name.clone(),
                priority: rule.priority,
                status: "condition_skipped".into(),
                actions: vec![],
                reasoning: String::new(),
                diagnostic: None,
                llm_response: String::new(),
                llm_model: None,
                llm_provider: None,
                prompt_tokens: None,
                completion_tokens: None,
                total_tokens: None,
                duration_ms: None,
                continued: true,
            });
            on_progress(PipelineDryRunProgress {
                phase: "completed".into(),
                rule_id: rule.id,
                rule_name: rule.name.clone(),
                priority: rule.priority,
                decision_estimate: None,
                step: steps.last().cloned(),
            });
            continue;
        }

        let memories = memories_by_rule
            .get(&rule.id)
            .map(Vec::as_slice)
            .unwrap_or_default();
        on_progress(PipelineDryRunProgress {
            phase: "evaluating".into(),
            rule_id: rule.id,
            rule_name: rule.name.clone(),
            priority: rule.priority,
            decision_estimate: decision_estimate(llm, rule, email, memories, labels)
                .ok()
                .flatten(),
            step: None,
        });
        let resolved = match resolve_rule(llm, rule, email, memories, labels).await {
            Ok(resolved) => resolved,
            Err(error) => {
                steps.push(PipelineDryRunStep {
                    rule_id: rule.id,
                    rule_name: rule.name.clone(),
                    priority: rule.priority,
                    status: "invalid_decision".into(),
                    actions: vec![],
                    reasoning: String::new(),
                    diagnostic: Some(error.to_string()),
                    llm_response: String::new(),
                    llm_model: None,
                    llm_provider: None,
                    prompt_tokens: None,
                    completion_tokens: None,
                    total_tokens: None,
                    duration_ms: None,
                    continued: false,
                });
                on_progress(PipelineDryRunProgress {
                    phase: "completed".into(),
                    rule_id: rule.id,
                    rule_name: rule.name.clone(),
                    priority: rule.priority,
                    decision_estimate: None,
                    step: steps.last().cloned(),
                });
                return PipelineDryRun {
                    steps,
                    status: "would_queue".into(),
                    summary: "The model decision could not be completed; production would queue a recheck."
                        .into(),
                };
            }
        };
        let actions = resolved
            .actions
            .iter()
            .map(|action| display_action_for_labels(action, labels))
            .collect();
        let (status, continued, terminal) = match resolved.outcome {
            Outcome::NoMatch => ("no_match", true, None),
            Outcome::Unparsed => ("invalid_decision", false, Some("would_queue")),
            Outcome::Matched if rule.continue_after_match => ("matched", true, None),
            Outcome::Matched => ("matched", false, Some("claimed")),
        };
        steps.push(PipelineDryRunStep {
            rule_id: rule.id,
            rule_name: rule.name.clone(),
            priority: rule.priority,
            status: status.into(),
            actions,
            reasoning: resolved.reason,
            diagnostic: resolved.diagnostic,
            llm_response: resolved.llm_response,
            llm_model: resolved.llm_model,
            llm_provider: resolved.llm_provider,
            prompt_tokens: resolved.prompt_tokens,
            completion_tokens: resolved.completion_tokens,
            total_tokens: resolved.total_tokens,
            duration_ms: resolved.llm_duration_ms,
            continued,
        });
        on_progress(PipelineDryRunProgress {
            phase: "completed".into(),
            rule_id: rule.id,
            rule_name: rule.name.clone(),
            priority: rule.priority,
            decision_estimate: None,
            step: steps.last().cloned(),
        });

        if let Some(status) = terminal {
            let summary = match status {
                "would_queue" => {
                    "The model reply was not a readable decision; production would queue a recheck."
                }
                _ => "A rule claimed the message, so lower-priority rules would not run.",
            };
            return PipelineDryRun {
                steps,
                status: status.into(),
                summary: summary.into(),
            };
        }
    }

    PipelineDryRun {
        steps,
        status: "no_match".into(),
        summary: "No enabled rule would claim this message.".into(),
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FallthroughStep {
    pub rule_id: i64,
    pub rule_name: String,
    pub matched: bool,
    pub indeterminate: bool,
    pub continued: bool,
    pub actions: Vec<ActionDisplay>,
    pub reasoning: String,
    pub diagnostic: Option<String>,
}

impl FallthroughStep {
    pub fn new(rule: &Rule, result: &TestResult) -> Self {
        Self {
            rule_id: rule.id,
            rule_name: rule.name.clone(),
            matched: result.matched,
            indeterminate: result.indeterminate,
            continued: result.matched && rule.continue_after_match,
            actions: result.actions.clone(),
            reasoning: result.reasoning.clone(),
            diagnostic: result.diagnostic.clone(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ActionDisplay {
    pub kind: String,
    pub detail: Option<String>,
    pub display: String,
}

pub fn display_action(action: &ParsedAction) -> ActionDisplay {
    match action {
        ParsedAction::Label(name) => ActionDisplay {
            kind: "label".into(),
            detail: Some(name.clone()),
            display: format!("Add label \"{}\"", name),
        },
        ParsedAction::RemoveLabel(name) => ActionDisplay {
            kind: "remove_label".into(),
            detail: Some(name.clone()),
            display: format!("Remove label \"{}\"", name),
        },
        ParsedAction::Archive => simple_action("archive", "Archive"),
        ParsedAction::Trash => simple_action("trash", "Move to Trash"),
        ParsedAction::Spam => simple_action("spam", "Mark as Spam"),
        ParsedAction::MarkRead => simple_action("mark_read", "Mark as read"),
        ParsedAction::MarkUnread => simple_action("mark_unread", "Mark as unread"),
        ParsedAction::Star => simple_action("star", "Star"),
    }
}

fn display_action_for_labels(action: &ParsedAction, labels: &[Label]) -> ActionDisplay {
    match action {
        ParsedAction::Label(id) => label_action_display("label", "Add label", id, labels),
        ParsedAction::RemoveLabel(id) => {
            label_action_display("remove_label", "Remove label", id, labels)
        }
        _ => display_action(action),
    }
}

fn label_action_display(kind: &str, verb: &str, id: &str, labels: &[Label]) -> ActionDisplay {
    let name = labels
        .iter()
        .find(|label| label.id == id)
        .map(|label| label.name.as_str())
        .unwrap_or(id);
    ActionDisplay {
        kind: kind.into(),
        detail: Some(id.into()),
        display: format!("{verb} \"{name}\""),
    }
}

fn simple_action(kind: &str, display: &str) -> ActionDisplay {
    ActionDisplay {
        kind: kind.into(),
        detail: None,
        display: display.into(),
    }
}

/// One outcome the model may pick, offered under `name` and executed as
/// `action`. Labels and actions are the same kind of thing to the model, which
/// is what lets a rule say "file it under A, B, or bin it".
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub name: String,
    pub action: ParsedAction,
}

impl Choice {
    /// Labels are offered by name, never by position: a positional index is only
    /// meaningful within the request that produced it, and resolving a stray
    /// number silently picks whatever label happens to sit at that offset.
    pub fn label(id: &str, name: &str) -> Self {
        Self {
            name: format!("\"{name}\""),
            action: ParsedAction::Label(id.into()),
        }
    }

    fn matches(&self, candidate: &str) -> bool {
        unquote(candidate).eq_ignore_ascii_case(unquote(&self.name))
    }
}

/// The menu offered with this rule, empty when the rule asks a plain
/// match-or-not question.
///
/// Choices naming a label Gmail no longer has are dropped rather than raised:
/// a stale menu entry is a rule to fix, not a reason to stall every email.
pub fn choice_catalog(rule: &Rule, labels: &[Label]) -> Vec<Choice> {
    let candidates: Vec<Choice> = if rule.choose_from_all_labels {
        labels
            .iter()
            .filter(|label| label.label_type.eq_ignore_ascii_case("user"))
            .map(|label| Choice::label(&label.id, &label.name))
            .collect()
    } else {
        rule.choices
            .iter()
            .filter_map(|action| choice_for(action, labels))
            .collect()
    };
    let mut catalog: Vec<Choice> = Vec::with_capacity(candidates.len());
    for choice in candidates {
        if !catalog
            .iter()
            .any(|existing| existing.matches(&choice.name))
        {
            catalog.push(choice);
        }
    }
    catalog
}

fn choice_for(action: &Action, labels: &[Label]) -> Option<Choice> {
    Some(match action {
        Action::Label { value } => {
            let label = find_label(value, labels)?;
            Choice::label(&label.id, &label.name)
        }
        Action::RemoveLabel { value } => {
            let label = find_label(value, labels)?;
            Choice {
                name: format!("REMOVE_LABEL \"{}\"", label.name),
                action: ParsedAction::RemoveLabel(label.id.clone()),
            }
        }
        Action::Archive => named_choice("ARCHIVE", ParsedAction::Archive),
        Action::Trash => named_choice("TRASH", ParsedAction::Trash),
        Action::Spam => named_choice("SPAM", ParsedAction::Spam),
        Action::MarkRead => named_choice("MARK_READ", ParsedAction::MarkRead),
        Action::MarkUnread => named_choice("MARK_UNREAD", ParsedAction::MarkUnread),
        Action::Star => named_choice("STAR", ParsedAction::Star),
    })
}

fn named_choice(name: &str, action: ParsedAction) -> Choice {
    Choice {
        name: name.into(),
        action,
    }
}

/// What a match runs: whatever the model chose, plus the rule's own recipe.
pub(crate) fn effective_actions(rule: &Rule, chosen: &[ParsedAction]) -> Vec<ParsedAction> {
    let mut actions = chosen.to_vec();
    for configured in rule.actions.iter().map(ParsedAction::from) {
        if !actions.contains(&configured) {
            actions.push(configured);
        }
    }
    actions
}

/// Explains why a matching rule changed nothing.
///
/// A rule that matches still claims the email, so an empty action list is a
/// silent dead end: history would otherwise only say "nothing happened".
pub fn no_action_reason(rule: &Rule, labels: &[Label], diagnostic: Option<&str>) -> String {
    let mut reason = String::from("Rule matched but produced no actions");
    if rule.actions.is_empty() && choice_catalog(rule, labels).is_empty() {
        reason.push_str(if rule.choices.is_empty() {
            "; it has no configured actions and offers the model no choices"
        } else {
            "; none of its choices match a label that still exists in Gmail"
        });
    }
    if let Some(diagnostic) = diagnostic {
        reason.push_str("; ");
        reason.push_str(diagnostic);
    }
    reason
}

/// One email's answer, as read off one line of the reply.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Row {
    pub text: String,
    pub matched: bool,
    pub chosen: Vec<ParsedAction>,
    pub reason: String,
    pub note: Option<String>,
}

impl Row {
    fn declined(text: &str, reason: String) -> Self {
        Self {
            text: text.to_string(),
            matched: false,
            chosen: vec![],
            reason,
            note: None,
        }
    }

    fn matched(text: &str, reason: String, chosen: Vec<ParsedAction>) -> Self {
        Self {
            text: text.to_string(),
            matched: true,
            chosen,
            reason,
            note: None,
        }
    }

    fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RowError {
    #[error("Expected a choice or NO_MATCH at the start of the line")]
    NotADecision,
}

const DECLINE_KEYWORDS: [&str; 3] = ["NO_MATCH", "NO MATCH", "SKIP"];

/// Reads one decision.
///
/// Newlines are field separators alongside `|`, so a model that spreads one
/// email's answer over several lines reads the same as the single line the
/// contract asks for.
pub(crate) fn parse_row(line: &str, menu: &[Choice]) -> Result<Row, RowError> {
    // A template wrapper around an otherwise valid decision reads the same as
    // the bare decision: `<1: NO_MATCH | ...>` and `1: <NO_MATCH | ...>` both
    // decline. Markers are stripped before and after so either ordering lands.
    let text = strip_markers(strip_angle_wrapper(&strip_markers(line)));
    let fields: Vec<&str> = text
        .split(['|', '\n'])
        .map(str::trim)
        .filter(|field| !field.is_empty())
        .collect();
    let &head = fields.first().ok_or(RowError::NotADecision)?;
    // Everything that is not the decision or a selection is the explanation.
    let reason = join_reason(fields.iter().enumerate().filter_map(|(index, field)| {
        (index > 0 && selection_tag(field).is_none()).then_some(*field)
    }));

    // The decline is read off the head before anything else, or a trailing
    // `LABELS: none` would turn "no match" into a match with no labels.
    let verdict = keyword(head);
    if let Some((Verdict::Decline, _)) = verdict {
        return Ok(Row::declined(&text, reason));
    }

    // A self-describing selection field outranks position, so `MATCH | CHOOSE:
    // Bills | why` and a bare `"Bills" | why` both land on the same selection.
    let tagged = fields.iter().find_map(|field| selection_tag(field));
    let declared_match = tagged.is_some() || verdict.is_some();
    let selection = tagged.unwrap_or(match verdict {
        Some((_, rest)) => rest,
        None => head,
    });

    // `none` is a deliberately empty answer only when no choice goes by that
    // name.
    if selection.is_empty()
        || (selection.eq_ignore_ascii_case("none")
            && !menu.iter().any(|choice| choice.matches(selection)))
    {
        let row = Row::matched(&text, reason, vec![]);
        return Ok(if menu.is_empty() {
            row
        } else {
            row.with_note("Matched without choosing from the menu")
        });
    }
    if menu.is_empty() {
        // A rule prompt that dictated its own vocabulary ("reply with only
        // TRASH") is honored as a plain match: the rule's configured actions
        // decide the effect, so the token itself is deliberately discarded.
        if let Ok(action) = ParsedAction::from_str(selection) {
            return Ok(Row::matched(&text, reason, vec![]).with_note(format!(
                "Model answered \"{}\" although no choices were offered; ran the rule's actions instead",
                display_action(&action).display
            )));
        }
        return if declared_match {
            Ok(Row::matched(&text, reason, vec![])
                .with_note("Model chose a name although no choices were offered; ignored it"))
        } else {
            Err(RowError::NotADecision)
        };
    }
    resolve_selection(&text, reason, selection, menu, declared_match)
}

/// Resolves chosen names against the menu, reporting rather than raising the
/// ones it does not contain: a hallucinated name is a classification miss, not
/// a reason to stall the email against every remaining rule.
fn resolve_selection(
    text: &str,
    mut reason: String,
    selection: &str,
    menu: &[Choice],
    declared_match: bool,
) -> Result<Row, RowError> {
    // Gmail permits commas inside label names, so try the whole selection as a
    // single name before treating commas as separators.
    if let Some(choice) = menu.iter().find(|choice| choice.matches(selection)) {
        return Ok(Row::matched(text, reason, vec![choice.action.clone()]));
    }

    if let Some((choice, explanation)) = menu
        .iter()
        .find_map(|choice| choice_with_explanation(choice, selection))
    {
        reason = if reason.is_empty() {
            explanation.to_string()
        } else {
            format!("{explanation} {reason}")
        };
        return Ok(Row::matched(text, reason, vec![choice.action.clone()]));
    }

    let mut chosen = Vec::new();
    let mut unknown = Vec::new();
    for value in selection.split(',').map(str::trim) {
        if value.is_empty() {
            continue;
        }
        match menu.iter().find(|choice| choice.matches(value)) {
            Some(choice) if !chosen.contains(&choice.action) => chosen.push(choice.action.clone()),
            Some(_) => {}
            None => unknown.push(format!("\"{}\"", unquote(value))),
        }
    }
    if chosen.is_empty() && !declared_match {
        // The line opened with prose rather than a choice, so it is not an
        // answer to this email at all.
        return Err(RowError::NotADecision);
    }
    let row = Row::matched(text, reason, chosen);
    Ok(match unknown.is_empty() {
        true => row,
        false => row.with_note(format!("Ignored {} — not on the menu", unknown.join(", "))),
    })
}

fn choice_with_explanation<'choice, 'selection>(
    choice: &'choice Choice,
    selection: &'selection str,
) -> Option<(&'choice Choice, &'selection str)> {
    [choice.name.as_str(), unquote(&choice.name)]
        .into_iter()
        .find_map(|name| {
            let prefix = selection.get(..name.len())?;
            prefix
                .eq_ignore_ascii_case(name)
                .then(|| selection.get(name.len()..))?
                .and_then(|suffix| suffix.strip_prefix(" -- "))
                .filter(|explanation| !explanation.is_empty())
        })
        .map(|explanation| (choice, explanation))
}

enum Verdict {
    Decline,
    Match,
}

/// Splits a leading decision keyword off a field, returning what follows it.
fn keyword(field: &str) -> Option<(Verdict, &str)> {
    for marker in DECLINE_KEYWORDS {
        if let Some(rest) = keyword_suffix(field, marker) {
            return Some((Verdict::Decline, rest));
        }
    }
    keyword_suffix(field, "MATCH").map(|rest| (Verdict::Match, rest))
}

fn keyword_suffix<'a>(field: &'a str, marker: &str) -> Option<&'a str> {
    let prefix = field.get(..marker.len())?;
    if !prefix.eq_ignore_ascii_case(marker) {
        return None;
    }
    let rest = field.get(marker.len()..)?;
    let ends_here = rest
        .chars()
        .next()
        .is_none_or(|character| !character.is_alphanumeric() && character != '_');
    ends_here.then(|| rest.trim_start_matches([':', '-']).trim())
}

/// The selection carried by a field that names itself, e.g. `CHOOSE: Bills`.
fn selection_tag(field: &str) -> Option<&str> {
    ["CHOOSE:", "LABELS:", "LABEL:"]
        .into_iter()
        .find_map(|tag| {
            field
                .get(..tag.len())
                .filter(|prefix| prefix.eq_ignore_ascii_case(tag))
                .and_then(|_| field.get(tag.len()..))
                .map(str::trim)
        })
}

fn join_reason<'a>(fields: impl Iterator<Item = &'a str>) -> String {
    fields.collect::<Vec<_>>().join(" ")
}

/// Drops the ordering markers the contract asks for, plus the ones it does not.
///
/// The number is only an alignment aid, so nothing downstream depends on which
/// marker the model wrote — `3:`, a copied placeholder `N:`, or a bullet.
fn strip_markers(line: &str) -> String {
    let mut line = line.trim();
    while let Some(marker) = MARKER_RE.find(line) {
        let rest = line[marker.end()..].trim_start();
        if rest.is_empty() {
            break;
        }
        line = rest;
    }
    line.to_string()
}

/// Drops one outer angle-bracket wrapper copied from a templated example.
/// Models that see `<choice> | <reason>` in the prompt mirror the delimiters
/// around an otherwise valid decision, e.g. `<NO_MATCH | banking>`.
fn strip_angle_wrapper(line: &str) -> &str {
    let trimmed = line.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('<') && trimmed.ends_with('>') {
        let inner = trimmed[1..trimmed.len() - 1].trim();
        if !inner.is_empty() {
            return inner;
        }
    }
    trimmed
}

static MARKER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*(?:(?:n|\d+)\s*[:.)]|[-*\u{2022}])\s*").unwrap());

pub(crate) fn email_parts(email: &Message) -> (String, String) {
    let headers = email
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
    (headers, extract_plain_text(email))
}

pub(crate) fn estimated_tokens(value: &str) -> usize {
    value.chars().count().div_ceil(4)
}

/// Resolves a stored label reference, which may be either a Gmail id or a name.
pub(crate) fn find_label<'a>(value: &str, labels: &'a [Label]) -> Option<&'a Label> {
    let value = unquote(value);
    labels
        .iter()
        .find(|label| label.id == value || label.name.eq_ignore_ascii_case(value))
}

/// Trims one pair of wrapping quotes. `trim_matches` would also eat quotes that
/// belong to the name itself.
fn unquote(value: &str) -> &str {
    let value = value.trim();
    ['"', '\'']
        .into_iter()
        .find_map(|quote| {
            value
                .strip_prefix(quote)
                .and_then(|inner| inner.strip_suffix(quote))
        })
        .map(str::trim)
        .unwrap_or(value)
}

pub(crate) fn fit_email_body(
    prefix: &str,
    body: &str,
    suffix: &str,
    input_budget: usize,
) -> Result<String, RuleError> {
    let fixed_tokens = estimated_tokens(prefix) + estimated_tokens(suffix);
    let Some(body_tokens) = input_budget.checked_sub(fixed_tokens) else {
        return Err(RuleError::PromptTooLarge);
    };
    let body = truncate_body(body, body_tokens.saturating_mul(4));
    Ok(format!("{prefix}{body}{suffix}"))
}

pub(crate) fn truncate_body(body: &str, max_chars: usize) -> String {
    if body.chars().count() <= max_chars {
        return body.to_string();
    }
    if max_chars <= TRUNCATION_MARKER.chars().count() {
        return TRUNCATION_MARKER.chars().take(max_chars).collect();
    }
    let content_chars = max_chars - TRUNCATION_MARKER.chars().count();
    let start_chars = content_chars * 3 / 5;
    let end_chars = content_chars - start_chars;
    let start: String = body.chars().take(start_chars).collect();
    let end: String = body
        .chars()
        .rev()
        .take(end_chars)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{start}{TRUNCATION_MARKER}{end}")
}

pub(crate) fn memory_block(memories: &[String]) -> String {
    if memories.is_empty() {
        String::new()
    } else {
        format!(
            "\n\n--- Learned Memory ---\n{}\n(Respect these exceptions and notes when deciding.)",
            memories.join("\n")
        )
    }
}

fn extract_plain_text(email: &Message) -> String {
    let Some(payload) = &email.payload else {
        return email.snippet.clone();
    };
    let find_plain = |parts: &[MessagePayload]| {
        parts
            .iter()
            .find(|part| part.mime_type == "text/plain")
            .and_then(|part| part.body.as_ref())
            .and_then(|body| body.data.as_deref())
            .map(decode_base64url)
    };
    let find_html = |parts: &[MessagePayload]| {
        parts
            .iter()
            .find(|part| part.mime_type == "text/html")
            .and_then(|part| part.body.as_ref())
            .and_then(|body| body.data.as_deref())
            .map(|data| strip_html(&decode_base64url(data)))
    };
    payload
        .body
        .as_ref()
        .and_then(|body| body.data.as_deref())
        .filter(|_| payload.mime_type == "text/plain")
        .map(decode_base64url)
        .or_else(|| payload.parts.as_deref().and_then(find_plain))
        .or_else(|| payload.parts.as_deref().and_then(find_html))
        .unwrap_or_else(|| email.snippet.clone())
}

fn decode_base64url(data: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE
        .decode(data)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}

fn strip_html(html: &str) -> String {
    static TAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").unwrap());
    TAG_RE
        .replace_all(html, "")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::models::{Condition, Operator};

    fn label(id: &str, name: &str) -> Label {
        Label {
            id: id.into(),
            name: name.into(),
            label_type: "user".into(),
            message_list_visibility: None,
            label_list_visibility: None,
        }
    }

    pub(crate) fn rule_with_actions(actions: Vec<Action>) -> Rule {
        Rule {
            id: 1,
            name: "r".into(),
            description: None,
            conditions: vec![],
            prompt: "classify".into(),
            choices: vec![],
            choose_from_all_labels: false,
            actions,
            priority: 0,
            enabled: true,
            parent_id: None,
            inference_policy: "default".into(),
            decision_reasoning_effort: crate::llm::ReasoningEffort::ServerDefault,
            decision_max_tokens: None,
            continue_after_match: false,
        }
    }

    fn menu() -> Vec<Choice> {
        vec![
            Choice::label("Label_1", "Education/Saarth"),
            Choice::label("Label_2", "Bills"),
        ]
    }

    fn chosen(line: &str, menu: &[Choice]) -> Vec<ParsedAction> {
        let row = parse_row(line, menu).expect("row should parse");
        assert!(row.matched, "expected a match for {line:?}");
        row.chosen
    }

    #[test]
    fn a_decline_needs_no_menu() {
        for line in [
            "NO_MATCH - unrelated",
            "NO MATCH | Reason",
            "SKIP | uncertain",
            "no_match",
            // A trailing empty selection must not read as a match.
            "NO_MATCH | LABELS: none | unrelated",
        ] {
            let row = parse_row(line, &menu()).expect("row should parse");
            assert!(!row.matched, "{line:?} should decline");
        }
    }

    /// The contract: the choice itself is the answer, which is also how rule
    /// prompts are naturally written ("classify as A, B, or NO_MATCH").
    #[test]
    fn a_bare_choice_name_is_the_answer() {
        assert_eq!(
            chosen("\"Education/Saarth\" | flute recital", &menu()),
            vec![ParsedAction::Label("Label_1".into())]
        );
        assert_eq!(
            chosen("Bills", &menu()),
            vec![ParsedAction::Label("Label_2".into())]
        );
    }

    #[test]
    fn a_choice_followed_by_a_double_dash_explanation_is_accepted() {
        let menu = vec![Choice::label("Label_1", "Education")];
        let row = parse_row(
            "Education -- Generic information about college visits and class officer elections",
            &menu,
        )
        .expect("choice with explanation should parse");

        assert_eq!(row.chosen, vec![ParsedAction::Label("Label_1".into())]);
        assert_eq!(
            row.reason,
            "Generic information about college visits and class officer elections"
        );
    }

    #[test]
    fn a_match_keyword_and_a_tagged_selection_read_the_same() {
        let expected = vec![ParsedAction::Label("Label_2".into())];

        assert_eq!(chosen("MATCH: Bills | billing", &menu()), expected);
        assert_eq!(chosen("MATCH | CHOOSE: Bills | billing", &menu()), expected);
        assert_eq!(chosen("MATCH | LABELS: Bills | billing", &menu()), expected);
        assert_eq!(chosen("MATCH\nLABELS: Bills\nbilling", &menu()), expected);
    }

    #[test]
    fn the_reason_is_everything_that_is_not_a_decision() {
        let row = parse_row("1: MATCH | CHOOSE: Bills | Billing statement.", &menu()).unwrap();
        assert_eq!(row.reason, "Billing statement.");

        let row = parse_row("NO_MATCH | Unrelated tech news.", &menu()).unwrap();
        assert_eq!(row.reason, "Unrelated tech news.");
    }

    /// The reported failure: the model copies the literal `N:` out of the
    /// template instead of substituting a number.
    #[test]
    fn ordering_markers_are_ignored_whatever_the_model_writes() {
        for line in [
            "1: NO_MATCH | banking notification",
            "N: NO_MATCH | banking notification",
            "1: N: NO_MATCH | banking notification",
            "- NO_MATCH | banking notification",
            "3) NO_MATCH | banking notification",
        ] {
            let row = parse_row(line, &[]).expect("row should parse");
            assert!(!row.matched, "{line:?} should decline");
            assert_eq!(row.reason, "banking notification");
        }
    }

    #[test]
    fn a_marker_is_never_mistaken_for_the_decision_itself() {
        assert!(!parse_row("1: NO_MATCH", &[]).unwrap().matched);
        assert!(parse_row("MATCH", &[]).unwrap().matched);
    }

    /// The reported Gemma failure: the model wrapped an otherwise valid
    /// decision in the template's angle brackets.
    #[test]
    fn a_template_wrapper_around_a_decision_still_reads() {
        for line in [
            "<NO_MATCH | banking notification>",
            "<1: NO_MATCH | banking notification>",
            "1: <NO_MATCH | banking notification>",
            "<NO_MATCH | The email content is about banking and data sharing with Splitwise from Chase, which does not relate to music.>",
        ] {
            let row = parse_row(line, &[]).expect("row should parse");
            assert!(!row.matched, "{line:?} should decline");
        }

        let row = parse_row("<Bills | Monthly power bill.>", &menu())
            .expect("wrapped choice should parse");
        assert_eq!(row.chosen, vec![ParsedAction::Label("Label_2".into())]);
    }

    #[test]
    fn prose_is_not_a_decision() {
        for line in [
            "I am not sure about this one",
            "Here are the decisions:",
            "All three relate to Bills in some way",
        ] {
            assert_eq!(parse_row(line, &menu()), Err(RowError::NotADecision));
        }
    }

    #[test]
    fn a_name_containing_a_comma_resolves_before_the_split() {
        let menu = vec![Choice::label("Label_1", "Bills, Utilities")];

        assert_eq!(
            chosen("MATCH | CHOOSE: Bills, Utilities", &menu),
            vec![ParsedAction::Label("Label_1".into())]
        );
    }

    #[test]
    fn unknown_names_are_dropped_and_reported() {
        let row = parse_row("MATCH | CHOOSE: Bills, Nonsense | why", &menu()).unwrap();

        assert_eq!(row.chosen, vec![ParsedAction::Label("Label_2".into())]);
        assert!(row.note.unwrap().contains("\"Nonsense\""));
    }

    /// Numbers stopped being meaningful once choices are offered by name, so a
    /// stray one must not silently pick whatever sits at that offset.
    #[test]
    fn a_number_is_not_a_choice() {
        let row = parse_row("MATCH | CHOOSE: 1 | why", &menu()).unwrap();

        assert!(row.chosen.is_empty());
        assert!(row.note.unwrap().contains("not on the menu"));
    }

    /// A menu was offered and the model skipped the choice, so the match
    /// silently produced nothing. Say so.
    #[test]
    fn a_match_that_skips_the_menu_is_flagged() {
        let row = parse_row("MATCH | The email discusses college visits.", &menu()).unwrap();
        assert!(row.chosen.is_empty());
        assert!(row.note.unwrap().contains("without choosing"));

        // With no menu on offer there is nothing to report.
        let row = parse_row("MATCH | The email discusses college visits.", &[]).unwrap();
        assert!(row.note.is_none());
    }

    /// A rule prompt authored against its own vocabulary ("reply with only
    /// TRASH") still has to resolve rather than stall.
    #[test]
    fn a_bare_action_token_is_a_match_when_nothing_was_offered() {
        let row = parse_row("TRASH", &[]).unwrap();

        assert!(row.matched);
        assert!(row.chosen.is_empty());
        assert!(row.note.unwrap().contains("Move to Trash"));
    }

    #[test]
    fn an_action_on_the_menu_is_chosen_like_any_other() {
        let menu = vec![
            Choice::label("Label_1", "Needs Action"),
            Choice {
                name: "TRASH".into(),
                action: ParsedAction::Trash,
            },
        ];

        assert_eq!(
            chosen("TRASH | github noise", &menu),
            vec![ParsedAction::Trash]
        );
    }

    #[test]
    fn the_catalog_offers_labels_and_actions_by_name() {
        let labels = vec![label("Label_1", "Needs Action"), label("Label_2", "Bills")];
        let mut rule = rule_with_actions(vec![Action::Archive]);
        rule.choices = vec![
            Action::Label {
                value: "Label_1".into(),
            },
            Action::Trash,
            Action::Label {
                value: "Label_gone".into(),
            },
        ];

        let names: Vec<String> = choice_catalog(&rule, &labels)
            .into_iter()
            .map(|choice| choice.name)
            .collect();
        assert_eq!(names, vec!["\"Needs Action\"", "TRASH"]);
    }

    #[test]
    fn choosing_from_all_labels_ignores_the_configured_choices() {
        let labels = vec![label("Label_1", "Needs Action"), label("Label_2", "Bills")];
        let mut rule = rule_with_actions(vec![]);
        rule.choices = vec![Action::Trash];
        rule.choose_from_all_labels = true;

        assert_eq!(choice_catalog(&rule, &labels).len(), 2);
    }

    #[test]
    fn a_match_runs_the_choice_and_the_recipe() {
        let rule = rule_with_actions(vec![Action::Archive]);

        assert_eq!(
            effective_actions(&rule, &[ParsedAction::Label("Label_2".into())]),
            vec![ParsedAction::Label("Label_2".into()), ParsedAction::Archive]
        );
    }

    #[test]
    fn no_action_reason_calls_out_a_rule_that_cannot_act() {
        let bare = rule_with_actions(vec![]);
        let reason = no_action_reason(&bare, &[], None);
        assert!(reason.contains("no configured actions"));
        assert!(reason.contains("no choices"));

        let with_actions = rule_with_actions(vec![Action::Archive]);
        assert_eq!(
            no_action_reason(&with_actions, &[], Some("some note")),
            "Rule matched but produced no actions; some note"
        );
    }

    /// The reported case: the menu draws from labels that no longer exist, so
    /// the model has nothing to pick and the match applies nothing.
    #[test]
    fn no_action_reason_calls_out_a_stale_menu() {
        let mut rule = rule_with_actions(vec![]);
        rule.choices = vec![Action::Label {
            value: "Label_gone".into(),
        }];
        assert!(no_action_reason(&rule, &[], None).contains("still exists in Gmail"));

        let labels = vec![label("Label_gone", "Financial")];
        assert_eq!(
            no_action_reason(&rule, &labels, None),
            "Rule matched but produced no actions"
        );
    }

    fn rule_at(id: i64, priority: i32, enabled: bool, conditions: Vec<Condition>) -> Rule {
        let mut rule = rule_with_actions(vec![]);
        rule.id = id;
        rule.priority = priority;
        rule.enabled = enabled;
        rule.conditions = conditions;
        rule
    }

    fn email_from(sender: &str) -> Message {
        Message {
            id: "m1".into(),
            thread_id: "t1".into(),
            label_ids: vec![],
            snippet: String::new(),
            history_id: "1".into(),
            internal_date: String::new(),
            size_estimate: 0,
            payload: Some(MessagePayload {
                mime_type: "text/plain".into(),
                headers: vec![crate::gmail::models::Header {
                    name: "From".into(),
                    value: sender.into(),
                }],
                body: None,
                parts: None,
                filename: None,
            }),
        }
    }

    #[test]
    fn rules_after_yields_lower_priority_rules_in_order() {
        let rules = vec![
            rule_at(1, 0, true, vec![]),
            rule_at(2, 1, true, vec![]),
            rule_at(3, 2, true, vec![]),
        ];

        let ids: Vec<i64> = rules_after(&rules, 1, &email_from("a@b.com"))
            .iter()
            .map(|rule| rule.id)
            .collect();
        assert_eq!(ids, vec![2, 3]);

        assert!(rules_after(&rules, 3, &email_from("a@b.com")).is_empty());
    }

    #[test]
    fn rules_after_skips_disabled_and_condition_filtered_rules() {
        let rules = vec![
            rule_at(1, 0, true, vec![]),
            rule_at(2, 1, false, vec![]),
            rule_at(
                3,
                2,
                true,
                vec![Condition::From {
                    operator: Operator::Contains,
                    value: "nope@example.com".into(),
                }],
            ),
            rule_at(4, 3, true, vec![]),
        ];

        let ids: Vec<i64> = rules_after(&rules, 1, &email_from("a@b.com"))
            .iter()
            .map(|rule| rule.id)
            .collect();
        assert_eq!(ids, vec![4]);
    }

    /// An unsaved draft has no stored id, so there is no position to continue from.
    #[test]
    fn rules_after_is_empty_for_an_unknown_rule_id() {
        let rules = vec![rule_at(1, 0, true, vec![]), rule_at(2, 1, true, vec![])];

        assert!(rules_after(&rules, 0, &email_from("a@b.com")).is_empty());
    }

    #[test]
    fn truncation_keeps_both_ends() {
        let body = "abcdefghijklmnopqrstuvwxyz".repeat(4);
        let truncated = truncate_body(&body, 80);
        assert!(truncated.starts_with('a'));
        assert!(truncated.ends_with('z'));
        assert!(truncated.contains("truncated"));
    }
}
