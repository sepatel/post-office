use super::response_parser::ParsedAction;
use crate::gmail::models::Label;
use crate::gmail::GmailClient;

pub async fn execute_action(
    gmail: &mut GmailClient,
    email_id: &str,
    action: &ParsedAction,
    existing_labels: &[Label],
) -> Result<(), crate::gmail::GmailError> {
    let (add, remove) = match action {
        ParsedAction::Label(value) => {
            let label_id = resolve_existing_label_id(value, existing_labels)?;
            (vec![label_id], vec![])
        }
        ParsedAction::Archive => (vec![], vec!["INBOX".into()]),
        ParsedAction::Trash => (vec!["TRASH".into()], vec!["INBOX".into()]),
        ParsedAction::Spam => (vec!["SPAM".into()], vec!["INBOX".into()]),
        ParsedAction::MarkRead => (vec![], vec!["UNREAD".into()]),
        ParsedAction::MarkUnread => (vec!["UNREAD".into()], vec![]),
        ParsedAction::Star => (vec!["STARRED".into()], vec![]),
        ParsedAction::Apply => (vec![], vec![]),
    };

    if add.is_empty() && remove.is_empty() {
        return Ok(());
    }

    let add_refs: Vec<&str> = add.iter().map(String::as_str).collect();
    let remove_refs: Vec<&str> = remove.iter().map(String::as_str).collect();
    gmail
        .modify_labels(email_id, &add_refs, &remove_refs)
        .await?;

    Ok(())
}

fn resolve_existing_label_id(
    value: &str,
    existing_labels: &[Label],
) -> Result<String, crate::gmail::GmailError> {
    let trimmed = value.trim();

    if let Some(existing) = existing_labels.iter().find(|l| l.id == trimmed) {
        return Ok(existing.id.clone());
    }

    if let Some(existing) = existing_labels
        .iter()
        .find(|l| l.name.eq_ignore_ascii_case(trimmed))
    {
        return Ok(existing.id.clone());
    }

    Err(crate::gmail::GmailError::Auth(format!(
        "Unknown label: {}",
        value
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn resolves_by_id() {
        let labels = vec![label("Label_1", "Invoices")];
        let resolved = resolve_existing_label_id("Label_1", &labels).unwrap();
        assert_eq!(resolved, "Label_1");
    }

    #[test]
    fn resolves_by_name() {
        let labels = vec![label("Label_1", "Invoices")];
        let resolved = resolve_existing_label_id("invoices", &labels).unwrap();
        assert_eq!(resolved, "Label_1");
    }

    #[test]
    fn unknown_label_is_error() {
        let labels = vec![label("Label_1", "Invoices")];
        let err = resolve_existing_label_id("Receipts", &labels).unwrap_err();
        assert!(err.to_string().contains("Unknown label"));
    }
}
