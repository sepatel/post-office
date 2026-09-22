use rusqlite::{params, Connection, Result, Row, ToSql};

use crate::rules::models::Rule;

const RULE_COLUMNS: &str = "r.id, r.name, r.description, r.conditions, r.prompt, r.choices, r.choose_from_all_labels,
     r.actions, r.priority, r.enabled, r.parent_id, COALESCE(p.policy_id, 'default'), r.decision_reasoning_effort,
     r.decision_max_tokens, r.continue_after_match";

const RULE_FROM: &str = "FROM rules r LEFT JOIN rule_inference_policy p ON p.rule_id = r.id";

fn map_rule_row(row: &Row<'_>) -> Result<Rule> {
    Ok(Rule {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        conditions: serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or_default(),
        prompt: row.get(4)?,
        choices: serde_json::from_str(&row.get::<_, String>(5)?).unwrap_or_default(),
        choose_from_all_labels: row.get::<_, i32>(6)? != 0,
        actions: serde_json::from_str(&row.get::<_, String>(7)?).unwrap_or_default(),
        priority: row.get(8)?,
        enabled: row.get::<_, i32>(9)? != 0,
        parent_id: row.get(10)?,
        inference_policy: row.get(11)?,
        decision_reasoning_effort: serde_json::from_str(&format!(
            "\"{}\"",
            row.get::<_, String>(12)?
        ))
        .unwrap_or(crate::llm::ReasoningEffort::Off),
        decision_max_tokens: row.get(13)?,
        continue_after_match: row.get::<_, i32>(14)? != 0,
    })
}

pub struct RuleRepository<'a> {
    conn: &'a Connection,
}

impl<'a> RuleRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn list_all(&self, account_email: &str) -> Result<Vec<Rule>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {RULE_COLUMNS} {RULE_FROM}
             WHERE r.account_email = ?1
             ORDER BY priority ASC"
        ))?;

        let rules = stmt
            .query_map(params![account_email], map_rule_row)?
            .collect();
        rules
    }

    pub fn get_enabled_rules(&self, account_email: &str) -> Result<Vec<Rule>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {RULE_COLUMNS} {RULE_FROM}
             WHERE r.account_email = ?1 AND r.enabled = 1
             ORDER BY priority ASC"
        ))?;

        let rules = stmt
            .query_map(params![account_email], map_rule_row)?
            .collect();
        rules
    }

    /// Load only the rules whose ids are in `ids`, preserving the requested
    /// order. Used by one-off backfills that apply a selected subset of rules.
    pub fn get_by_ids(&self, account_email: &str, ids: &[i64]) -> Result<Vec<Rule>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {RULE_COLUMNS} {RULE_FROM}
             WHERE r.account_email = ?1 AND r.id IN ({placeholders})
             ORDER BY r.priority ASC"
        ))?;
        let mut values: Vec<&dyn ToSql> = Vec::with_capacity(ids.len() + 1);
        values.push(&account_email);
        for id in ids {
            values.push(id);
        }
        let rules = stmt.query_map(&*values, map_rule_row)?.collect();
        rules
    }

    pub fn reorder(&self, account_email: &str, ids: &[i64]) -> Result<()> {
        for (priority, id) in ids.iter().enumerate() {
            self.conn.execute(
                "UPDATE rules SET priority = ?1, updated_at = datetime('now')
                  WHERE id = ?2 AND account_email = ?3",
                params![priority as i32, id, account_email],
            )?;
        }
        Ok(())
    }

    pub fn create(&self, account_email: &str, rule: &CreateRuleRequest) -> Result<Rule> {
        self.conn.execute(
            "INSERT INTO rules (account_email, name, description, conditions, prompt, choices,
                                choose_from_all_labels, actions, priority, enabled, decision_reasoning_effort,
                                decision_max_tokens, continue_after_match)
              VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                account_email,
                rule.name,
                rule.description,
                serde_json::to_string(&rule.conditions).unwrap_or_default(),
                rule.prompt,
                serde_json::to_string(&rule.choices).unwrap_or_default(),
                rule.choose_from_all_labels as i32,
                serde_json::to_string(&rule.actions).unwrap_or_default(),
                rule.priority,
                rule.enabled as i32,
                rule.decision_reasoning_effort.config_value(),
                rule.decision_max_tokens,
                rule.continue_after_match as i32,
            ],
        )?;

        let id = self.conn.last_insert_rowid();

        self.conn.execute(
            "INSERT INTO rule_inference_policy (rule_id, policy_id) VALUES (?1, ?2)",
            params![id, rule.inference_policy],
        )?;

        Ok(Rule {
            id,
            name: rule.name.clone(),
            description: rule.description.clone(),
            conditions: rule.conditions.clone(),
            prompt: rule.prompt.clone(),
            choices: rule.choices.clone(),
            choose_from_all_labels: rule.choose_from_all_labels,
            actions: rule.actions.clone(),
            priority: rule.priority,
            enabled: rule.enabled,
            parent_id: None,
            inference_policy: rule.inference_policy.clone(),
            decision_reasoning_effort: rule.decision_reasoning_effort,
            decision_max_tokens: rule.decision_max_tokens,
            continue_after_match: rule.continue_after_match,
        })
    }

    pub fn update(&self, account_email: &str, id: i64, rule: &UpdateRuleRequest) -> Result<Rule> {
        self.conn.execute(
            "UPDATE rules SET name = ?1, description = ?2, conditions = ?3, prompt = ?4,
             choices = ?5, choose_from_all_labels = ?6, actions = ?7, priority = ?8, enabled = ?9,
             decision_reasoning_effort = ?10, decision_max_tokens = ?11, continue_after_match = ?12,
             updated_at = datetime('now') WHERE id = ?13 AND account_email = ?14",
            params![
                rule.name,
                rule.description,
                serde_json::to_string(&rule.conditions).unwrap_or_default(),
                rule.prompt,
                serde_json::to_string(&rule.choices).unwrap_or_default(),
                rule.choose_from_all_labels as i32,
                serde_json::to_string(&rule.actions).unwrap_or_default(),
                rule.priority,
                rule.enabled as i32,
                rule.decision_reasoning_effort.config_value(),
                rule.decision_max_tokens,
                rule.continue_after_match as i32,
                id,
                account_email,
            ],
        )?;

        self.conn.execute(
            "INSERT INTO rule_inference_policy (rule_id, policy_id, updated_at) VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(rule_id) DO UPDATE SET policy_id = excluded.policy_id, updated_at = datetime('now')",
            params![id, rule.inference_policy],
        )?;

        self.get_by_id(account_email, id)?
            .ok_or_else(|| rusqlite::Error::QueryReturnedNoRows)
    }

    pub fn delete(&self, account_email: &str, id: i64) -> Result<()> {
        self.conn.execute(
            "DELETE FROM rules WHERE id = ?1 AND account_email = ?2",
            params![id, account_email],
        )?;
        Ok(())
    }

    pub fn get_by_id(&self, account_email: &str, id: i64) -> Result<Option<Rule>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {RULE_COLUMNS} {RULE_FROM}
             WHERE r.id = ?1 AND r.account_email = ?2"
        ))?;

        let rule = stmt
            .query_map(params![id, account_email], map_rule_row)?
            .next()
            .transpose();
        rule
    }
}

#[derive(Debug, Clone)]
pub struct CreateRuleRequest {
    pub name: String,
    pub description: Option<String>,
    pub conditions: Vec<crate::rules::models::Condition>,
    pub prompt: String,
    pub choices: Vec<crate::rules::models::Action>,
    pub choose_from_all_labels: bool,
    pub actions: Vec<crate::rules::models::Action>,
    pub priority: i32,
    pub enabled: bool,
    pub inference_policy: String,
    pub decision_reasoning_effort: crate::llm::ReasoningEffort,
    pub decision_max_tokens: Option<u32>,
    pub continue_after_match: bool,
}

#[derive(Debug, Clone)]
pub struct UpdateRuleRequest {
    pub name: String,
    pub description: Option<String>,
    pub conditions: Vec<crate::rules::models::Condition>,
    pub prompt: String,
    pub choices: Vec<crate::rules::models::Action>,
    pub choose_from_all_labels: bool,
    pub actions: Vec<crate::rules::models::Action>,
    pub priority: i32,
    pub enabled: bool,
    pub inference_policy: String,
    pub decision_reasoning_effort: crate::llm::ReasoningEffort,
    pub decision_max_tokens: Option<u32>,
    pub continue_after_match: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::rules::models::{Action, Condition, Operator};
    use std::path::Path;

    #[test]
    fn the_menu_and_the_recipe_roundtrip_separately() {
        let db = Database::open(Path::new(":memory:")).unwrap();
        db.migrate().unwrap();

        let mut req = sample_request();
        req.continue_after_match = true;
        req.choices = vec![Action::Trash];
        req.decision_reasoning_effort = crate::llm::ReasoningEffort::Low;
        req.decision_max_tokens = Some(4_096);
        let created = db
            .with_rules(|repo| repo.create("test@example.com", &req))
            .unwrap();
        assert!(created.continue_after_match);
        let stored = db
            .with_rules(|repo| repo.get_by_id("test@example.com", created.id))
            .unwrap()
            .unwrap();
        assert!(stored.continue_after_match);
        assert_eq!(
            stored.decision_reasoning_effort,
            crate::llm::ReasoningEffort::Low
        );
        assert_eq!(stored.decision_max_tokens, Some(4_096));
        assert_eq!(stored.choices.len(), 1);
        assert_eq!(stored.actions.len(), 1);

        let update = UpdateRuleRequest {
            name: req.name.clone(),
            description: None,
            conditions: vec![],
            prompt: req.prompt.clone(),
            choices: vec![],
            choose_from_all_labels: true,
            actions: vec![],
            priority: 0,
            enabled: true,
            inference_policy: "default".into(),
            decision_reasoning_effort: crate::llm::ReasoningEffort::Medium,
            decision_max_tokens: None,
            continue_after_match: false,
        };
        let updated = db
            .with_rules(|repo| repo.update("test@example.com", created.id, &update))
            .unwrap();
        assert!(!updated.continue_after_match);
        assert!(updated.choose_from_all_labels);
        assert!(updated.choices.is_empty());
        assert_eq!(
            updated.decision_reasoning_effort,
            crate::llm::ReasoningEffort::Medium
        );
        assert_eq!(updated.decision_max_tokens, None);
    }

    fn sample_request() -> CreateRuleRequest {
        CreateRuleRequest {
            name: "t".into(),
            description: None,
            conditions: vec![
                Condition::From {
                    operator: Operator::Contains,
                    value: "mongo".into(),
                },
                Condition::Subject {
                    operator: Operator::NotContains,
                    value: "alert".into(),
                },
            ],
            prompt: "do something".into(),
            choices: vec![],
            choose_from_all_labels: false,
            actions: vec![Action::Label {
                value: "Mongo".into(),
            }],
            priority: 0,
            enabled: true,
            inference_policy: "default".into(),
            decision_reasoning_effort: crate::llm::ReasoningEffort::ServerDefault,
            decision_max_tokens: None,
            continue_after_match: false,
        }
    }

    #[test]
    fn conditions_roundtrip_through_db() {
        let req = sample_request();
        let db = Database::open(Path::new(":memory:")).unwrap();
        db.migrate().unwrap();
        let _created = db
            .with_rules(|repo| repo.create("test@example.com", &req))
            .unwrap();
        let _other = db
            .with_rules(|repo| repo.create("other@example.com", &req))
            .unwrap();
        let all = db
            .with_rules(|repo| repo.list_all("test@example.com"))
            .unwrap();

        assert_eq!(all.len(), 1);
        assert_eq!(all[0].conditions.len(), 2);
        assert_eq!(all[0].actions.len(), 1);
        let conditions_json = serde_json::to_string(&all[0].conditions).unwrap();
        assert!(conditions_json.contains("mongo"));
        let actions_json = serde_json::to_string(&all[0].actions).unwrap();
        assert!(actions_json.contains("Mongo"));
    }
}
