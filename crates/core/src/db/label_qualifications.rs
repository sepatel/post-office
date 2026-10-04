use rusqlite::{params, Connection, OptionalExtension, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelQualification {
    pub account_email: String,
    pub label_id: String,
    pub description: String,
    pub examples: Vec<String>,
    pub negative_examples: Vec<String>,
    pub source: String,
    pub updated_at: String,
}

pub struct LabelQualificationRepository<'a> {
    conn: &'a Connection,
}

impl<'a> LabelQualificationRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn get(&self, account_email: &str, label_id: &str) -> Result<Option<LabelQualification>> {
        self.conn
            .query_row(
                "SELECT account_email, label_id, description, examples, negative_examples, source, updated_at
                 FROM label_qualifications WHERE account_email = ?1 AND label_id = ?2",
                params![account_email, label_id],
                map_qualification,
            )
            .optional()
    }

    pub fn list_for_account(&self, account_email: &str) -> Result<Vec<LabelQualification>> {
        let mut stmt = self.conn.prepare(
            "SELECT account_email, label_id, description, examples, negative_examples, source, updated_at
             FROM label_qualifications WHERE account_email = ?1 ORDER BY label_id",
        )?;
        let rows = stmt
            .query_map(params![account_email], map_qualification)?
            .collect();
        rows
    }

    pub fn upsert(
        &self,
        account_email: &str,
        label_id: &str,
        qualification: &LabelQualification,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO label_qualifications(account_email, label_id, description, examples, negative_examples, source, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'))
             ON CONFLICT(account_email, label_id) DO UPDATE SET
                description = excluded.description,
                examples = excluded.examples,
                negative_examples = excluded.negative_examples,
                source = excluded.source,
                updated_at = datetime('now')",
            params![
                account_email,
                label_id,
                qualification.description,
                serde_json::to_string(&qualification.examples).unwrap_or_else(|_| "[]".into()),
                serde_json::to_string(&qualification.negative_examples)
                    .unwrap_or_else(|_| "[]".into()),
                qualification.source,
            ],
        )?;
        Ok(())
    }

    pub fn delete(&self, account_email: &str, label_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM label_qualifications WHERE account_email = ?1 AND label_id = ?2",
            params![account_email, label_id],
        )?;
        Ok(())
    }
}

fn map_qualification(row: &rusqlite::Row<'_>) -> Result<LabelQualification> {
    Ok(LabelQualification {
        account_email: row.get(0)?,
        label_id: row.get(1)?,
        description: row.get(2)?,
        examples: serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or_default(),
        negative_examples: serde_json::from_str(&row.get::<_, String>(4)?).unwrap_or_default(),
        source: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use std::path::Path;

    fn qualification() -> LabelQualification {
        LabelQualification {
            account_email: "a@example.com".into(),
            label_id: "Label_1".into(),
            description: "Bills from vendors".into(),
            examples: vec!["invoice".into(), "receipt".into()],
            negative_examples: vec!["newsletter".into()],
            source: "user".into(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn qualifications_roundtrip_per_account() {
        let db = Database::open(Path::new(":memory:")).unwrap();
        db.migrate().unwrap();

        db.with_label_qualifications(|repo| {
            repo.upsert("a@example.com", "Label_1", &qualification())
        })
        .unwrap();
        db.with_label_qualifications(|repo| {
            repo.upsert(
                "b@example.com",
                "Label_1",
                &LabelQualification {
                    account_email: "b@example.com".into(),
                    description: "Other".into(),
                    ..qualification()
                },
            )
        })
        .unwrap();

        let stored = db
            .with_label_qualifications(|repo| repo.get("a@example.com", "Label_1"))
            .unwrap()
            .unwrap();
        assert_eq!(stored.description, "Bills from vendors");
        assert_eq!(stored.examples.len(), 2);
        assert_eq!(stored.negative_examples, vec!["newsletter".to_string()]);
        assert_eq!(stored.source, "user");
        assert!(crate::db::parse_stored_utc(&stored.updated_at).is_some());

        assert_eq!(
            db.with_label_qualifications(|repo| repo.list_for_account("a@example.com"))
                .unwrap()
                .len(),
            1
        );

        db.with_label_qualifications(|repo| repo.delete("a@example.com", "Label_1"))
            .unwrap();
        assert!(db
            .with_label_qualifications(|repo| repo.get("a@example.com", "Label_1"))
            .unwrap()
            .is_none());
    }

    #[test]
    fn removing_an_account_drops_its_qualifications() {
        let db = Database::open(Path::new(":memory:")).unwrap();
        db.migrate().unwrap();
        db.with_accounts(|repo| repo.add("a@example.com")).unwrap();
        db.with_label_qualifications(|repo| {
            repo.upsert("a@example.com", "Label_1", &qualification())
        })
        .unwrap();

        db.with_accounts(|repo| repo.delete_with_data("a@example.com"))
            .unwrap();

        assert!(db
            .with_label_qualifications(|repo| repo.get("a@example.com", "Label_1"))
            .unwrap()
            .is_none());
    }
}
