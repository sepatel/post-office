use std::collections::{HashMap, HashSet};
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{LlmClient, LlmError, ProcessRequest, ProcessResponse};
use crate::db::Database;

const MAX_RETRIES_PER_PROVIDER: usize = 2;
const DEFAULT_CONTEXT_WINDOW_TOKENS: u32 = 8_192;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmProviderProfile {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub model: String,
    #[serde(default = "default_api_key_ref")]
    pub api_key_ref: String,
    #[serde(default = "default_quality_tier")]
    pub quality_tier: String,
    #[serde(default = "default_privacy_status")]
    pub privacy_status: String,
    #[serde(default)]
    pub input_cost_per_million_usd: f64,
    #[serde(default)]
    pub output_cost_per_million_usd: f64,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_context_window_tokens")]
    pub context_window_tokens: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmRoutingPolicy {
    pub id: String,
    pub name: String,
    pub candidate_provider_ids: Vec<String>,
    #[serde(default)]
    pub minimum_quality: String,
    #[serde(default)]
    pub privacy_requirement: PrivacyRequirement,
    #[serde(default = "default_true")]
    pub allow_fallback: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyRequirement {
    #[default]
    Any,
    LocalOrZdr,
    ZdrOnly,
    LocalOnly,
}

#[derive(Clone)]
struct ProviderClient {
    profile: LlmProviderProfile,
    client: Option<LlmClient>,
    init_error: Option<String>,
}

#[derive(Clone)]
pub struct InferenceRouter {
    providers: HashMap<String, ProviderClient>,
    policies: HashMap<String, LlmRoutingPolicy>,
    default_policy: String,
    max_tokens: u32,
    database: Option<Database>,
}

impl InferenceRouter {
    pub fn from_config(config: &crate::config::AppConfig) -> Self {
        let mut profiles = config.llm_providers.clone();
        if !profiles.iter().any(|profile| profile.id == "legacy") {
            profiles.push(legacy_profile(config));
        }

        let profiles = profiles
            .into_iter()
            .map(|profile| {
                if profile.id == "legacy" {
                    (profile.id.clone(), legacy_profile(config))
                } else {
                    (profile.id.clone(), profile)
                }
            })
            .collect::<HashMap<_, _>>();

        let policies = if config.llm_routing_policies.is_empty() {
            let default_policy = LlmRoutingPolicy {
                id: "default".into(),
                name: "Default".into(),
                candidate_provider_ids: vec!["legacy".into()],
                minimum_quality: String::new(),
                privacy_requirement: PrivacyRequirement::Any,
                allow_fallback: true,
            };
            HashMap::from([(default_policy.id.clone(), default_policy)])
        } else {
            config
                .llm_routing_policies
                .iter()
                .cloned()
                .map(|policy| (policy.id.clone(), policy))
                .collect()
        };

        let providers = profiles
            .into_values()
            .map(|profile| {
                let key_ref = if profile.id == "legacy" {
                    "legacy".to_string()
                } else if profile.api_key_ref.trim().is_empty() {
                    profile.id.clone()
                } else {
                    profile.api_key_ref.clone()
                };
                let key = if key_ref == "legacy" {
                    Ok(config.llm_api_key.clone())
                } else {
                    crate::llm::credentials::load_provider_api_key(&key_ref)
                        .map(|key| key.unwrap_or_default())
                };
                let init_error = key.as_ref().err().cloned().or_else(|| {
                    if profile.base_url.trim().is_empty() || profile.model.trim().is_empty() {
                        Some("base URL and model are required".into())
                    } else {
                        None
                    }
                });
                let client = if init_error.is_some() {
                    None
                } else {
                    Some(LlmClient::with_options(
                        &profile.base_url,
                        key.as_ref().unwrap(),
                        &profile.model,
                        profile.timeout_secs,
                        Some(profile.id.clone()),
                    ))
                };
                (
                    profile.id.clone(),
                    ProviderClient {
                        profile,
                        client,
                        init_error,
                    },
                )
            })
            .collect();

        Self {
            providers,
            policies,
            default_policy: if config.llm_default_policy.trim().is_empty() {
                "default".into()
            } else {
                config.llm_default_policy.clone()
            },
            max_tokens: config.llm_max_tokens,
            database: None,
        }
    }

    pub fn with_database(mut self, database: Database) -> Self {
        self.database = Some(database);
        self
    }

    pub async fn process(
        &self,
        policy_id: &str,
        request: ProcessRequest,
    ) -> Result<ProcessResponse, LlmError> {
        let policy = self.policy(policy_id);
        let (candidates, unavailable) = self.candidates(&policy);
        if candidates.is_empty() {
            let detail = if unavailable.is_empty() {
                "no providers meet the policy requirements".to_string()
            } else {
                unavailable.join("; ")
            };
            return Err(LlmError::Routing(format!(
                "policy '{}' has no eligible providers: {}",
                policy.id, detail
            )));
        }

        let mut failures = unavailable;
        for (index, provider) in candidates.iter().enumerate() {
            let Some(client) = provider.client.as_ref() else {
                failures.push(format!(
                    "{}: {}",
                    provider.profile.id,
                    provider.init_error.as_deref().unwrap_or("not configured")
                ));
                continue;
            };

            let retries = attempts_per_candidate(&policy);
            for attempt in 0..retries {
                match client.process(request.clone()).await {
                    Ok(response) => {
                        self.clear_rate_limit(&provider.profile.id);
                        return Ok(response);
                    }
                    Err(error) => {
                        if let Some(until) = rate_limit_reset_at(&error) {
                            self.mark_rate_limited(&provider.profile.id, until);
                            failures.push(format!(
                                "{}: rate limited until {}",
                                provider.profile.id,
                                until.to_rfc3339()
                            ));
                            break;
                        }
                        let retryable = is_retryable(&error);
                        failures.push(format!(
                            "{} ({}): {}",
                            provider.profile.id,
                            provider.profile.model,
                            describe_error(&error)
                        ));
                        if !retryable || attempt + 1 >= retries {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(250 * 2u64.pow(attempt as u32)))
                            .await;
                    }
                }
            }

            if !policy.allow_fallback || index + 1 >= candidates.len() {
                break;
            }
        }

        Err(LlmError::Routing(format!(
            "policy '{}' exhausted: {}",
            policy.id,
            failures.join("; ")
        )))
    }

    pub async fn chat_json(
        &self,
        policy_id: &str,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<Value, LlmError> {
        let response = self
            .process(
                policy_id,
                ProcessRequest {
                    system_prompt: Some(system_prompt.to_string()),
                    user_prompt: user_prompt.to_string(),
                    model: None,
                    temperature: Some(0.3),
                    max_tokens: None,
                },
            )
            .await?;
        let content = strip_code_fence(&response.content);
        serde_json::from_str(&content).map_err(|e| LlmError::ParseError(e.to_string()))
    }

    pub fn policy_ids(&self) -> Vec<String> {
        let mut ids = self.policies.keys().cloned().collect::<Vec<_>>();
        ids.sort();
        ids
    }

    pub fn max_tokens(&self) -> u32 {
        self.max_tokens
    }

    pub fn input_token_budget(&self, policy_id: &str) -> usize {
        let policy = self.policy(policy_id);
        let (candidates, _) = self.candidates(&policy);
        let context_window = candidates
            .into_iter()
            .map(|provider| provider.profile.context_window_tokens as usize)
            .min()
            .unwrap_or(DEFAULT_CONTEXT_WINDOW_TOKENS as usize);
        context_window.saturating_sub(self.max_tokens as usize)
    }

    fn policy(&self, policy_id: &str) -> LlmRoutingPolicy {
        self.policies
            .get(policy_id)
            .or_else(|| self.policies.get(&self.default_policy))
            .cloned()
            .unwrap_or(LlmRoutingPolicy {
                id: "default".into(),
                name: "Default".into(),
                candidate_provider_ids: vec!["legacy".into()],
                minimum_quality: String::new(),
                privacy_requirement: PrivacyRequirement::Any,
                allow_fallback: true,
            })
    }

    fn candidates(&self, policy: &LlmRoutingPolicy) -> (Vec<&ProviderClient>, Vec<String>) {
        let mut candidates = Vec::new();
        let mut unavailable = Vec::new();
        let mut seen = HashSet::new();
        for provider_id in &policy.candidate_provider_ids {
            if !seen.insert(provider_id) {
                unavailable.push(format!("{}: listed more than once", provider_id));
                continue;
            }
            let Some(provider) = self.providers.get(provider_id) else {
                unavailable.push(format!("{}: not configured", provider_id));
                continue;
            };
            let label = format!("{} ({})", provider.profile.id, provider.profile.model);
            if !provider.profile.enabled {
                unavailable.push(format!("{}: disabled", label));
                continue;
            }
            if !quality_at_least(&provider.profile.quality_tier, &policy.minimum_quality) {
                unavailable.push(format!(
                    "{}: quality '{}' is below required '{}'",
                    label, provider.profile.quality_tier, policy.minimum_quality
                ));
                continue;
            }
            if !privacy_allowed(
                &provider.profile.privacy_status,
                &policy.privacy_requirement,
            ) {
                unavailable.push(format!(
                    "{}: privacy '{}' does not meet '{}'",
                    label,
                    provider.profile.privacy_status,
                    privacy_requirement_name(&policy.privacy_requirement)
                ));
                continue;
            }
            if let Some(error) = &provider.init_error {
                unavailable.push(format!("{}: unavailable ({})", label, error));
                continue;
            }
            if let Some(until) = self.rate_limited_until(&provider.profile.id) {
                unavailable.push(format!(
                    "{}: rate limited until {}",
                    provider.profile.id,
                    until.to_rfc3339()
                ));
                continue;
            }
            candidates.push(provider);
        }
        (candidates, unavailable)
    }

    fn rate_limited_until(&self, provider_id: &str) -> Option<DateTime<Utc>> {
        let status = self
            .database
            .as_ref()?
            .with_llm_provider_status(|repo| repo.get(provider_id).ok().flatten())?;
        status
            .rate_limited_until
            .as_deref()
            .and_then(parse_rfc3339)
            .filter(|until| *until > Utc::now())
    }

    fn mark_rate_limited(&self, provider_id: &str, until: DateTime<Utc>) {
        if let Some(database) = &self.database {
            if let Err(error) = database.with_llm_provider_status(|repo| {
                repo.mark_rate_limited(provider_id, &until.to_rfc3339())
            }) {
                tracing::warn!(
                    "Failed to persist rate limit for {}: {}",
                    provider_id,
                    error
                );
            }
        }
    }

    fn clear_rate_limit(&self, provider_id: &str) {
        if let Some(database) = &self.database {
            if let Err(error) =
                database.with_llm_provider_status(|repo| repo.clear_rate_limit(provider_id))
            {
                tracing::warn!("Failed to clear rate limit for {}: {}", provider_id, error);
            }
        }
    }
}

fn legacy_profile(config: &crate::config::AppConfig) -> LlmProviderProfile {
    LlmProviderProfile {
        id: "legacy".into(),
        name: config.llm_legacy_name.clone(),
        base_url: config.llm_base_url.clone(),
        model: config.llm_default_model.clone(),
        api_key_ref: "legacy".into(),
        quality_tier: config.llm_legacy_quality_tier.clone(),
        privacy_status: config.llm_legacy_privacy_status.clone(),
        input_cost_per_million_usd: config.llm_input_cost_per_million_usd,
        output_cost_per_million_usd: config.llm_output_cost_per_million_usd,
        timeout_secs: config.llm_timeout_secs,
        context_window_tokens: config.llm_context_window_tokens,
        enabled: config.llm_legacy_enabled,
    }
}

fn quality_at_least(actual: &str, minimum: &str) -> bool {
    quality_rank(actual) >= quality_rank(minimum)
}

fn quality_rank(value: &str) -> u8 {
    match value.to_ascii_lowercase().as_str() {
        "strong" | "reasoning" => 2,
        "balanced" | "standard" => 1,
        _ => 0,
    }
}

fn privacy_allowed(status: &str, requirement: &PrivacyRequirement) -> bool {
    let status = status.to_ascii_lowercase();
    match requirement {
        PrivacyRequirement::Any => true,
        PrivacyRequirement::LocalOrZdr => {
            status == "local" || status == "verified_zdr" || status == "self_attested_zdr"
        }
        PrivacyRequirement::ZdrOnly => status == "verified_zdr" || status == "self_attested_zdr",
        PrivacyRequirement::LocalOnly => status == "local",
    }
}

fn privacy_requirement_name(requirement: &PrivacyRequirement) -> &'static str {
    match requirement {
        PrivacyRequirement::Any => "any provider",
        PrivacyRequirement::LocalOrZdr => "local or ZDR",
        PrivacyRequirement::ZdrOnly => "ZDR only",
        PrivacyRequirement::LocalOnly => "local only",
    }
}

fn attempts_per_candidate(policy: &LlmRoutingPolicy) -> usize {
    if policy.allow_fallback {
        1
    } else {
        MAX_RETRIES_PER_PROVIDER
    }
}

fn is_retryable(error: &LlmError) -> bool {
    match error {
        LlmError::Api(error) => match error {
            async_openai::error::OpenAIError::Reqwest(reqwest_error) => {
                reqwest_error.is_timeout() || reqwest_error.is_connect()
            }
            async_openai::error::OpenAIError::ApiError(response) => {
                response.status_code.as_u16() >= 500 || response.status_code.as_u16() == 429
            }
            _ => false,
        },
        LlmError::NoResponse => true,
        LlmError::ParseError(_) | LlmError::Routing(_) => false,
    }
}

fn describe_error(error: &LlmError) -> String {
    match error {
        LlmError::Api(async_openai::error::OpenAIError::Reqwest(error)) if error.is_timeout() => {
            "request timed out waiting for a completion".into()
        }
        LlmError::Api(async_openai::error::OpenAIError::Reqwest(error)) if error.is_connect() => {
            format!("could not connect: {error:#}")
        }
        _ => error.user_message(),
    }
}

fn rate_limit_reset_at(error: &LlmError) -> Option<DateTime<Utc>> {
    let content = match error {
        LlmError::Api(async_openai::error::OpenAIError::JSONDeserialize(_, content)) => content,
        LlmError::Api(async_openai::error::OpenAIError::ApiError(response))
            if response.status_code.as_u16() == 429 =>
        {
            return Some(Utc::now() + chrono::Duration::minutes(5));
        }
        _ => return None,
    };
    let response: Value = serde_json::from_str(content).ok()?;
    let code = response
        .pointer("/error/code")
        .and_then(|value| value.as_i64().or_else(|| value.as_str()?.parse().ok()));
    if code != Some(429) {
        return None;
    }
    response
        .pointer("/error/metadata/headers/X-RateLimit-Reset")
        .and_then(|value| {
            value
                .as_str()
                .and_then(parse_reset_time)
                .or_else(|| value.as_i64().and_then(parse_reset_timestamp))
        })
        .or_else(|| Some(Utc::now() + chrono::Duration::minutes(5)))
}

fn parse_reset_time(value: &str) -> Option<DateTime<Utc>> {
    value.parse::<i64>().ok().and_then(parse_reset_timestamp)
}

fn parse_reset_timestamp(timestamp: i64) -> Option<DateTime<Utc>> {
    if timestamp > 100_000_000_000 {
        Utc.timestamp_millis_opt(timestamp).single()
    } else {
        Utc.timestamp_opt(timestamp, 0).single()
    }
}

fn parse_rfc3339(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn strip_code_fence(content: &str) -> String {
    let trimmed = content.trim();
    if let Some(open) = trimmed.find("```") {
        let after_open = &trimmed[open + 3..];
        let after_lang = match after_open.find('\n') {
            Some(nl) => &after_open[nl + 1..],
            None => after_open,
        };
        if let Some(end) = after_lang.find("```") {
            return after_lang[..end].trim().to_string();
        }
    }
    trimmed.to_string()
}

fn default_api_key_ref() -> String {
    String::new()
}

fn default_quality_tier() -> String {
    "balanced".into()
}

fn default_privacy_status() -> String {
    "unknown".into()
}

fn default_timeout_secs() -> u64 {
    30
}

fn default_context_window_tokens() -> u32 {
    DEFAULT_CONTEXT_WINDOW_TOKENS
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::config::AppConfig;
    use crate::db::Database;

    #[test]
    fn privacy_requirement_filters_provider_statuses() {
        assert!(privacy_allowed("local", &PrivacyRequirement::LocalOnly));
        assert!(!privacy_allowed(
            "verified_zdr",
            &PrivacyRequirement::LocalOnly
        ));
        assert!(privacy_allowed(
            "self_attested_zdr",
            &PrivacyRequirement::LocalOrZdr
        ));
        assert!(!privacy_allowed("unknown", &PrivacyRequirement::ZdrOnly));
    }

    #[test]
    fn quality_requirement_does_not_downgrade_fallbacks() {
        assert!(quality_at_least("strong", "balanced"));
        assert!(quality_at_least("balanced", "cheap"));
        assert!(!quality_at_least("cheap", "strong"));
    }

    #[test]
    fn fallback_policy_attempts_each_provider_once() {
        let policy = LlmRoutingPolicy {
            id: "fallback".into(),
            name: "Fallback".into(),
            candidate_provider_ids: vec!["first".into(), "second".into()],
            minimum_quality: String::new(),
            privacy_requirement: PrivacyRequirement::Any,
            allow_fallback: true,
        };

        assert_eq!(attempts_per_candidate(&policy), 1);
    }

    #[test]
    fn legacy_metadata_can_satisfy_local_strong_policy() {
        let config = AppConfig {
            llm_legacy_quality_tier: "strong".into(),
            llm_legacy_privacy_status: "local".into(),
            llm_routing_policies: vec![LlmRoutingPolicy {
                id: "local".into(),
                name: "Local".into(),
                candidate_provider_ids: vec!["legacy".into()],
                minimum_quality: "strong".into(),
                privacy_requirement: PrivacyRequirement::LocalOnly,
                allow_fallback: true,
            }],
            ..AppConfig::default()
        };

        let router = InferenceRouter::from_config(&config);
        let policy = router.policy("local");
        let (candidates, unavailable) = router.candidates(&policy);

        assert_eq!(candidates.len(), 1);
        assert!(unavailable.is_empty());
    }

    #[test]
    fn reports_privacy_filtered_provider() {
        let config = AppConfig {
            llm_routing_policies: vec![LlmRoutingPolicy {
                id: "local".into(),
                name: "Local".into(),
                candidate_provider_ids: vec!["legacy".into()],
                minimum_quality: String::new(),
                privacy_requirement: PrivacyRequirement::LocalOnly,
                allow_fallback: true,
            }],
            ..AppConfig::default()
        };
        let router = InferenceRouter::from_config(&config);
        let policy = router.policy("local");
        let (candidates, unavailable) = router.candidates(&policy);

        assert!(candidates.is_empty());
        assert!(unavailable[0].contains("does not meet 'local only'"));
    }

    #[test]
    fn parses_openrouter_numeric_rate_limit_code_and_reset() {
        let content = r#"{"error":{"code":429,"metadata":{"headers":{"X-RateLimit-Reset":"1785542400000"}}}}"#;
        let decode_error = serde_json::from_str::<Value>("not json").unwrap_err();
        let error = LlmError::Api(async_openai::error::OpenAIError::JSONDeserialize(
            decode_error,
            content.into(),
        ));

        let reset = rate_limit_reset_at(&error).unwrap();

        assert_eq!(reset.timestamp_millis(), 1_785_542_400_000);
    }

    #[test]
    fn skips_a_provider_until_its_persisted_reset_time() {
        let db = Database::open(Path::new(":memory:")).unwrap();
        db.migrate().unwrap();
        let reset = Utc::now() + chrono::Duration::hours(1);
        db.with_llm_provider_status(|repo| repo.mark_rate_limited("legacy", &reset.to_rfc3339()))
            .unwrap();

        let router = InferenceRouter::from_config(&AppConfig::default()).with_database(db);
        let policy = router.policy("default");
        let (candidates, unavailable) = router.candidates(&policy);

        assert!(candidates.is_empty());
        assert!(unavailable[0].contains("rate limited until"));
    }
}
