use rusqlite::{params, Connection, OptionalExtension, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleLabelOverride {
    pub account_email: String,
    pub rule_id: i64,
    pub label_id: String,
    pub description: String,
    pub examples: Vec<String>,
    pub negative_examples: Vec<String>,
    pub updated_at: String,
}

pub struct RuleLabelOverrideRepository<'a> {
    conn: &'a Connection,
}

impl<'a> RuleLabelOverrideRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn get(&self, rule_id: i64, label_id: &str) -> Result<Option<RuleLabelOverride>> {
        self.conn
            .query_row(
                "SELECT account_email, rule_id, label_id, description, examples, negative_examples, updated_at
                 FROM rule_label_overrides WHERE rule_id = ?1 AND label_id = ?2",
                params![rule_id, label_id],
                map_override,
            )
            .optional()
    }

    pub fn list_for_rule(&self, rule_id: i64) -> Result<Vec<RuleLabelOverride>> {
        let mut stmt = self.conn.prepare(
            "SELECT account_email, rule_id, label_id, description, examples, negative_examples, updated_at
             FROM rule_label_overrides WHERE rule_id = ?1 ORDER BY label_id",
        )?;
        let rows = stmt.query_map(params![rule_id], map_override)?.collect();
        rows
    }

    pub fn list_for_account(&self, account_email: &str) -> Result<Vec<RuleLabelOverride>> {
        let mut stmt = self.conn.prepare(
            "SELECT account_email, rule_id, label_id, description, examples, negative_examples, updated_at
             FROM rule_label_overrides WHERE account_email = ?1 ORDER BY rule_id, label_id",
        )?;
        let rows = stmt
            .query_map(params![account_email], map_override)?
            .collect();
        rows
    }

    pub fn upsert(&self, override_row: &RuleLabelOverride) -> Result<()> {
        self.conn.execute(
            "INSERT INTO rule_label_overrides(account_email, rule_id, label_id, description, examples, negative_examples, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'))
             ON CONFLICT(rule_id, label_id) DO UPDATE SET
                account_email = excluded.account_email,
                description = excluded.description,
                examples = excluded.examples,
                negative_examples = excluded.negative_examples,
                updated_at = datetime('now')",
            params![
                override_row.account_email,
                override_row.rule_id,
                override_row.label_id,
                override_row.description,
                serde_json::to_string(&override_row.examples).unwrap_or_else(|_| "[]".into()),
                serde_json::to_string(&override_row.negative_examples)
                    .unwrap_or_else(|_| "[]".into()),
            ],
        )?;
        Ok(())
    }

    pub fn delete(&self, rule_id: i64, label_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM rule_label_overrides WHERE rule_id = ?1 AND label_id = ?2",
            params![rule_id, label_id],
        )?;
        Ok(())
    }

    pub fn delete_for_rule(&self, rule_id: i64) -> Result<()> {
        self.conn.execute(
            "DELETE FROM rule_label_overrides WHERE rule_id = ?1",
            params![rule_id],
        )?;
        Ok(())
    }
}

fn map_override(row: &rusqlite::Row<'_>) -> Result<RuleLabelOverride> {
    Ok(RuleLabelOverride {
        account_email: row.get(0)?,
        rule_id: row.get(1)?,
        label_id: row.get(2)?,
        description: row.get(3)?,
        examples: serde_json::from_str(&row.get::<_, String>(4)?).unwrap_or_default(),
        negative_examples: serde_json::from_str(&row.get::<_, String>(5)?).unwrap_or_default(),
        updated_at: row.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::db::rules::CreateRuleRequest;
    use std::path::Path;

    fn rule_request() -> CreateRuleRequest {
        CreateRuleRequest {
            name: "t".into(),
            description: None,
            conditions: vec![],
            prompt: String::new(),
            choices: vec![],
            choose_from_all_labels: false,
            actions: vec![],
            priority: 0,
            enabled: true,
            inference_policy: "default".into(),
            decision_reasoning_effort: crate::llm::ReasoningEffort::Off,
            decision_max_tokens: None,
            continue_after_match: false,
            match_mode: crate::rules::models::MatchMode::Single,
        }
    }

    fn override_row(rule_id: i64) -> RuleLabelOverride {
        RuleLabelOverride {
            account_email: "a@example.com".into(),
            rule_id,
            label_id: "Label_1".into(),
            description: "Only bills for this rule".into(),
            examples: vec!["power bill".into()],
            negative_examples: vec!["newsletter".into()],
            updated_at: String::new(),
        }
    }

    #[test]
    fn overrides_roundtrip_per_rule() {
        let db = Database::open(Path::new(":memory:")).unwrap();
        db.migrate().unwrap();
        let rule = db
            .with_rules(|repo| repo.create("a@example.com", &rule_request()))
            .unwrap();

        db.with_rule_label_overrides(|repo| repo.upsert(&override_row(rule.id)))
            .unwrap();

        let stored = db
            .with_rule_label_overrides(|repo| repo.get(rule.id, "Label_1"))
            .unwrap()
            .unwrap();
        assert_eq!(stored.description, "Only bills for this rule");
        assert_eq!(stored.examples, vec!["power bill".to_string()]);
        assert_eq!(
            db.with_rule_label_overrides(|repo| repo.list_for_rule(rule.id))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            db.with_rule_label_overrides(|repo| repo.list_for_account("a@example.com"))
                .unwrap()
                .len(),
            1
        );

        db.with_rule_label_overrides(|repo| repo.delete(rule.id, "Label_1"))
            .unwrap();
        assert!(db
            .with_rule_label_overrides(|repo| repo.get(rule.id, "Label_1"))
            .unwrap()
            .is_none());
    }

    #[test]
    fn deleting_a_rule_drops_its_overrides() {
        let db = Database::open(Path::new(":memory:")).unwrap();
        db.migrate().unwrap();
        let rule = db
            .with_rules(|repo| repo.create("a@example.com", &rule_request()))
            .unwrap();
        db.with_rule_label_overrides(|repo| repo.upsert(&override_row(rule.id)))
            .unwrap();

        db.with_rules(|repo| repo.delete("a@example.com", rule.id))
            .unwrap();

        assert!(db
            .with_rule_label_overrides(|repo| repo.list_for_rule(rule.id))
            .unwrap()
            .is_empty());
    }
}
