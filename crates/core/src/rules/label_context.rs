use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::db::label_qualifications::LabelQualification;
use crate::db::rule_label_overrides::RuleLabelOverride;

/// What the classifier is told about one label: the global default with any
/// per-rule override applied field-by-field. An empty override field falls
/// back to the global value, so clearing a field reverts it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedLabel {
    pub label_id: String,
    pub description: String,
    pub examples: Vec<String>,
    pub negative_examples: Vec<String>,
    pub has_override: bool,
    pub source: String,
}

impl ResolvedLabel {
    pub fn is_empty(&self) -> bool {
        self.description.trim().is_empty()
            && self.examples.is_empty()
            && self.negative_examples.is_empty()
    }
}

/// Loads the resolved label contexts for one rule: global defaults with that
/// rule's overrides applied. Missing rows resolve to an empty map, which
/// renders as bare label names.
pub fn contexts_for_rule(
    db: &crate::db::Database,
    account_email: &str,
    rule_id: i64,
) -> HashMap<String, ResolvedLabel> {
    let globals = db
        .with_label_qualifications(|repo| repo.list_for_account(account_email))
        .unwrap_or_default();
    let overrides = db
        .with_rule_label_overrides(|repo| repo.list_for_rule(rule_id))
        .unwrap_or_default();
    resolve_label_contexts(&globals, &overrides)
}

/// Loads resolved contexts for every rule id in `rule_ids`, sharing one
/// global fetch.
pub fn contexts_by_rule(
    db: &crate::db::Database,
    account_email: &str,
    rule_ids: &[i64],
) -> HashMap<i64, HashMap<String, ResolvedLabel>> {
    let globals = db
        .with_label_qualifications(|repo| repo.list_for_account(account_email))
        .unwrap_or_default();
    let mut by_rule = HashMap::new();
    for rule_id in rule_ids {
        let overrides = db
            .with_rule_label_overrides(|repo| repo.list_for_rule(*rule_id))
            .unwrap_or_default();
        by_rule.insert(*rule_id, resolve_label_contexts(&globals, &overrides));
    }
    by_rule
}

/// Merges global qualifications with per-rule overrides, keyed by label id.
pub fn resolve_label_contexts(
    globals: &[LabelQualification],
    overrides: &[RuleLabelOverride],
) -> HashMap<String, ResolvedLabel> {
    let mut contexts: HashMap<String, ResolvedLabel> = HashMap::new();
    for global in globals {
        contexts.insert(
            global.label_id.clone(),
            ResolvedLabel {
                label_id: global.label_id.clone(),
                description: global.description.clone(),
                examples: global.examples.clone(),
                negative_examples: global.negative_examples.clone(),
                has_override: false,
                source: global.source.clone(),
            },
        );
    }
    for label_override in overrides {
        let entry = contexts
            .entry(label_override.label_id.clone())
            .or_insert_with(|| ResolvedLabel {
                label_id: label_override.label_id.clone(),
                ..ResolvedLabel::default()
            });
        if !label_override.description.trim().is_empty() {
            entry.description = label_override.description.clone();
        }
        if !label_override.examples.is_empty() {
            entry.examples = label_override.examples.clone();
        }
        if !label_override.negative_examples.is_empty() {
            entry.negative_examples = label_override.negative_examples.clone();
        }
        entry.has_override = true;
        if entry.source.is_empty() {
            entry.source = "rule".to_string();
        }
    }
    contexts
}

/// One-line description for a menu entry, e.g.
/// `"Finance" -- Bills from vendors. Examples: invoice; receipt`.
/// Returns `None` when there is nothing beyond the bare name.
pub fn describe_for_menu(name: &str, resolved: Option<&ResolvedLabel>) -> Option<String> {
    let resolved = resolved?;
    if resolved.is_empty() {
        return None;
    }
    let mut text = name.to_string();
    let description = resolved.description.trim();
    if !description.is_empty() {
        text.push_str(" -- ");
        text.push_str(description);
    }
    if !resolved.examples.is_empty() {
        text.push_str(". Examples: ");
        text.push_str(&resolved.examples.join("; "));
    }
    if text == name {
        return None;
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn global() -> LabelQualification {
        LabelQualification {
            account_email: "a@x".into(),
            label_id: "Label_1".into(),
            description: "Bills from vendors".into(),
            examples: vec!["invoice".into()],
            negative_examples: vec!["newsletter".into()],
            source: "user".into(),
            updated_at: String::new(),
        }
    }

    fn override_row() -> RuleLabelOverride {
        RuleLabelOverride {
            account_email: "a@x".into(),
            rule_id: 7,
            label_id: "Label_1".into(),
            description: "Only power bills for this rule".into(),
            examples: vec![],
            negative_examples: vec!["newsletter".into(), "bank alert".into()],
            updated_at: String::new(),
        }
    }

    #[test]
    fn override_replaces_only_non_empty_fields() {
        let contexts = resolve_label_contexts(&[global()], &[override_row()]);
        let resolved = contexts.get("Label_1").unwrap();

        assert!(resolved.has_override);
        assert_eq!(resolved.description, "Only power bills for this rule");
        // Empty override examples fall back to the global value.
        assert_eq!(resolved.examples, vec!["invoice".to_string()]);
        assert_eq!(
            resolved.negative_examples,
            vec!["newsletter".to_string(), "bank alert".to_string()]
        );
    }

    #[test]
    fn override_without_global_still_resolves() {
        let contexts = resolve_label_contexts(&[], &[override_row()]);
        let resolved = contexts.get("Label_1").unwrap();

        assert!(resolved.has_override);
        assert_eq!(resolved.description, "Only power bills for this rule");
    }

    #[test]
    fn empty_context_has_no_menu_description() {
        assert_eq!(
            describe_for_menu("\"Finance\"", Some(&ResolvedLabel::default())),
            None
        );
        assert_eq!(describe_for_menu("\"Finance\"", None), None);
    }

    #[test]
    fn menu_description_combines_text_and_examples() {
        let contexts = resolve_label_contexts(&[global()], &[]);
        let text =
            describe_for_menu("\"Finance\"", contexts.get("Label_1")).unwrap();

        assert!(text.contains("Bills from vendors"));
        assert!(text.contains("invoice"));
    }
}
