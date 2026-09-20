use super::response_parser::ParsedAction;
use crate::gmail::models::Label;
use crate::gmail::GmailClient;
use std::collections::BTreeSet;

#[derive(Debug, Clone)]
pub struct LabelMutation {
    pub add: Vec<String>,
    pub remove: Vec<String>,
}

pub async fn execute_action(
    gmail: &mut GmailClient,
    email_id: &str,
    action: &ParsedAction,
    existing_labels: &[Label],
) -> Result<(), crate::gmail::GmailError> {
    execute_actions(
        gmail,
        email_id,
        std::slice::from_ref(action),
        existing_labels,
    )
    .await
}

pub async fn execute_actions(
    gmail: &mut GmailClient,
    email_id: &str,
    actions: &[ParsedAction],
    existing_labels: &[Label],
) -> Result<(), crate::gmail::GmailError> {
    let mutation = resolve_actions(actions, existing_labels)?;

    if mutation.add.is_empty() && mutation.remove.is_empty() {
        return Ok(());
    }

    let add_refs: Vec<&str> = mutation.add.iter().map(String::as_str).collect();
    let remove_refs: Vec<&str> = mutation.remove.iter().map(String::as_str).collect();
    gmail
        .modify_labels(email_id, &add_refs, &remove_refs)
        .await?;

    Ok(())
}

pub fn resolve_actions(
    actions: &[ParsedAction],
    existing_labels: &[Label],
) -> Result<LabelMutation, crate::gmail::GmailError> {
    let mut add = BTreeSet::new();
    let mut remove = BTreeSet::new();

    for action in actions {
        let (to_add, to_remove) = action_labels(action, existing_labels)?;
        add.extend(to_add);
        remove.extend(to_remove);
    }

    if let Some(label) = add.intersection(&remove).next() {
        return Err(crate::gmail::GmailError::Auth(format!(
            "Conflicting label changes for {}",
            label
        )));
    }
    if add.contains("TRASH") && add.contains("SPAM") {
        return Err(crate::gmail::GmailError::Auth(
            "Conflicting mailbox destination changes".into(),
        ));
    }

    Ok(LabelMutation {
        add: add.into_iter().collect(),
        remove: remove.into_iter().collect(),
    })
}

fn action_labels(
    action: &ParsedAction,
    existing_labels: &[Label],
) -> Result<(Vec<String>, Vec<String>), crate::gmail::GmailError> {
    let labels = match action {
        ParsedAction::Label(value) => (
            vec![resolve_existing_label_id(value, existing_labels)?],
            vec![],
        ),
        ParsedAction::RemoveLabel(value) => (
            vec![],
            vec![resolve_existing_label_id(value, existing_labels)?],
        ),
        ParsedAction::Archive => (vec![], vec!["INBOX".into()]),
        ParsedAction::Trash => (vec!["TRASH".into()], vec!["INBOX".into()]),
        ParsedAction::Spam => (vec!["SPAM".into()], vec!["INBOX".into()]),
        ParsedAction::MarkRead => (vec![], vec!["UNREAD".into()]),
        ParsedAction::MarkUnread => (vec!["UNREAD".into()], vec![]),
        ParsedAction::Star => (vec!["STARRED".into()], vec![]),
    };
    Ok(labels)
}

fn resolve_existing_label_id(
    value: &str,
    existing_labels: &[Label],
) -> Result<String, crate::gmail::GmailError> {
    crate::rules::engine::find_label(value, existing_labels)
        .map(|label| label.id.clone())
        .ok_or_else(|| {
            crate::gmail::GmailError::Auth(format!(
                "Unknown label: {value}. Labels are never created automatically, so it must already exist in Gmail; the cached label list refreshes periodically."
            ))
        })
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
    fn resolves_add_and_remove_actions_once() {
        let labels = vec![label("Label_1", "Invoices")];
        let mutation = resolve_actions(
            &[
                ParsedAction::Label("Invoices".into()),
                ParsedAction::Archive,
                ParsedAction::Label("Label_1".into()),
            ],
            &labels,
        )
        .unwrap();

        assert_eq!(mutation.add, ["Label_1"]);
        assert_eq!(mutation.remove, ["INBOX"]);
    }

    #[test]
    fn unknown_label_is_error() {
        let labels = vec![label("Label_1", "Invoices")];
        let err = resolve_existing_label_id("Receipts", &labels).unwrap_err();
        assert!(err.to_string().contains("Unknown label"));
    }

    #[test]
    fn rejects_conflicting_label_changes() {
        let labels = vec![label("Label_1", "Invoices")];
        let err = resolve_actions(
            &[
                ParsedAction::Label("Invoices".into()),
                ParsedAction::RemoveLabel("Label_1".into()),
            ],
            &labels,
        )
        .unwrap_err();

        assert!(err.to_string().contains("Conflicting label changes"));
    }
}
