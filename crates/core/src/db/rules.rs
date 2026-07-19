use rusqlite::{params, Connection, Result};

use crate::rules::models::Rule;

pub struct RuleRepository<'a> {
    conn: &'a Connection,
}

impl<'a> RuleRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn list_all(&self) -> Result<Vec<Rule>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, description, conditions, prompt, actions, priority, enabled, parent_id
             FROM rules
             ORDER BY priority ASC",
        )?;

        let rules = stmt
            .query_map([], |row| {
                Ok(Rule {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                    conditions: serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or_default(),
                    prompt: row.get(4)?,
                    actions: serde_json::from_str(&row.get::<_, String>(5)?).unwrap_or_default(),
                    priority: row.get(6)?,
                    enabled: row.get::<_, i32>(7)? != 0,
                    parent_id: row.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>>>()?;

        Ok(rules)
    }

    pub fn get_enabled_rules(&self) -> Result<Vec<Rule>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, description, conditions, prompt, actions, priority, enabled, parent_id
             FROM rules
             WHERE enabled = 1
             ORDER BY priority ASC",
        )?;

        let rules = stmt
            .query_map([], |row| {
                Ok(Rule {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                    conditions: serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or_default(),
                    prompt: row.get(4)?,
                    actions: serde_json::from_str(&row.get::<_, String>(5)?).unwrap_or_default(),
                    priority: row.get(6)?,
                    enabled: row.get::<_, i32>(7)? != 0,
                    parent_id: row.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>>>()?;

        Ok(rules)
    }

    /// Load only the rules whose ids are in `ids`, preserving the requested
    /// order. Used by one-off backfills that apply a selected subset of rules.
    pub fn get_by_ids(&self, ids: &[i64]) -> Result<Vec<Rule>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = ids
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT id, name, description, conditions, prompt, actions, priority, enabled, parent_id
             FROM rules
             WHERE id IN ({placeholders})
             ORDER BY priority ASC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mapped = stmt.query_map(rusqlite::params_from_iter(ids.iter().copied()), |row| {
            Ok(Rule {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                conditions: serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or_default(),
                prompt: row.get(4)?,
                actions: serde_json::from_str(&row.get::<_, String>(5)?).unwrap_or_default(),
                priority: row.get(6)?,
                enabled: row.get::<_, i32>(7)? != 0,
                parent_id: row.get(8)?,
            })
        })?;
        let rules = mapped.collect::<Result<Vec<_>>>()?;

        Ok(rules)
    }

    pub fn create(&self, rule: &CreateRuleRequest) -> Result<Rule> {
        self.conn.execute(
            "INSERT INTO rules (name, description, conditions, prompt, actions, priority, enabled)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                rule.name,
                rule.description,
                serde_json::to_string(&rule.conditions).unwrap_or_default(),
                rule.prompt,
                serde_json::to_string(&rule.actions).unwrap_or_default(),
                rule.priority,
                rule.enabled as i32,
            ],
        )?;

        let id = self.conn.last_insert_rowid();

        Ok(Rule {
            id,
            name: rule.name.clone(),
            description: rule.description.clone(),
            conditions: rule.conditions.clone(),
            prompt: rule.prompt.clone(),
            actions: rule.actions.clone(),
            priority: rule.priority,
            enabled: rule.enabled,
            parent_id: None,
        })
    }

    pub fn update(&self, id: i64, rule: &UpdateRuleRequest) -> Result<Rule> {
        self.conn.execute(
            "UPDATE rules SET name = ?1, description = ?2, conditions = ?3, prompt = ?4,
             actions = ?5, priority = ?6, enabled = ?7, updated_at = datetime('now')
             WHERE id = ?8",
            params![
                rule.name,
                rule.description,
                serde_json::to_string(&rule.conditions).unwrap_or_default(),
                rule.prompt,
                serde_json::to_string(&rule.actions).unwrap_or_default(),
                rule.priority,
                rule.enabled as i32,
                id,
            ],
        )?;

        self.get_by_id(id)?
            .ok_or_else(|| rusqlite::Error::QueryReturnedNoRows)
    }

    pub fn delete(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM rules WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn get_by_id(&self, id: i64) -> Result<Option<Rule>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, description, conditions, prompt, actions, priority, enabled, parent_id
             FROM rules WHERE id = ?1",
        )?;

        let mut rules = stmt.query_map(params![id], |row| {
            Ok(Rule {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                conditions: serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or_default(),
                prompt: row.get(4)?,
                actions: serde_json::from_str(&row.get::<_, String>(5)?).unwrap_or_default(),
                priority: row.get(6)?,
                enabled: row.get::<_, i32>(7)? != 0,
                parent_id: row.get(8)?,
            })
        })?;

        rules.next().transpose()
    }
}

#[derive(Debug, Clone)]
pub struct CreateRuleRequest {
    pub name: String,
    pub description: Option<String>,
    pub conditions: Vec<crate::rules::models::Condition>,
    pub prompt: String,
    pub actions: Vec<crate::rules::models::Action>,
    pub priority: i32,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub struct UpdateRuleRequest {
    pub name: String,
    pub description: Option<String>,
    pub conditions: Vec<crate::rules::models::Condition>,
    pub prompt: String,
    pub actions: Vec<crate::rules::models::Action>,
    pub priority: i32,
    pub enabled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::rules::models::{Action, Condition, Operator};
    use std::path::Path;

    #[test]
    fn conditions_roundtrip_through_db() {
        let req = CreateRuleRequest {
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
            actions: vec![Action::Label {
                value: "Mongo".into(),
            }],
            priority: 0,
            enabled: true,
        };

        let db = Database::open(Path::new(":memory:")).unwrap();
        db.migrate().unwrap();
        let _created = db.with_rules(|repo| repo.create(&req)).unwrap();
        let all = db.with_rules(|repo| repo.list_all()).unwrap();

        assert_eq!(all.len(), 1);
        assert_eq!(all[0].conditions.len(), 2);
        assert_eq!(all[0].actions.len(), 1);
        let conditions_json = serde_json::to_string(&all[0].conditions).unwrap();
        assert!(conditions_json.contains("mongo"));
        let actions_json = serde_json::to_string(&all[0].actions).unwrap();
        assert!(actions_json.contains("Mongo"));
    }
}
