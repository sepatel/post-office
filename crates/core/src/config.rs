use serde::{Deserialize, Serialize};

use crate::db::config::ConfigRepository;
use crate::llm::{LlmProviderProfile, LlmRoutingPolicy, ReasoningEffort};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub gmail_account: Option<String>,
    pub google_client_id: String,
    pub llm_base_url: String,
    pub llm_api_key: String,
    pub llm_default_model: String,
    pub llm_temperature: f32,
    pub llm_max_tokens: u32,
    pub llm_timeout_secs: u64,
    pub llm_context_window_tokens: u32,
    pub llm_legacy_max_concurrent_requests: u8,
    pub llm_legacy_max_emails_per_request: u8,
    pub llm_legacy_decision_reasoning_effort: ReasoningEffort,
    pub llm_legacy_chat_reasoning_effort: ReasoningEffort,
    pub llm_legacy_name: String,
    pub llm_legacy_quality_tier: String,
    pub llm_legacy_privacy_status: String,
    pub llm_legacy_enabled: bool,
    pub llm_input_cost_per_million_usd: f64,
    pub llm_output_cost_per_million_usd: f64,
    pub llm_providers: Vec<LlmProviderProfile>,
    pub llm_routing_policies: Vec<LlmRoutingPolicy>,
    pub llm_default_policy: String,
    pub polling_query: String,
    pub polling_interval_minutes: u32,
    pub polling_max_per_cycle: u32,
    pub polling_enabled: bool,
    pub sync_enabled: bool,
    pub sync_reconcile_interval_minutes: u32,
    pub sync_watch_topic: String,
    pub sync_watch_label_ids: String,
    pub relay_enabled: bool,
    pub relay_ws_url: String,
    pub relay_auth_token: String,
    pub tray_theme: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            gmail_account: None,
            google_client_id: String::new(),
            llm_base_url: "http://localhost:11434/v1".into(),
            llm_api_key: "ollama".into(),
            llm_default_model: "llama3".into(),
            llm_temperature: 0.3,
            llm_max_tokens: 1024,
            llm_timeout_secs: 30,
            llm_context_window_tokens: 8_192,
            llm_legacy_max_concurrent_requests: 1,
            llm_legacy_max_emails_per_request: 3,
            llm_legacy_decision_reasoning_effort: ReasoningEffort::Off,
            llm_legacy_chat_reasoning_effort: ReasoningEffort::ServerDefault,
            llm_legacy_name: "Current endpoint".into(),
            llm_legacy_quality_tier: "balanced".into(),
            llm_legacy_privacy_status: "unknown".into(),
            llm_legacy_enabled: true,
            llm_input_cost_per_million_usd: 0.0,
            llm_output_cost_per_million_usd: 0.0,
            llm_providers: vec![],
            llm_routing_policies: vec![],
            llm_default_policy: "default".into(),
            polling_query: "is:unread".into(),
            polling_interval_minutes: 5,
            polling_max_per_cycle: 100,
            polling_enabled: true,
            sync_enabled: false,
            sync_reconcile_interval_minutes: 5,
            sync_watch_topic: String::new(),
            sync_watch_label_ids: String::new(),
            relay_enabled: false,
            relay_ws_url: String::new(),
            relay_auth_token: String::new(),
            tray_theme: "auto".into(),
        }
    }
}

impl AppConfig {
    pub fn load(repo: &ConfigRepository) -> Self {
        let get = |key: &str, default: &str| -> String {
            repo.get(key)
                .ok()
                .flatten()
                .unwrap_or_else(|| default.to_string())
        };

        Self {
            gmail_account: repo
                .get("gmail.account")
                .ok()
                .flatten()
                .filter(|account| !account.trim().is_empty()),
            google_client_id: get("google.client_id", ""),
            llm_base_url: get("llm.base_url", &Self::default().llm_base_url),
            llm_api_key: get("llm.api_key", &Self::default().llm_api_key),
            llm_default_model: get("llm.default_model", &Self::default().llm_default_model),
            llm_temperature: get("llm.temperature", "0.3").parse().unwrap_or(0.3),
            llm_max_tokens: get("llm.max_tokens", "1024").parse().unwrap_or(1024),
            llm_timeout_secs: get("llm.timeout_secs", "30").parse().unwrap_or(30),
            llm_context_window_tokens: get("llm.context_window_tokens", "8192")
                .parse()
                .unwrap_or(8_192),
            llm_legacy_max_concurrent_requests: get("llm.legacy_max_concurrent_requests", "1")
                .parse()
                .unwrap_or(1),
            llm_legacy_max_emails_per_request: get("llm.legacy_max_emails_per_request", "3")
                .parse()
                .unwrap_or(3),
            llm_legacy_decision_reasoning_effort: serde_json::from_str(&get(
                "llm.legacy_decision_reasoning_effort",
                "\"off\"",
            ))
            .unwrap_or(ReasoningEffort::Off),
            llm_legacy_chat_reasoning_effort: serde_json::from_str(&get(
                "llm.legacy_chat_reasoning_effort",
                "\"server_default\"",
            ))
            .unwrap_or(ReasoningEffort::ServerDefault),
            llm_legacy_name: get("llm.legacy_name", "Current endpoint"),
            llm_legacy_quality_tier: get("llm.legacy_quality_tier", "balanced"),
            llm_legacy_privacy_status: get("llm.legacy_privacy_status", "unknown"),
            llm_legacy_enabled: get("llm.legacy_enabled", "true").parse().unwrap_or(true),
            llm_input_cost_per_million_usd: get("llm.input_cost_per_million_usd", "0")
                .parse()
                .unwrap_or(0.0),
            llm_output_cost_per_million_usd: get("llm.output_cost_per_million_usd", "0")
                .parse()
                .unwrap_or(0.0),
            llm_providers: parse_json(&get("llm.providers", "[]")),
            llm_routing_policies: parse_json(&get("llm.routing_policies", "[]")),
            llm_default_policy: get("llm.default_policy", "default"),
            polling_query: get("polling.query", &Self::default().polling_query),
            polling_interval_minutes: get("polling.interval_minutes", "5").parse().unwrap_or(5),
            polling_max_per_cycle: get("polling.max_per_cycle", "100").parse().unwrap_or(100),
            polling_enabled: get("polling.enabled", "true").parse().unwrap_or(true),
            sync_enabled: get("sync.enabled", "false").parse().unwrap_or(false),
            sync_reconcile_interval_minutes: get("sync.reconcile_interval_minutes", "5")
                .parse()
                .unwrap_or(5),
            sync_watch_topic: get("sync.watch.topic", ""),
            sync_watch_label_ids: get("sync.watch.label_ids", ""),
            relay_enabled: get("sync.relay.enabled", "false").parse().unwrap_or(false),
            relay_ws_url: get("sync.relay.ws_url", ""),
            relay_auth_token: get("sync.relay.auth_token", ""),
            tray_theme: get("ui.tray_theme", "auto"),
        }
    }

    pub fn save(&self, repo: &ConfigRepository) -> rusqlite::Result<()> {
        let set = |key: &str, value: &str| -> rusqlite::Result<()> { repo.set(key, value) };

        set("gmail.account", self.gmail_account.as_deref().unwrap_or(""))?;
        set("google.client_id", &self.google_client_id)?;
        set("llm.base_url", &self.llm_base_url)?;
        set("llm.api_key", &self.llm_api_key)?;
        set("llm.default_model", &self.llm_default_model)?;
        set("llm.temperature", &self.llm_temperature.to_string())?;
        set("llm.max_tokens", &self.llm_max_tokens.to_string())?;
        set("llm.timeout_secs", &self.llm_timeout_secs.to_string())?;
        set(
            "llm.context_window_tokens",
            &self.llm_context_window_tokens.to_string(),
        )?;
        set(
            "llm.legacy_max_concurrent_requests",
            &self.llm_legacy_max_concurrent_requests.to_string(),
        )?;
        set(
            "llm.legacy_max_emails_per_request",
            &self.llm_legacy_max_emails_per_request.to_string(),
        )?;
        set(
            "llm.legacy_decision_reasoning_effort",
            &serde_json::to_string(&self.llm_legacy_decision_reasoning_effort)
                .unwrap_or_else(|_| "\"off\"".into()),
        )?;
        set(
            "llm.legacy_chat_reasoning_effort",
            &serde_json::to_string(&self.llm_legacy_chat_reasoning_effort)
                .unwrap_or_else(|_| "\"server_default\"".into()),
        )?;
        set("llm.legacy_name", &self.llm_legacy_name)?;
        set("llm.legacy_quality_tier", &self.llm_legacy_quality_tier)?;
        set("llm.legacy_privacy_status", &self.llm_legacy_privacy_status)?;
        set("llm.legacy_enabled", &self.llm_legacy_enabled.to_string())?;
        set(
            "llm.input_cost_per_million_usd",
            &self.llm_input_cost_per_million_usd.to_string(),
        )?;
        set(
            "llm.output_cost_per_million_usd",
            &self.llm_output_cost_per_million_usd.to_string(),
        )?;
        set(
            "llm.providers",
            &serde_json::to_string(&self.llm_providers).unwrap_or_else(|_| "[]".into()),
        )?;
        set(
            "llm.routing_policies",
            &serde_json::to_string(&self.llm_routing_policies).unwrap_or_else(|_| "[]".into()),
        )?;
        set("llm.default_policy", &self.llm_default_policy)?;
        set("polling.query", &self.polling_query)?;
        set(
            "polling.interval_minutes",
            &self.polling_interval_minutes.to_string(),
        )?;
        set(
            "polling.max_per_cycle",
            &self.polling_max_per_cycle.to_string(),
        )?;
        set("polling.enabled", &self.polling_enabled.to_string())?;
        set("sync.enabled", &self.sync_enabled.to_string())?;
        set(
            "sync.reconcile_interval_minutes",
            &self.sync_reconcile_interval_minutes.to_string(),
        )?;
        set("sync.watch.topic", &self.sync_watch_topic)?;
        set("sync.watch.label_ids", &self.sync_watch_label_ids)?;
        set("sync.relay.enabled", &self.relay_enabled.to_string())?;
        set("sync.relay.ws_url", &self.relay_ws_url)?;
        set("sync.relay.auth_token", &self.relay_auth_token)?;
        set("ui.tray_theme", &self.tray_theme)?;
        Ok(())
    }
}

fn parse_json<T>(value: &str) -> Vec<T>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_str(value).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::db::Database;
    use crate::llm::PrivacyRequirement;

    #[test]
    fn saves_and_loads_inference_configuration_together() {
        let db = Database::open(Path::new(":memory:")).unwrap();
        db.migrate().unwrap();
        let config = AppConfig {
            llm_providers: vec![LlmProviderProfile {
                id: "travel".into(),
                name: "Travel fallback".into(),
                base_url: "https://example.test/v1".into(),
                model: "example/cheap".into(),
                api_key_ref: "travel".into(),
                quality_tier: "cheap".into(),
                privacy_status: "unknown".into(),
                input_cost_per_million_usd: 1.0,
                output_cost_per_million_usd: 2.0,
                timeout_secs: 45,
                context_window_tokens: 8_192,
                max_concurrent_requests: 4,
                max_emails_per_request: 6,
                decision_reasoning_effort: ReasoningEffort::Off,
                chat_reasoning_effort: ReasoningEffort::ServerDefault,
                enabled: true,
            }],
            llm_routing_policies: vec![LlmRoutingPolicy {
                id: "travel".into(),
                name: "Travel".into(),
                candidate_provider_ids: vec!["travel".into()],
                minimum_quality: "cheap".into(),
                privacy_requirement: PrivacyRequirement::Any,
                allow_fallback: true,
            }],
            llm_default_policy: "travel".into(),
            llm_timeout_secs: 45,
            llm_legacy_max_concurrent_requests: 1,
            llm_legacy_max_emails_per_request: 3,
            llm_legacy_decision_reasoning_effort: ReasoningEffort::Off,
            llm_legacy_chat_reasoning_effort: ReasoningEffort::ServerDefault,
            llm_legacy_name: "Local agent".into(),
            llm_legacy_quality_tier: "strong".into(),
            llm_legacy_privacy_status: "local".into(),
            llm_legacy_enabled: true,
            ..AppConfig::default()
        };

        db.with_config(|repo| config.save(&repo)).unwrap();
        let loaded = db.with_config(|repo| AppConfig::load(&repo));

        assert_eq!(loaded.llm_default_policy, "travel");
        assert_eq!(loaded.llm_timeout_secs, 45);
        assert_eq!(loaded.llm_legacy_max_concurrent_requests, 1);
        assert_eq!(loaded.llm_legacy_max_emails_per_request, 3);
        assert_eq!(
            loaded.llm_legacy_decision_reasoning_effort,
            ReasoningEffort::Off
        );
        assert_eq!(
            loaded.llm_providers[0].chat_reasoning_effort,
            ReasoningEffort::ServerDefault
        );
        assert_eq!(loaded.llm_legacy_name, "Local agent");
        assert_eq!(loaded.llm_legacy_quality_tier, "strong");
        assert_eq!(loaded.llm_legacy_privacy_status, "local");
        assert_eq!(loaded.llm_providers[0].model, "example/cheap");
        assert_eq!(
            loaded.llm_routing_policies[0].candidate_provider_ids,
            ["travel"]
        );
    }
}
