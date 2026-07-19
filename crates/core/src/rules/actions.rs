use crate::gmail::models::Label;
use crate::gmail::GmailClient;
use super::response_parser::ParsedAction;

pub async fn execute_action(
    gmail: &mut GmailClient,
    email_id: &str,
    action: &ParsedAction,
    existing_labels: &[Label],
) -> Result<(), crate::gmail::GmailError> {
    let (add, remove) = match action {
        ParsedAction::Label(name) => {
            let label = get_or_create_label(gmail, name, existing_labels).await?;
            (vec![label.id], vec![])
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
    gmail.modify_labels(email_id, &add_refs, &remove_refs).await?;

    Ok(())
}

async fn get_or_create_label(
    gmail: &mut GmailClient,
    name: &str,
    existing_labels: &[Label],
) -> Result<Label, crate::gmail::GmailError> {
    if let Some(existing) = existing_labels.iter().find(|l| l.name == name) {
        return Ok(existing.clone());
    }

    gmail.create_label(name).await
}
