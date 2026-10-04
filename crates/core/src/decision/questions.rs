//! Turns a rule into rverdict questions, and reads the LLM's and rverdict's
//! answers back in the same terms so they can be compared.

use rverdict_core::{Calibration, Logits, RenderedKind, Request};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::rules::email_view::EmailView;
use crate::rules::engine::{parse_row, Choice};
use crate::rules::models::Rule;
use crate::rules::response_parser::ParsedAction;

/// How a rule is put to rverdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Framing {
    /// Instruction-only rule as a yes/no question.
    Noul,
    /// Instruction-only rule as a choice between two described options.
    Binary,
    /// Menu rule: one option per choice plus "none of these apply".
    Menu,
}

impl Framing {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Noul => "noul",
            Self::Binary => "binary",
            Self::Menu => "menu",
        }
    }
}

/// What picking an option means for the rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "name", rename_all = "snake_case")]
pub enum Meaning {
    Applies,
    DoesNotApply,
    /// A menu entry, by its name in the rule's menu.
    Choice(String),
}

/// One question asked about one email for one rule.
#[derive(Debug, Clone)]
pub struct Asked {
    pub framing: Framing,
    pub question: Value,
    /// Option meanings in the order rverdict scores them.
    pub options: Vec<Meaning>,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub request: Request,
    pub asked: Vec<Asked>,
}

/// Bumped whenever the questions asked for the same rule change, so old
/// verdicts are not mixed with new ones: verdicts are recorded per model id
/// and this version.
pub const QUESTIONS_VERSION: u32 = 1;

const NONE_OF_THESE: &str = "None of these: the email is about something else";

/// A rule prompt split into what rverdict needs: the instruction, the
/// descriptions it gives menu entries (`- "Name" -- description`), and what
/// it says NO_MATCH means. Lines that only explain the LLM's reply format
/// are dropped.
#[derive(Debug, Default, PartialEq)]
struct Prompt {
    instruction: String,
    descriptions: Vec<Option<String>>,
    none: Option<String>,
}

static LIST_ITEM: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r#"^\s*[-*•]\s*"?(?P<name>[^"]+?)"?\s*(?:--|—|–|:|\s-)\s*(?P<desc>.+)$"#)
        .expect("valid regex")
});
static NO_MATCH: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)^\s*[-*•]?\s*(?:reply\s+)?no[_ ]match\b\s*(?:--|—|–|:|-)?\s*(?P<rest>.*)$",
    )
    .expect("valid regex")
});
static REPLY_MATCH: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(?:reply|answer|respond)(?:\s+with)?\s+match\s+if\s+")
        .expect("valid regex")
});

fn normalized(name: &str) -> String {
    name.trim().trim_matches('"').to_lowercase()
}

/// The menu entry a prompt's list item describes: an exact name, else the
/// one entry that differs only in its ending ("Finance" for "Financial"),
/// never across a `/`, so "Education/High" is not "Education".
fn menu_entry(menu: &[Choice], name: &str) -> Option<usize> {
    let name = normalized(name);
    if let Some(exact) = menu.iter().position(|c| normalized(&c.name) == name) {
        return Some(exact);
    }
    let close = |entry: &str| {
        let common = entry
            .chars()
            .zip(name.chars())
            .take_while(|(a, b)| a == b)
            .count();
        let shorter = entry.chars().count().min(name.chars().count());
        let rest = |s: &str| s.chars().skip(common).any(|c| c == '/');
        common >= 5 && common + 1 >= shorter && !rest(entry) && !rest(&name)
    };
    let mut candidates = menu
        .iter()
        .enumerate()
        .filter(|(_, c)| close(&normalized(&c.name)));
    match (candidates.next(), candidates.next()) {
        (Some((index, _)), None) => Some(index),
        _ => None,
    }
}

/// The "none of these apply" option, worded with what the prompt says
/// NO_MATCH means when it says.
fn none_text(said: Option<&str>) -> String {
    let Some(said) = said
        .map(|s| s.trim().trim_start_matches("if ").trim())
        .filter(|s| !s.is_empty())
    else {
        return NONE_OF_THESE.to_owned();
    };
    if said.to_lowercase().starts_with("none") {
        let mut chars = said.chars();
        chars
            .next()
            .map(|c| c.to_uppercase().chain(chars).collect())
            .unwrap_or_default()
    } else {
        format!("None of these: {said}")
    }
}

fn parse_prompt(prompt: &str, menu: &[Choice]) -> Prompt {
    let mut parsed = Prompt {
        descriptions: vec![None; menu.len()],
        ..Prompt::default()
    };
    let mut kept = Vec::new();
    for line in prompt.lines() {
        if let Some(caps) = NO_MATCH.captures(line) {
            let rest = caps["rest"].trim();
            if !rest.is_empty() {
                parsed.none = Some(rest.to_owned());
            }
            continue;
        }
        if let Some(caps) = LIST_ITEM.captures(line) {
            if let Some(index) = menu_entry(menu, &caps["name"]) {
                parsed.descriptions[index] = Some(caps["desc"].trim().to_owned());
                continue;
            }
        }
        let trimmed = line.trim().trim_end_matches(':');
        if trimmed.eq_ignore_ascii_case("classify as")
            || trimmed.eq_ignore_ascii_case("classify it as")
        {
            continue;
        }
        kept.push(REPLY_MATCH.replace_all(line, "").into_owned());
    }
    parsed.instruction = kept.join("\n").trim().to_owned();
    parsed
}

/// The questions for `rule` about `view`, or `None` when the rule makes no
/// model decision. Instruction-only rules are asked both ways, so shadow
/// data shows which framing agrees better with the LLM.
pub fn plan(rule: &Rule, menu: &[Choice], view: &EmailView, memories: &[String]) -> Option<Plan> {
    let prompt = parse_prompt(&rule.prompt, menu);
    let instruction = prompt.instruction.as_str();
    let notes = if memories.is_empty() {
        String::new()
    } else {
        format!("\n\nNotes:\n- {}", memories.join("\n- "))
    };
    let asked = if menu.is_empty() {
        if instruction.is_empty() {
            return None;
        }
        let instructions = format!("{instruction}{notes}");
        vec![
            Asked {
                framing: Framing::Noul,
                question: json!({"type": "noul", "instructions": instructions}),
                options: vec![Meaning::Applies, Meaning::DoesNotApply],
            },
            Asked {
                framing: Framing::Binary,
                question: json!({"type": "choice", "instructions": instructions, "criteria": {
                    "applies": "The rule applies to this email",
                    "does_not_apply": "The rule does not apply to this email",
                }}),
                options: vec![Meaning::Applies, Meaning::DoesNotApply],
            },
        ]
    } else {
        let instructions = if instruction.is_empty() {
            format!("Which of these best describes this email?{notes}")
        } else {
            format!("{instruction}\n\nWhich of these best describes this email?{notes}")
        };
        let mut criteria = Map::new();
        let mut options = Vec::with_capacity(menu.len() + 1);
        for (index, choice) in menu.iter().enumerate() {
            let text = match &prompt.descriptions[index] {
                Some(description) => format!("{}: {description}", describe(choice)),
                None => describe(choice),
            };
            criteria.insert(format!("c{index}"), Value::String(text));
            options.push(Meaning::Choice(choice.name.clone()));
        }
        criteria.insert(
            "none".into(),
            Value::String(none_text(prompt.none.as_deref())),
        );
        options.push(Meaning::DoesNotApply);
        vec![Asked {
            framing: Framing::Menu,
            question: json!({"type": "choice", "instructions": instructions, "criteria": criteria}),
            options,
        }]
    };

    let questions: Map<String, Value> = asked
        .iter()
        .map(|a| (a.framing.as_str().to_owned(), a.question.clone()))
        .collect();
    let request = Request {
        state: Value::String(state_text(view)),
        model: None,
        questions,
    };
    Some(Plan { request, asked })
}

/// The email as rverdict reads it: the same headers and cleaned body the LLM
/// is shown.
pub fn state_text(view: &EmailView) -> String {
    format!("{}\n\n{}", view.rendered_headers(), view.body)
}

/// A menu entry as an option description. Von reads options as phrases, so
/// action tokens become short descriptions.
fn describe(choice: &Choice) -> String {
    let unquoted = |s: &str| s.trim().trim_matches('"').replace('/', " / ");
    match &choice.action {
        ParsedAction::Label(_) => unquoted(&choice.name),
        ParsedAction::RemoveLabel(_) => format!(
            "Remove the label {}",
            unquoted(choice.name.trim_start_matches("REMOVE_LABEL"))
        ),
        ParsedAction::Archive => "Archive it: nothing to keep in the inbox".into(),
        ParsedAction::Trash => "Delete it: junk or not worth keeping".into(),
        ParsedAction::Spam => "Spam: unsolicited bulk mail or a scam".into(),
        ParsedAction::MarkRead => "Mark it read: nothing that needs reading".into(),
        ParsedAction::MarkUnread => "Keep it unread: it needs attention".into(),
        ParsedAction::Star => "Star it: important".into(),
    }
}

/// The LLM's decision for a recorded step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LlmAnswer {
    pub matched: bool,
    /// Names of the menu entries it chose.
    pub chosen: Vec<String>,
}

/// Reads a recorded step back as the LLM's answer. The stored reply is
/// re-parsed against the menu, exactly as the live pipeline parsed it.
pub fn llm_answer(outcome: &str, reply: Option<&str>, menu: &[Choice]) -> Option<LlmAnswer> {
    match outcome {
        "no_match" => Some(LlmAnswer {
            matched: false,
            chosen: vec![],
        }),
        "matched" | "matched_no_action" => {
            let chosen = reply
                .and_then(|text| parse_row(text, menu).ok())
                .map(|row| {
                    row.chosen
                        .iter()
                        .filter_map(|action| menu.iter().find(|c| c.action == *action))
                        .map(|c| c.name.clone())
                        .collect()
                })
                .unwrap_or_default();
            Some(LlmAnswer {
                matched: true,
                chosen,
            })
        }
        _ => None,
    }
}

/// rverdict's answer to one asked question, compared with the LLM's.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub probabilities: Vec<f64>,
    pub matched: bool,
    pub choice: Option<String>,
    /// Probability of the answer given: the top option.
    pub confidence: f64,
    /// Index of the LLM's answer among the options, when it has one.
    pub llm_option: Option<usize>,
    pub agrees: Option<bool>,
}

pub fn verdict(
    asked: &Asked,
    kind: &RenderedKind,
    logits: &Logits,
    calibration: &Calibration,
    llm: Option<&LlmAnswer>,
) -> Verdict {
    let probabilities = calibration.distribution(kind, logits);
    let (best, confidence) =
        probabilities
            .iter()
            .copied()
            .enumerate()
            .fold((0, f64::NEG_INFINITY), |best, (i, p)| {
                if p > best.1 {
                    (i, p)
                } else {
                    best
                }
            });
    let meaning = &asked.options[best];
    let matched = *meaning != Meaning::DoesNotApply;
    let choice = match meaning {
        Meaning::Choice(name) => Some(name.clone()),
        _ => None,
    };

    let llm_option = llm.and_then(|llm| {
        asked.options.iter().position(|m| match m {
            Meaning::Applies => llm.matched,
            Meaning::DoesNotApply => !llm.matched,
            Meaning::Choice(name) => llm.matched && llm.chosen.first() == Some(name),
        })
    });
    let agrees = llm.map(|llm| match (meaning, llm.matched) {
        (Meaning::DoesNotApply, matched) => !matched,
        (Meaning::Applies, matched) => matched,
        // A match without a recognisable menu entry still agrees on matching.
        (Meaning::Choice(name), true) => llm.chosen.is_empty() || llm.chosen.contains(name),
        (Meaning::Choice(_), false) => false,
    });
    Verdict {
        probabilities,
        matched,
        choice,
        confidence,
        llm_option,
        agrees,
    }
}

#[cfg(test)]
mod tests {
    use rverdict_core::Logits;

    use super::*;
    use crate::gmail::models::{Header, Message, MessagePayload};

    fn rule(prompt: &str) -> Rule {
        serde_json::from_value(json!({
            "id": 7, "name": "Test", "description": null, "conditions": [], "prompt": prompt,
            "actions": [], "priority": 0, "enabled": true, "parent_id": null
        }))
        .unwrap()
    }

    fn view() -> EmailView {
        EmailView::from_message(&Message {
            id: "m1".into(),
            thread_id: "t1".into(),
            label_ids: vec!["INBOX".into()],
            snippet: "Your invoice is attached".into(),
            history_id: "1".into(),
            internal_date: String::new(),
            size_estimate: 0,
            payload: Some(MessagePayload {
                mime_type: "text/plain".into(),
                headers: vec![Header {
                    name: "Subject".into(),
                    value: "Invoice 4411".into(),
                }],
                body: None,
                parts: None,
                filename: None,
            }),
        })
    }

    fn menu() -> Vec<Choice> {
        vec![
            Choice::label("Label_1", "Financial"),
            Choice::label("Label_2", "Education/High"),
            Choice {
                name: "TRASH".into(),
                action: ParsedAction::Trash,
            },
        ]
    }

    #[test]
    fn instruction_only_rules_are_asked_both_ways() {
        let plan = plan(&rule("Is this a MongoDB alert?"), &[], &view(), &[]).unwrap();
        let framings: Vec<_> = plan.asked.iter().map(|a| a.framing).collect();
        assert_eq!(framings, [Framing::Noul, Framing::Binary]);
        assert_eq!(plan.request.questions.len(), 2);
        assert!(plan.request.parse_questions().is_ok());
        assert_eq!(
            plan.request.state,
            Value::String("Subject: Invoice 4411\n\nYour invoice is attached".into())
        );
    }

    #[test]
    fn menus_get_described_options_and_a_none_option() {
        let plan = plan(&rule(""), &menu(), &view(), &["Bills are Financial".into()]).unwrap();
        let question = &plan.asked[0].question;
        assert_eq!(
            question["criteria"],
            json!({"c0": "Financial", "c1": "Education / High", "c2": "Delete it: junk or not worth keeping",
                   "none": "None of these: the email is about something else"})
        );
        assert!(question["instructions"]
            .as_str()
            .unwrap()
            .ends_with("Notes:\n- Bills are Financial"));
        assert_eq!(plan.asked[0].options.last(), Some(&Meaning::DoesNotApply));
    }

    #[test]
    fn prompts_lend_their_descriptions_to_the_menu() {
        let prompt = "Apply to emails sent directly to me.\n\nClassify as\n\
            - \"Financial\" -- is an invoice or billing to existing services.\n\
            - Education/High -- Generic school announcements\n\
            - NO_MATCH -- None of the above are appropriate";
        let plan = plan(&rule(prompt), &menu(), &view(), &[]).unwrap();
        let question = &plan.asked[0].question;
        assert_eq!(
            question["criteria"],
            json!({"c0": "Financial: is an invoice or billing to existing services.",
                   "c1": "Education / High: Generic school announcements",
                   "c2": "Delete it: junk or not worth keeping",
                   "none": "None of the above are appropriate"})
        );
        assert_eq!(
            question["instructions"],
            "Apply to emails sent directly to me.\n\nWhich of these best describes this email?"
        );
    }

    #[test]
    fn sub_labels_keep_their_own_descriptions() {
        let menu = vec![
            Choice::label("L1", "Education"),
            Choice::label("L2", "Education/High"),
            Choice::label("L3", "Financial"),
        ];
        let parsed = parse_prompt(
            "- Education -- unclear which child\n- Education/High -- general announcements\n- Finance -- bills",
            &menu,
        );
        let descriptions: Vec<_> = parsed.descriptions.iter().map(Option::as_deref).collect();
        assert_eq!(
            descriptions,
            [
                Some("unclear which child"),
                Some("general announcements"),
                Some("bills")
            ]
        );
    }

    #[test]
    fn reply_protocol_phrases_are_dropped() {
        let parsed = parse_prompt(
            "reply MATCH if this looks like an alert\nNO_MATCH if it does not fit",
            &[],
        );
        assert_eq!(parsed.instruction, "this looks like an alert");
        assert_eq!(
            none_text(parsed.none.as_deref()),
            "None of these: it does not fit"
        );
    }

    #[test]
    fn rules_without_a_model_decision_are_not_asked() {
        assert!(plan(&rule("  "), &[], &view(), &[]).is_none());
    }

    #[test]
    fn stored_replies_are_read_back_against_the_menu() {
        let menu = menu();
        let answer = llm_answer("matched", Some("Financial | Investment notice."), &menu).unwrap();
        assert_eq!(answer.chosen, ["\"Financial\""]);
        assert!(
            !llm_answer("no_match", Some("NO_MATCH"), &menu)
                .unwrap()
                .matched
        );
        assert!(llm_answer("condition_skipped", None, &menu).is_none());
    }

    #[test]
    fn verdicts_are_compared_with_the_llm() {
        let menu = menu();
        let plan = plan(&rule(""), &menu, &view(), &[]).unwrap();
        let asked = &plan.asked[0];
        let kind = RenderedKind::Choice {
            keys: vec!["c0".into(), "c1".into(), "c2".into(), "none".into()],
        };
        let logits = Logits {
            logits: vec![3.0, 0.0, 0.0, 0.0],
            null_logits: None,
            state_tokens: 10,
        };
        let llm = llm_answer("matched", Some("Financial"), &menu).unwrap();
        let v = verdict(asked, &kind, &logits, &Calibration::default(), Some(&llm));
        assert_eq!(v.choice.as_deref(), Some("\"Financial\""));
        assert_eq!((v.agrees, v.llm_option), (Some(true), Some(0)));
        assert!(v.confidence > 0.8);

        let declined = llm_answer("no_match", None, &menu).unwrap();
        let v = verdict(
            asked,
            &kind,
            &logits,
            &Calibration::default(),
            Some(&declined),
        );
        assert_eq!((v.agrees, v.llm_option), (Some(false), Some(3)));
    }
}
