use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::config::AppConfig;
use crate::db::rules::CreateRuleRequest;
use crate::db::Database;
use crate::gmail::models::Label;
use crate::llm::{LlmProviderProfile, LlmRoutingPolicy, ReasoningEffort};
use crate::rules::models::{Action, Condition};

pub const BACKUP_VERSION: u32 = 1;
pub const BACKUP_APP: &str = "post-office";
// Large enough for hundreds of rules with memories, small enough to reject accidents.
pub const MAX_IMPORT_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_IMPORT_RULES: usize = 1_000;

fn default_backup_version() -> u32 {
    BACKUP_VERSION
}

fn default_backup_app() -> String {
    BACKUP_APP.into()
}

fn default_decision_reasoning_effort() -> ReasoningEffort {
    ReasoningEffort::Off
}

fn default_chat_reasoning_effort() -> ReasoningEffort {
    ReasoningEffort::ServerDefault
}

fn default_policy_id() -> String {
    "default".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupMemory {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupRule {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub conditions: Vec<Condition>,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub choices: Vec<Action>,
    #[serde(default)]
    pub choose_from_all_labels: bool,
    #[serde(default)]
    pub actions: Vec<Action>,
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_policy_id")]
    pub inference_policy: String,
    #[serde(default = "default_decision_reasoning_effort")]
    pub decision_reasoning_effort: ReasoningEffort,
    #[serde(default)]
    pub decision_max_tokens: Option<u32>,
    #[serde(default)]
    pub continue_after_match: bool,
    #[serde(default)]
    pub memories: Vec<BackupMemory>,
}

fn default_true() -> bool {
    true
}

// Non-secret LLM tuning fields. API keys and tokens are never exported:
// provider key values live in the OS keyring and Gmail OAuth is per-machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupLlmLegacy {
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default = "default_model")]
    pub default_model: String,
    #[serde(default)]
    pub temperature: f32,
    #[serde(default)]
    pub max_tokens: u32,
    #[serde(default)]
    pub timeout_secs: u64,
    #[serde(default)]
    pub context_window_tokens: u32,
    #[serde(default)]
    pub input_cost_per_million_usd: f64,
    #[serde(default)]
    pub output_cost_per_million_usd: f64,
    #[serde(default)]
    pub legacy_max_concurrent_requests: u8,
    #[serde(default)]
    pub legacy_output_tokens_per_second: f64,
    #[serde(default = "default_chat_reasoning_effort")]
    pub legacy_chat_reasoning_effort: ReasoningEffort,
    #[serde(default)]
    pub legacy_name: String,
    #[serde(default)]
    pub legacy_quality_tier: String,
    #[serde(default)]
    pub legacy_privacy_status: String,
    #[serde(default = "default_true")]
    pub legacy_enabled: bool,
}

fn default_base_url() -> String {
    "http://localhost:11434/v1".into()
}

fn default_model() -> String {
    "llama3".into()
}

impl Default for BackupLlmLegacy {
    fn default() -> Self {
        let config = AppConfig::default();
        Self::from_config(&config)
    }
}

impl BackupLlmLegacy {
    fn from_config(config: &AppConfig) -> Self {
        Self {
            base_url: config.llm_base_url.clone(),
            default_model: config.llm_default_model.clone(),
            temperature: config.llm_temperature,
            max_tokens: config.llm_max_tokens,
            timeout_secs: config.llm_timeout_secs,
            context_window_tokens: config.llm_context_window_tokens,
            input_cost_per_million_usd: config.llm_input_cost_per_million_usd,
            output_cost_per_million_usd: config.llm_output_cost_per_million_usd,
            legacy_max_concurrent_requests: config.llm_legacy_max_concurrent_requests,
            legacy_output_tokens_per_second: config.llm_legacy_output_tokens_per_second,
            legacy_chat_reasoning_effort: config.llm_legacy_chat_reasoning_effort,
            legacy_name: config.llm_legacy_name.clone(),
            legacy_quality_tier: config.llm_legacy_quality_tier.clone(),
            legacy_privacy_status: config.llm_legacy_privacy_status.clone(),
            legacy_enabled: config.llm_legacy_enabled,
        }
    }

    fn apply_to(&self, config: &mut AppConfig) {
        config.llm_base_url = self.base_url.clone();
        config.llm_default_model = self.default_model.clone();
        config.llm_temperature = self.temperature;
        config.llm_max_tokens = self.max_tokens;
        config.llm_timeout_secs = self.timeout_secs;
        config.llm_context_window_tokens = self.context_window_tokens;
        config.llm_input_cost_per_million_usd = self.input_cost_per_million_usd;
        config.llm_output_cost_per_million_usd = self.output_cost_per_million_usd;
        config.llm_legacy_max_concurrent_requests = self.legacy_max_concurrent_requests;
        config.llm_legacy_output_tokens_per_second = self.legacy_output_tokens_per_second;
        config.llm_legacy_chat_reasoning_effort = self.legacy_chat_reasoning_effort;
        config.llm_legacy_name = self.legacy_name.clone();
        config.llm_legacy_quality_tier = self.legacy_quality_tier.clone();
        config.llm_legacy_privacy_status = self.legacy_privacy_status.clone();
        config.llm_legacy_enabled = self.legacy_enabled;
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BackupLlm {
    #[serde(default)]
    pub providers: Vec<LlmProviderProfile>,
    #[serde(default)]
    pub routing_policies: Vec<LlmRoutingPolicy>,
    #[serde(default = "default_policy_id")]
    pub default_policy: String,
    #[serde(default)]
    pub legacy: BackupLlmLegacy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupDoc {
    #[serde(default = "default_backup_version")]
    pub version: u32,
    #[serde(default = "default_backup_app")]
    pub app: String,
    #[serde(default)]
    pub exported_at: String,
    #[serde(default)]
    pub account_email: Option<String>,
    #[serde(default)]
    pub label_names: HashMap<String, String>,
    #[serde(default)]
    pub rules: Vec<BackupRule>,
    #[serde(default)]
    pub llm: BackupLlm,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResult {
    pub imported_rules: usize,
    pub imported_memories: usize,
    pub merged_providers: usize,
    pub merged_policies: usize,
    pub warnings: Vec<String>,
}

pub fn parse_backup(payload: &str) -> Result<BackupDoc, String> {
    if payload.len() > MAX_IMPORT_BYTES {
        return Err(format!(
            "Backup file is too large ({} bytes, max {} bytes)",
            payload.len(),
            MAX_IMPORT_BYTES
        ));
    }
    let doc: BackupDoc =
        serde_json::from_str(payload).map_err(|e| format!("Invalid backup file: {e}"))?;
    if doc.version != BACKUP_VERSION {
        return Err(format!(
            "Unsupported backup version {} (this app reads version {})",
            doc.version, BACKUP_VERSION
        ));
    }
    if doc.app != BACKUP_APP {
        return Err(format!("Not a {} backup file", BACKUP_APP));
    }
    if doc.rules.len() > MAX_IMPORT_RULES {
        return Err(format!(
            "Backup contains {} rules (max {})",
            doc.rules.len(),
            MAX_IMPORT_RULES
        ));
    }
    for (index, rule) in doc.rules.iter().enumerate() {
        if rule.name.trim().is_empty() {
            return Err(format!("Rule #{} has an empty name", index + 1));
        }
    }
    Ok(doc)
}

// Builds a portable backup for `account_email`. Label names come from the
// locally cached Gmail labels so export works offline; secrets are excluded.
pub fn build_export(
    db: &Database,
    account_email: &str,
    config: &AppConfig,
) -> Result<BackupDoc, String> {
    let rules = db
        .with_rules(|repo| repo.list_all(account_email))
        .map_err(|e| e.to_string())?;

    let mut backup_rules = Vec::with_capacity(rules.len());
    for rule in &rules {
        let memories = db
            .with_rule_memory(|repo| repo.list_for_rule(rule.id))
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|entry| BackupMemory {
                kind: entry.kind,
                text: entry.text,
            })
            .collect();
        backup_rules.push(BackupRule {
            name: rule.name.clone(),
            description: rule.description.clone(),
            conditions: rule.conditions.clone(),
            prompt: rule.prompt.clone(),
            choices: rule.choices.clone(),
            choose_from_all_labels: rule.choose_from_all_labels,
            actions: rule.actions.clone(),
            priority: rule.priority,
            enabled: rule.enabled,
            inference_policy: rule.inference_policy.clone(),
            decision_reasoning_effort: rule.decision_reasoning_effort,
            decision_max_tokens: rule.decision_max_tokens,
            continue_after_match: rule.continue_after_match,
            memories,
        });
    }

    let label_names = db
        .with_labels(|repo| repo.get(account_email))
        .map_err(|e| e.to_string())?
        .map(|cached| {
            cached
                .labels
                .into_iter()
                .map(|label| (label.id, label.name))
                .collect()
        })
        .unwrap_or_default();

    Ok(BackupDoc {
        version: BACKUP_VERSION,
        app: BACKUP_APP.into(),
        exported_at: chrono::Utc::now().to_rfc3339(),
        account_email: Some(account_email.to_string()),
        label_names,
        rules: backup_rules,
        llm: BackupLlm {
            providers: config.llm_providers.clone(),
            routing_policies: config.llm_routing_policies.clone(),
            default_policy: config.llm_default_policy.clone(),
            legacy: BackupLlmLegacy::from_config(config),
        },
    })
}

struct LabelResolver<'a> {
    by_id: HashMap<&'a str, &'a Label>,
    by_name: HashMap<String, &'a Label>,
    // Exported id -> name map, for files whose ids differ on the new machine.
    exported_names: &'a HashMap<String, String>,
    unknown: HashSet<String>,
}

impl<'a> LabelResolver<'a> {
    fn new(live: &'a [Label], exported_names: &'a HashMap<String, String>) -> Self {
        let mut by_id = HashMap::new();
        let mut by_name = HashMap::new();
        for label in live {
            by_id.insert(label.id.as_str(), label);
            by_name.insert(label.name.to_lowercase(), label);
        }
        Self {
            by_id,
            by_name,
            exported_names,
            unknown: HashSet::new(),
        }
    }

    fn resolve(&mut self, value: &str) -> String {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return trimmed.to_string();
        }
        if let Some(label) = self.by_id.get(trimmed) {
            return label.id.clone();
        }
        if let Some(label) = self.by_name.get(&trimmed.to_lowercase()) {
            return label.id.clone();
        }
        // The file may reference an id from the old machine whose name matches
        // a differently-cased or re-created label here.
        if let Some(name) = self.exported_names.get(trimmed) {
            if let Some(label) = self.by_name.get(&name.to_lowercase()) {
                return label.id.clone();
            }
        }
        self.unknown.insert(trimmed.to_string());
        trimmed.to_string()
    }

    fn resolve_condition(&mut self, condition: &mut Condition) {
        match condition {
            Condition::Label { value, .. } => *value = self.resolve(value),
            Condition::And { conditions } | Condition::Or { conditions } => {
                for nested in conditions {
                    self.resolve_condition(nested);
                }
            }
            Condition::Not { condition } => self.resolve_condition(condition),
            _ => {}
        }
    }

    fn resolve_action(&mut self, action: &mut Action) {
        if let Action::Label { value } | Action::RemoveLabel { value } = action {
            *value = self.resolve(value);
        }
    }
}

// Imports a backup into `account_email`, appending every rule as a copy after
// the existing ones. Providers/policies are merged append-only by id; existing
// entries are never overwritten. Returns counts plus non-fatal warnings.
pub fn apply_import(
    db: &Database,
    account_email: &str,
    config: &mut AppConfig,
    mut doc: BackupDoc,
    live_labels: &[Label],
) -> Result<ImportResult, String> {
    let mut warnings = Vec::new();

    let mut merged_providers = 0;
    for provider in doc.llm.providers {
        if provider.id.trim().is_empty() {
            warnings.push("Skipped a provider with an empty id.".into());
            continue;
        }
        if config.llm_providers.iter().any(|p| p.id == provider.id) {
            warnings.push(format!(
                "Provider '{}' already exists; kept the existing one.",
                provider.id
            ));
            continue;
        }
        config.llm_providers.push(provider);
        merged_providers += 1;
    }

    let mut merged_policies = 0;
    for policy in doc.llm.routing_policies {
        if policy.id.trim().is_empty() {
            warnings.push("Skipped a routing policy with an empty id.".into());
            continue;
        }
        if config
            .llm_routing_policies
            .iter()
            .any(|p| p.id == policy.id)
        {
            warnings.push(format!(
                "Routing policy '{}' already exists; kept the existing one.",
                policy.id
            ));
            continue;
        }
        config.llm_routing_policies.push(policy);
        merged_policies += 1;
    }

    if doc.llm.default_policy.trim().is_empty() {
        warnings.push("Backup has no default routing policy; kept the current one.".into());
    } else if config
        .llm_routing_policies
        .iter()
        .any(|p| p.id == doc.llm.default_policy)
        || doc.llm.default_policy == "default"
    {
        config.llm_default_policy = doc.llm.default_policy.clone();
    } else {
        warnings.push(format!(
            "Default routing policy '{}' is unknown; kept the current one.",
            doc.llm.default_policy
        ));
    }
    doc.llm.legacy.apply_to(config);

    db.with_config(|repo| config.save(&repo))
        .map_err(|e| e.to_string())?;

    let known_policies: HashSet<String> = config
        .llm_routing_policies
        .iter()
        .map(|p| p.id.clone())
        .chain(["default".to_string(), "legacy".to_string()])
        .collect();
    let mut unknown_policies = HashSet::new();

    let existing = db
        .with_rules(|repo| repo.list_all(account_email))
        .map_err(|e| e.to_string())?;
    let first_priority = existing
        .iter()
        .map(|rule| rule.priority)
        .max()
        .unwrap_or(-1)
        + 1;

    // Preserve the exported ordering regardless of stored priority gaps.
    doc.rules.sort_by_key(|rule| rule.priority);
    let rule_count = doc.rules.len();

    let mut resolver = LabelResolver::new(live_labels, &doc.label_names);
    let mut imported_memories = 0;

    for (offset, mut rule) in doc.rules.into_iter().enumerate() {
        for condition in &mut rule.conditions {
            resolver.resolve_condition(condition);
        }
        for action in rule.choices.iter_mut().chain(rule.actions.iter_mut()) {
            resolver.resolve_action(action);
        }

        let policy = rule.inference_policy.trim().to_string();
        let policy = if policy.is_empty() || known_policies.contains(&policy) {
            if policy.is_empty() {
                default_policy_id()
            } else {
                policy
            }
        } else {
            unknown_policies.insert(policy.clone());
            default_policy_id()
        };

        let created = db
            .with_rules(|repo| {
                repo.create(
                    account_email,
                    &CreateRuleRequest {
                        name: rule.name.clone(),
                        description: rule.description.clone(),
                        conditions: rule.conditions.clone(),
                        prompt: rule.prompt.clone(),
                        choices: rule.choices.clone(),
                        choose_from_all_labels: rule.choose_from_all_labels,
                        actions: rule.actions.clone(),
                        priority: first_priority + offset as i32,
                        enabled: rule.enabled,
                        inference_policy: policy,
                        decision_reasoning_effort: rule.decision_reasoning_effort,
                        decision_max_tokens: rule.decision_max_tokens,
                        continue_after_match: rule.continue_after_match,
                    },
                )
            })
            .map_err(|e| e.to_string())?;
        for memory in rule.memories {
            if memory.text.trim().is_empty() {
                continue;
            }
            let kind = if memory.kind.trim().is_empty() {
                "note"
            } else {
                memory.kind.trim()
            };
            db.with_rule_memory(|repo| repo.insert(created.id, kind, &memory.text, "import"))
                .map_err(|e| e.to_string())?;
            imported_memories += 1;
        }
    }

    for policy in unknown_policies {
        warnings.push(format!(
            "Routing policy '{policy}' is not configured here; those rules now use Default."
        ));
    }
    for label in resolver.unknown {
        warnings.push(format!(
            "Label '{label}' was not found here; kept the original reference."
        ));
    }

    Ok(ImportResult {
        imported_rules: rule_count,
        imported_memories,
        merged_providers,
        merged_policies,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::rules::models::Operator;
    use std::path::Path;

    fn test_db() -> Database {
        let db = Database::open(Path::new(":memory:")).unwrap();
        db.migrate().unwrap();
        db
    }

    fn sample_doc() -> BackupDoc {
        BackupDoc {
            version: BACKUP_VERSION,
            app: BACKUP_APP.into(),
            exported_at: "2026-09-14T00:00:00Z".into(),
            account_email: Some("a@example.com".into()),
            label_names: HashMap::from([("Label_1".into(), "Finance".into())]),
            rules: vec![BackupRule {
                name: "Receipts".into(),
                description: Some("d".into()),
                conditions: vec![Condition::Label {
                    operator: Operator::Equals,
                    value: "Label_1".into(),
                }],
                prompt: "match?".into(),
                choices: vec![Action::Label {
                    value: "Finance".into(),
                }],
                choose_from_all_labels: false,
                actions: vec![Action::Archive],
                priority: 0,
                enabled: true,
                inference_policy: "travel".into(),
                decision_reasoning_effort: ReasoningEffort::Low,
                decision_max_tokens: Some(1024),
                continue_after_match: true,
                memories: vec![BackupMemory {
                    kind: "note".into(),
                    text: "remember this".into(),
                }],
            }],
            llm: BackupLlm {
                providers: vec![LlmProviderProfile {
                    id: "travel".into(),
                    name: "Travel".into(),
                    base_url: "https://example.test/v1".into(),
                    model: "example/cheap".into(),
                    api_key_ref: "travel".into(),
                    quality_tier: "cheap".into(),
                    privacy_status: "unknown".into(),
                    input_cost_per_million_usd: 1.0,
                    output_cost_per_million_usd: 2.0,
                    timeout_secs: 45,
                    context_window_tokens: 8192,
                    max_concurrent_requests: 4,
                    output_tokens_per_second: 30.0,
                    chat_reasoning_effort: ReasoningEffort::ServerDefault,
                    enabled: true,
                }],
                routing_policies: vec![LlmRoutingPolicy {
                    id: "travel".into(),
                    name: "Travel".into(),
                    candidate_provider_ids: vec!["travel".into()],
                    minimum_quality: "cheap".into(),
                    privacy_requirement: crate::llm::PrivacyRequirement::Any,
                    allow_fallback: true,
                }],
                default_policy: "travel".into(),
                legacy: BackupLlmLegacy::default(),
            },
        }
    }

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
    fn export_import_roundtrip_appends_as_copies() {
        let db = test_db();
        let mut config = AppConfig::default();
        let live = vec![label("Label_9", "Finance")];

        let doc = sample_doc();
        let result = apply_import(&db, "b@example.com", &mut config, doc, &live).unwrap();
        assert_eq!(result.imported_rules, 1);
        assert_eq!(result.imported_memories, 1);
        assert_eq!(result.merged_providers, 1);
        assert_eq!(result.merged_policies, 1);
        assert!(result.warnings.is_empty());

        let rules = db
            .with_rules(|repo| repo.list_all("b@example.com"))
            .unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].name, "Receipts");
        assert_eq!(rules[0].inference_policy, "travel");
        // Label id and label name both resolve to the local label id.
        assert!(matches!(
            &rules[0].conditions[0],
            Condition::Label { value, .. } if value == "Label_9"
        ));
        assert!(matches!(
            &rules[0].choices[0],
            Action::Label { value } if value == "Label_9"
        ));

        let memories = db
            .with_rule_memory(|repo| repo.list_for_rule(rules[0].id))
            .unwrap();
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].text, "remember this");

        // Export reflects what was imported, without secrets.
        let exported = build_export(&db, "b@example.com", &config).unwrap();
        let json = serde_json::to_string(&exported).unwrap();
        assert!(!json.contains("llm_api_key"));
        assert_eq!(exported.rules.len(), 1);

        // A second import appends rather than replacing.
        let again = parse_backup(&json).unwrap();
        let second = apply_import(&db, "b@example.com", &mut config, again, &live).unwrap();
        assert_eq!(second.imported_rules, 1);
        assert_eq!(
            db.with_rules(|repo| repo.list_all("b@example.com"))
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn unknown_policy_and_label_fall_back_with_warnings() {
        let db = test_db();
        let mut config = AppConfig::default();
        let mut doc = sample_doc();
        doc.llm.providers.clear();
        doc.llm.routing_policies.clear();
        doc.llm.default_policy = "default".into();

        let result = apply_import(&db, "c@example.com", &mut config, doc, &[]).unwrap();
        // Unknown routing policy + two unknown label references.
        assert_eq!(result.warnings.len(), 3);

        let rules = db
            .with_rules(|repo| repo.list_all("c@example.com"))
            .unwrap();
        assert_eq!(rules[0].inference_policy, "default");
    }

    #[test]
    fn rejects_wrong_version_and_empty_names() {
        let mut doc = sample_doc();
        doc.version = 999;
        assert!(parse_backup(&serde_json::to_string(&doc).unwrap()).is_err());

        let mut doc = sample_doc();
        doc.version = BACKUP_VERSION;
        doc.rules[0].name = "  ".into();
        assert!(parse_backup(&serde_json::to_string(&doc).unwrap()).is_err());
    }
}
