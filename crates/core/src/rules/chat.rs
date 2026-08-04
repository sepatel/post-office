use serde::de::{DeserializeOwned, Deserializer};
use serde::{Deserialize, Serialize};

use crate::db::rule_chat::ChatMessageRow;
use crate::db::rules::UpdateRuleRequest;
use crate::db::Database;
use crate::llm::InferenceRouter;
use crate::rules::models::{Action, Condition};
use crate::rules::prompts::CHAT_SYSTEM_PROMPT;

/// A single learned fact proposed by the chat (and later persisted).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryInput {
    pub kind: String,
    pub text: String,
}

/// A structured change to a rule, proposed by the chat and applied on approval.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChatProposal {
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default, deserialize_with = "deserialize_lossy_actions")]
    pub actions_add: Vec<Action>,
    #[serde(default, deserialize_with = "deserialize_lossy_conditions")]
    pub conditions_add: Vec<Condition>,
    #[serde(default, deserialize_with = "deserialize_lossy_memories")]
    pub memories_add: Vec<MemoryInput>,
}

/// One round of chat: the assistant's reply plus its proposed change.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChatTurn {
    #[serde(default)]
    pub reply: String,
    #[serde(default)]
    pub proposal: ChatProposal,
}

#[derive(Debug, thiserror::Error)]
pub enum ChatError {
    #[error(transparent)]
    Database(#[from] rusqlite::Error),

    #[error(transparent)]
    Llm(#[from] crate::llm::LlmError),

    #[error("Rule not found")]
    RuleNotFound,

    #[error("Failed to parse chat response: {0}")]
    Parse(String),
}

/// Send a message to the rule's chat and get back a reply + proposed change.
/// Persists both sides of the transcript (the proposal is stored alongside the
/// assistant message for later review/apply). No rule state is mutated here.
pub async fn chat_with_rule(
    db: &Database,
    llm: &InferenceRouter,
    rule_id: i64,
    message: &str,
) -> Result<ChatTurn, ChatError> {
    let rule = db
        .with_rules(|repo| repo.get_by_id(rule_id))?
        .ok_or(ChatError::RuleNotFound)?;

    let memories = db.with_rule_memory(|repo| repo.list_for_rule(rule_id))?;
    let history = db.with_rule_chat(|repo| repo.list(rule_id))?;

    let user_content = build_chat_user_prompt(&rule, &memories, &history, message);

    let response: serde_json::Value = llm
        .chat_json(&rule.inference_policy, CHAT_SYSTEM_PROMPT, &user_content)
        .await?;

    let turn: ChatTurn =
        serde_json::from_value(response).map_err(|e| ChatError::Parse(e.to_string()))?;

    db.with_rule_chat(|repo| {
        repo.insert(rule_id, "user", message, None)?;
        let proposal_json = serde_json::to_string(&turn.proposal).ok();
        repo.insert(rule_id, "assistant", &turn.reply, proposal_json.as_deref())?;
        Ok::<(), rusqlite::Error>(())
    })?;

    Ok(turn)
}

/// Apply an approved proposal: persist new memories and merge prompt/actions/
/// conditions into the rule. Safe to call only after the user accepts.
pub async fn apply_proposal(
    db: &Database,
    rule_id: i64,
    proposal: &ChatProposal,
) -> Result<(), ChatError> {
    db.with_rule_memory(|repo| {
        for m in &proposal.memories_add {
            repo.insert(rule_id, &m.kind, &m.text, "chat")?;
        }
        Ok::<(), rusqlite::Error>(())
    })?;

    let mutates_rule = proposal.prompt.is_some()
        || !proposal.actions_add.is_empty()
        || !proposal.conditions_add.is_empty();

    if mutates_rule {
        let mut rule = db
            .with_rules(|repo| repo.get_by_id(rule_id))?
            .ok_or(ChatError::RuleNotFound)?;

        if let Some(prompt) = &proposal.prompt {
            rule.prompt = prompt.clone();
        }
        rule.actions.extend(proposal.actions_add.clone());
        rule.conditions.extend(proposal.conditions_add.clone());

        let update = UpdateRuleRequest {
            name: rule.name,
            description: rule.description,
            conditions: rule.conditions,
            prompt: rule.prompt,
            actions: rule.actions,
            priority: rule.priority,
            enabled: rule.enabled,
            inference_policy: rule.inference_policy,
        };
        db.with_rules(|repo| repo.update(rule.id, &update))?;
    }

    Ok(())
}

/// Recent transcript for a rule (for rendering the chat UI).
pub fn chat_history(db: &Database, rule_id: i64) -> Result<Vec<ChatMessageRow>, ChatError> {
    Ok(db.with_rule_chat(|repo| repo.list(rule_id))?)
}

fn build_chat_user_prompt(
    rule: &crate::rules::models::Rule,
    memories: &[crate::db::rule_memory::MemoryEntry],
    history: &[ChatMessageRow],
    message: &str,
) -> String {
    let actions = rule
        .actions
        .iter()
        .map(|a| serde_json::to_string(a).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");

    let conditions = rule
        .conditions
        .iter()
        .map(|c| serde_json::to_string(c).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");

    let memory_block = if memories.is_empty() {
        "(none)".to_string()
    } else {
        memories
            .iter()
            .map(|m| format!("- [{}] {}", m.kind, m.text))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let history_block = if history.is_empty() {
        "(none)".to_string()
    } else {
        history
            .iter()
            .map(|h| format!("{}: {}", h.role, h.content))
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        "CURRENT RULE STATE:\n\
         name: {}\n\
         prompt: {}\n\
         actions:\n{}\n\
         conditions:\n{}\n\
         learned memory:\n{}\n\n\
         RECENT CHAT:\n{}\n\n\
         USER MESSAGE:\n{}\n\n\
         Respond with the JSON proposal described in your instructions.",
        rule.name, rule.prompt, actions, conditions, memory_block, history_block, message,
    )
}

fn deserialize_lossy_actions<'de, D>(deserializer: D) -> Result<Vec<Action>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_lossy_vec(deserializer)
}

fn deserialize_lossy_conditions<'de, D>(deserializer: D) -> Result<Vec<Condition>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_lossy_vec(deserializer)
}

fn deserialize_lossy_memories<'de, D>(deserializer: D) -> Result<Vec<MemoryInput>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_lossy_vec(deserializer)
}

fn deserialize_lossy_vec<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let raw = Option::<Vec<serde_json::Value>>::deserialize(deserializer)?.unwrap_or_default();
    Ok(raw
        .into_iter()
        .filter_map(|item| serde_json::from_value::<T>(item).ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_turn_skips_malformed_action_entries() {
        let raw = serde_json::json!({
            "reply": "Updated.",
            "proposal": {
                "prompt": null,
                "actions_add": [
                    { "type": "archive" },
                    { "value": "MissingType" }
                ],
                "conditions_add": [],
                "memories_add": []
            }
        });

        let turn: ChatTurn = serde_json::from_value(raw).unwrap();
        assert_eq!(turn.reply, "Updated.");
        assert_eq!(turn.proposal.actions_add.len(), 1);
    }

    #[test]
    fn chat_turn_skips_malformed_condition_entries() {
        let raw = serde_json::json!({
            "reply": "Noted.",
            "proposal": {
                "prompt": null,
                "actions_add": [],
                "conditions_add": [
                    { "type": "from", "operator": "contains", "value": "boss" },
                    { "operator": "contains", "value": "missing type" }
                ],
                "memories_add": []
            }
        });

        let turn: ChatTurn = serde_json::from_value(raw).unwrap();
        assert_eq!(turn.proposal.conditions_add.len(), 1);
    }
}
