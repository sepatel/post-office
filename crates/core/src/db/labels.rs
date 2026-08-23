use rusqlite::{params, Connection, OptionalExtension, Result};

use crate::gmail::models::Label;

pub struct LabelRepository<'a> {
    conn: &'a Connection,
}

pub struct CachedLabels {
    pub labels: Vec<Label>,
    pub fetched_at: String,
}

impl<'a> LabelRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn get(&self, account_email: &str) -> Result<Option<CachedLabels>> {
        self.conn
            .query_row(
                "SELECT labels, fetched_at FROM gmail_labels WHERE account_email = ?1",
                params![account_email],
                |row| {
                    Ok(CachedLabels {
                        labels: serde_json::from_str(&row.get::<_, String>(0)?).unwrap_or_default(),
                        fetched_at: row.get(1)?,
                    })
                },
            )
            .optional()
    }

    pub fn upsert(&self, account_email: &str, labels: &[Label]) -> Result<()> {
        self.conn.execute(
            "INSERT INTO gmail_labels(account_email, labels, fetched_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(account_email) DO UPDATE SET
                labels = excluded.labels,
                fetched_at = datetime('now')",
            params![
                account_email,
                serde_json::to_string(labels).unwrap_or_else(|_| "[]".into())
            ],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use std::path::Path;

    fn label(id: &str, name: &str) -> Label {
        Label {
            id: id.into(),
            name: name.into(),
            label_type: "user".into(),
            message_list_visibility: None,
            label_list_visibility: None,
        }
    }

    #[test]
    fn labels_roundtrip_per_account() {
        let db = Database::open(Path::new(":memory:")).unwrap();
        db.migrate().unwrap();

        assert!(db
            .with_labels(|repo| repo.get("a@example.com"))
            .unwrap()
            .is_none());

        db.with_labels(|repo| repo.upsert("a@example.com", &[label("Label_1", "Finance")]))
            .unwrap();
        db.with_labels(|repo| repo.upsert("b@example.com", &[label("Label_1", "Other")]))
            .unwrap();

        let cached = db
            .with_labels(|repo| repo.get("a@example.com"))
            .unwrap()
            .unwrap();
        assert_eq!(cached.labels.len(), 1);
        assert_eq!(cached.labels[0].name, "Finance");
        assert!(crate::db::parse_stored_utc(&cached.fetched_at).is_some());

        db.with_labels(|repo| {
            repo.upsert(
                "a@example.com",
                &[label("Label_1", "Finance"), label("Label_2", "Receipts")],
            )
        })
        .unwrap();
        assert_eq!(
            db.with_labels(|repo| repo.get("a@example.com"))
                .unwrap()
                .unwrap()
                .labels
                .len(),
            2
        );
        assert_eq!(
            db.with_labels(|repo| repo.get("b@example.com"))
                .unwrap()
                .unwrap()
                .labels[0]
                .name,
            "Other"
        );
    }

    #[test]
    fn removing_an_account_drops_its_cached_labels() {
        let db = Database::open(Path::new(":memory:")).unwrap();
        db.migrate().unwrap();
        db.with_accounts(|repo| repo.add("a@example.com")).unwrap();
        db.with_labels(|repo| repo.upsert("a@example.com", &[label("Label_1", "Finance")]))
            .unwrap();

        db.with_accounts(|repo| repo.delete_with_data("a@example.com"))
            .unwrap();

        assert!(db
            .with_labels(|repo| repo.get("a@example.com"))
            .unwrap()
            .is_none());
    }
}
