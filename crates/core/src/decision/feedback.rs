//! Implicit feedback: Gmail label changes that undo or correct one of our
//! actions, read from the same history pages ingestion already fetches.

use crate::db::Database;
use crate::gmail::models::HistoryRecord;

/// One message's label changes in one history record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelEvent {
    pub gmail_message_id: String,
    pub history_id: String,
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

pub fn collect(record: &HistoryRecord, events: &mut Vec<LabelEvent>) {
    let mut push = |id: &str, labels: &[String], added: bool| {
        let event = LabelEvent {
            gmail_message_id: id.to_owned(),
            history_id: record.id.clone(),
            added: if added { labels.to_vec() } else { vec![] },
            removed: if added { vec![] } else { labels.to_vec() },
        };
        events.push(event);
    };
    for envelope in record.labels_added.as_deref().unwrap_or_default() {
        push(&envelope.message.id, &envelope.label_ids, true);
    }
    for envelope in record.labels_removed.as_deref().unwrap_or_default() {
        push(&envelope.message.id, &envelope.label_ids, false);
    }
}

fn is_user_label(id: &str) -> bool {
    id.starts_with("Label_")
}

/// What a label change says about a decision that planned `planned_add` and
/// `planned_remove`, if anything.
fn classify(
    label: &str,
    added: bool,
    planned_add: &[String],
    planned_remove: &[String],
) -> Option<&'static str> {
    if added {
        if planned_remove.iter().any(|l| l == label) {
            return (label == "INBOX").then_some("unarchived");
        }
        return (is_user_label(label) && !planned_add.iter().any(|l| l == label))
            .then_some("label_added");
    }
    if !planned_add.iter().any(|l| l == label) {
        return None;
    }
    match label {
        "TRASH" => Some("restored_from_trash"),
        "SPAM" => Some("restored_from_spam"),
        "STARRED" => Some("unstarred"),
        l if is_user_label(l) => Some("label_removed"),
        _ => None,
    }
}

/// Records feedback for every event that touches a message we acted on.
/// Changes made by our own actions match the plan and are ignored.
pub fn record(
    db: &Database,
    account_email: &str,
    events: &[LabelEvent],
) -> rusqlite::Result<usize> {
    let mut recorded = 0;
    for event in events {
        let plans =
            db.with_verdicts(|repo| repo.applied_plans(account_email, &event.gmail_message_id))?;
        let Some(latest) = plans.first() else {
            continue;
        };
        let changes = event
            .added
            .iter()
            .map(|l| (l, true))
            .chain(event.removed.iter().map(|l| (l, false)));
        for (label, added) in changes {
            // The newest plan that explains the change, else the newest one.
            let explained = plans.iter().find_map(|plan| {
                classify(label, added, &plan.add_label_ids, &plan.remove_label_ids)
                    .filter(|kind| *kind != "label_added")
                    .map(|kind| (plan, kind))
            });
            let all_added: Vec<String> =
                plans.iter().flat_map(|p| p.add_label_ids.clone()).collect();
            let (plan, kind) = match explained {
                Some(found) => found,
                None => match classify(label, added, &all_added, &[]) {
                    Some(kind @ "label_added") => (latest, kind),
                    _ => continue,
                },
            };
            if db.with_verdicts(|repo| {
                repo.record_feedback(
                    account_email,
                    plan.message_id,
                    Some(plan.run_id),
                    plan.step_id,
                    Some(plan.rule_index),
                    kind,
                    label,
                    &event.history_id,
                )
            })? {
                recorded += 1;
            }
        }
    }
    Ok(recorded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::worker::tests::seeded;

    fn event(added: &[&str], removed: &[&str]) -> LabelEvent {
        LabelEvent {
            gmail_message_id: "g1".into(),
            history_id: "99".into(),
            added: added.iter().map(|s| (*s).to_owned()).collect(),
            removed: removed.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    #[test]
    fn undoing_our_label_is_negative_feedback_on_that_step() {
        let db = seeded();
        db.with_conn(|c| {
            c.execute(
                "UPDATE workflow_action_plans SET add_label_ids = '[\"Label_1\"]'",
                [],
            )
        })
        .unwrap();
        let recorded = record(
            &db,
            "a@x",
            &[event(&[], &["Label_1"]), event(&["Label_2"], &[])],
        )
        .unwrap();
        assert_eq!(recorded, 2);
        let kinds: Vec<_> = db
            .with_verdicts(|r| r.feedback_for_message(1))
            .unwrap()
            .into_iter()
            .map(|f| (f.step_id, f.kind, f.label_id))
            .collect();
        assert_eq!(
            kinds,
            [
                (Some(2), "label_removed".to_owned(), "Label_1".to_owned()),
                (Some(2), "label_added".to_owned(), "Label_2".to_owned()),
            ]
        );
        // Replaying the same history records nothing new.
        assert_eq!(record(&db, "a@x", &[event(&[], &["Label_1"])]).unwrap(), 0);
    }

    #[test]
    fn our_own_changes_and_unrelated_system_labels_are_ignored() {
        let db = seeded();
        db.with_conn(|c| {
            c.execute(
                "UPDATE workflow_action_plans SET add_label_ids = '[\"Label_1\"]'",
                [],
            )
        })
        .unwrap();
        let recorded = record(&db, "a@x", &[event(&["Label_1", "UNREAD"], &["INBOX"])]).unwrap();
        assert_eq!(recorded, 0);
        assert_eq!(
            classify("TRASH", false, &["TRASH".into()], &[]),
            Some("restored_from_trash")
        );
        assert_eq!(
            classify("INBOX", true, &[], &["INBOX".into()]),
            Some("unarchived")
        );
    }
}
