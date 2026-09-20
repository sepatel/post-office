use std::collections::{HashMap, HashSet};
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{InferenceRuntime, LlmClient, LlmError, ProcessKind, ProcessRequest, ProcessResponse};
use crate::db::Database;

const MAX_RETRIES_PER_PROVIDER: usize = 2;
const ENDPOINT_UNAVAILABLE_SECS: i64 = 300;
const DEFAULT_CONTEXT_WINDOW_TOKENS: u32 = 8_192;
/// Hard ceiling for rule decisions. llama.cpp slots must hold prompt plus
/// thinking plus answer, so unbounded or very large caps can exhaust a slot
/// (observed as `Context size has been exceeded` with runaway generations).
/// Thinking tokens count toward this cap, so it must leave room for reasoning
/// plus the final decision line.
pub const MAX_DECISION_MAX_TOKENS: u32 = 8_192;
const MIN_DECISION_MAX_TOKENS: u32 = 64;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    #[default]
    Off,
    ServerDefault,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ReasoningEffort {
    pub fn config_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::ServerDefault => "server_default",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }

    pub fn request_value(self) -> Option<&'static str> {
        match self {
            Self::Off => Some("none"),
            Self::ServerDefault => None,
            Self::Minimal => Some("minimal"),
            Self::Low => Some("low"),
            Self::Medium => Some("medium"),
            Self::High => Some("high"),
            Self::Xhigh => Some("xhigh"),
            Self::Max => Some("max"),
        }
    }
}

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
    #[serde(default = "default_max_concurrent_requests")]
    pub max_concurrent_requests: u8,
    #[serde(default)]
    pub output_tokens_per_second: f64,
    #[serde(default = "default_chat_reasoning_effort")]
    pub chat_reasoning_effort: ReasoningEffort,
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
    endpoint: String,
    effective_max_concurrent_requests: usize,
}

#[derive(Clone)]
pub struct InferenceRouter {
    providers: HashMap<String, ProviderClient>,
    policies: HashMap<String, LlmRoutingPolicy>,
    default_policy: String,
    max_tokens: u32,
    database: Option<Database>,
    runtime: InferenceRuntime,
    bypass_endpoint_circuit: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct LlmProviderAttribution {
    pub provider_id: String,
    pub provider_name: String,
    pub model: String,
    pub endpoint: String,
}

impl InferenceRouter {
    pub fn from_config(config: &crate::config::AppConfig) -> Self {
        Self::from_config_with_runtime(config, InferenceRuntime::default())
    }

    pub fn from_config_with_runtime(
        config: &crate::config::AppConfig,
        runtime: InferenceRuntime,
    ) -> Self {
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

        let endpoint_limits = profiles.values().filter(|profile| profile.enabled).fold(
            HashMap::<String, usize>::new(),
            |mut limits, profile| {
                let endpoint = endpoint_key(&profile.base_url);
                let limit = profile.max_concurrent_requests.max(1) as usize;
                limits
                    .entry(endpoint)
                    .and_modify(|existing| *existing = (*existing).min(limit))
                    .or_insert(limit);
                limits
            },
        );

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
                let endpoint = endpoint_key(&profile.base_url);
                let key_ref = if profile.id == "legacy" {
                    "legacy".to_string()
                } else if profile.api_key_ref.trim().is_empty() {
                    profile.id.clone()
                } else {
                    profile.api_key_ref.clone()
                };
                let key = crate::llm::credentials::load_provider_api_key(&key_ref).map(|key| {
                    key.filter(|value| !value.trim().is_empty())
                        // Config remains a read-only fallback for databases created before
                        // provider secrets moved into the OS keyring.
                        .unwrap_or_else(|| {
                            if key_ref == "legacy" {
                                config.llm_api_key.clone()
                            } else {
                                String::new()
                            }
                        })
                });
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
                        endpoint: endpoint.clone(),
                        effective_max_concurrent_requests: endpoint_limits
                            .get(&endpoint)
                            .copied()
                            .unwrap_or(1),
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
            runtime,
            bypass_endpoint_circuit: false,
        }
    }

    pub fn with_database(mut self, database: Database) -> Self {
        self.database = Some(database);
        self
    }

    /// A human-requested retry gets one probe even while the endpoint circuit is open.
    pub fn with_endpoint_circuit_bypass(mut self) -> Self {
        self.bypass_endpoint_circuit = true;
        self
    }

    pub fn provider_attribution(&self, policy_id: &str) -> Option<LlmProviderAttribution> {
        let policy = self.policy(policy_id);
        policy
            .candidate_provider_ids
            .iter()
            .find_map(|provider_id| {
                let provider = self.providers.get(provider_id)?;
                Some(LlmProviderAttribution {
                    provider_id: provider.profile.id.clone(),
                    provider_name: provider.profile.name.clone(),
                    model: provider.profile.model.clone(),
                    endpoint: provider.endpoint.clone(),
                })
            })
    }

    pub fn endpoint_circuit_open(&self, policy_id: &str) -> bool {
        if self.bypass_endpoint_circuit {
            return false;
        }
        let policy = self.policy(policy_id);
        policy.candidate_provider_ids.iter().any(|provider_id| {
            self.providers
                .get(provider_id)
                .and_then(|provider| self.endpoint_unavailable_until(&provider.endpoint))
                .is_some()
        })
    }

    pub async fn process(
        &self,
        policy_id: &str,
        request: ProcessRequest,
    ) -> Result<ProcessResponse, LlmError> {
        let mut cancellation = self.runtime.cancellation();
        tokio::select! {
            response = self.process_inner(policy_id, request) => response,
            _ = cancellation.changed() => Err(LlmError::Cancelled),
        }
    }

    async fn process_inner(
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
            if !self.bypass_endpoint_circuit {
                if let Some(until) = self.endpoint_unavailable_until(&provider.endpoint) {
                    failures.push(format!(
                        "{} ({}): endpoint unavailable until {}",
                        provider.profile.id,
                        provider.profile.model,
                        until.to_rfc3339()
                    ));
                    continue;
                }
            }
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
                let mut request = request.clone();
                if matches!(request.kind, ProcessKind::Chat) {
                    request.reasoning_effort = provider
                        .profile
                        .chat_reasoning_effort
                        .request_value()
                        .map(str::to_string);
                }
                request.litellm_reasoning_passthrough = request.reasoning_effort.is_some()
                    && self.runtime.requires_reasoning_passthrough(
                        &provider.endpoint,
                        &provider.profile.model,
                    );
                let slot_timeout = provider.profile.timeout_secs.max(1);
                let response = match tokio::time::timeout(
                    Duration::from_secs(slot_timeout),
                    self.runtime.acquire(
                        &provider.endpoint,
                        provider.effective_max_concurrent_requests,
                    ),
                )
                .await
                {
                    Ok(_permit) => {
                        let response = client.process(request.clone()).await;
                        match response {
                            Err(error)
                                if request.reasoning_effort.is_some()
                                    && !request.litellm_reasoning_passthrough
                                    && error.requires_litellm_reasoning_passthrough() =>
                            {
                                self.runtime.enable_reasoning_passthrough(
                                    &provider.endpoint,
                                    &provider.profile.model,
                                );
                                request.litellm_reasoning_passthrough = true;
                                client.process(request).await
                            }
                            response => response,
                        }
                    }
                    Err(_) => Err(LlmError::Routing(format!(
                        "{} ({}) did not free an LLM request slot within {}s",
                        provider.profile.id, provider.profile.model, slot_timeout
                    ))),
                };
                match response {
                    Ok(response) => {
                        self.clear_rate_limit(&provider.profile.id);
                        self.clear_endpoint_unavailable(&provider.endpoint);
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
                        let connection_error = is_connection_error(&error);
                        let retryable = is_retryable(&error);
                        if connection_error {
                            self.mark_endpoint_unavailable(
                                &provider.endpoint,
                                &describe_error(&error),
                            );
                        }
                        failures.push(format!(
                            "{} ({}): {}",
                            provider.profile.id,
                            provider.profile.model,
                            describe_error(&error)
                        ));
                        if connection_error || !retryable || attempt + 1 >= retries {
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
            "All eligible LLM providers failed for policy '{}': {}",
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
                    kind: ProcessKind::Chat,
                    reasoning_effort: None,
                    litellm_reasoning_passthrough: false,
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

    pub fn input_token_budget(&self, policy_id: &str, reserved_completion_tokens: u32) -> usize {
        let policy = self.policy(policy_id);
        let (candidates, _) = self.candidates(&policy);
        let context_window = candidates
            .into_iter()
            .map(|provider| provider.profile.context_window_tokens as usize)
            .min()
            .unwrap_or(DEFAULT_CONTEXT_WINDOW_TOKENS as usize);
        context_window.saturating_sub(reserved_completion_tokens as usize)
    }

    pub fn max_concurrent_requests(&self, policy_id: &str) -> usize {
        self.policy_limit(policy_id, |profile| {
            profile.max_concurrent_requests as usize
        })
    }

    /// Effective completion budget for a rule decision, always capped.
    ///
    /// A missing per-rule budget falls back to the configured global default,
    /// clamped to the decision ceiling so a stale large `llm.max_tokens`
    /// cannot produce an unbounded request.
    pub fn decision_max_tokens(&self, decision_max_tokens: Option<u32>) -> u32 {
        decision_max_tokens
            .unwrap_or(self.max_tokens)
            .clamp(MIN_DECISION_MAX_TOKENS, MAX_DECISION_MAX_TOKENS)
    }

    pub fn decision_context_reserve_tokens(&self, decision_max_tokens: Option<u32>) -> u32 {
        self.decision_max_tokens(decision_max_tokens)
    }

    pub fn output_tokens_per_second(&self, policy_id: &str) -> Option<f64> {
        let policy = self.policy(policy_id);
        self.candidates(&policy)
            .0
            .first()
            .map(|provider| provider.profile.output_tokens_per_second)
            .filter(|rate| rate.is_finite() && *rate > 0.0)
    }

    pub fn available_request_slots(&self, policy_id: &str) -> Option<(u32, u32)> {
        let policy = self.policy(policy_id);
        let provider = self.candidates(&policy).0.into_iter().next()?;
        let capacity = provider.effective_max_concurrent_requests;
        Some((
            self.runtime
                .available_permits(&provider.endpoint, capacity)
                .min(u32::MAX as usize) as u32,
            capacity.min(u32::MAX as usize) as u32,
        ))
    }

    pub fn wake_retry_worker(&self) {
        self.runtime.wake_retry_worker();
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

    fn policy_limit(&self, policy_id: &str, value: impl Fn(&LlmProviderProfile) -> usize) -> usize {
        let policy = self.policy(policy_id);
        let (candidates, _) = self.candidates(&policy);
        candidates
            .into_iter()
            .map(|candidate| value(&candidate.profile).max(1))
            .min()
            .unwrap_or(1)
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
            if !self.bypass_endpoint_circuit {
                if let Some(until) = self.endpoint_unavailable_until(&provider.endpoint) {
                    unavailable.push(format!(
                        "{}: endpoint unavailable until {}",
                        label,
                        until.to_rfc3339()
                    ));
                    continue;
                }
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

    fn endpoint_unavailable_until(&self, endpoint: &str) -> Option<DateTime<Utc>> {
        let status = self
            .database
            .as_ref()?
            .with_llm_endpoint_status(|repo| repo.get(endpoint).ok().flatten())?;
        status
            .unavailable_until
            .as_deref()
            .and_then(parse_rfc3339)
            .filter(|until| *until > Utc::now())
    }

    fn mark_endpoint_unavailable(&self, endpoint: &str, error: &str) {
        let Some(database) = &self.database else {
            return;
        };
        let until = Utc::now() + chrono::Duration::seconds(ENDPOINT_UNAVAILABLE_SECS);
        if let Err(error) = database.with_llm_endpoint_status(|repo| {
            repo.mark_unavailable(endpoint, &until.to_rfc3339(), error)
        }) {
            tracing::warn!(
                "Failed to open LLM endpoint circuit for {}: {}",
                endpoint,
                error
            );
        }
    }

    fn clear_endpoint_unavailable(&self, endpoint: &str) {
        if let Some(database) = &self.database {
            if let Err(error) = database.with_llm_endpoint_status(|repo| repo.clear(endpoint)) {
                tracing::warn!(
                    "Failed to close LLM endpoint circuit for {}: {}",
                    endpoint,
                    error
                );
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
        max_concurrent_requests: config.llm_legacy_max_concurrent_requests,
        output_tokens_per_second: config.llm_legacy_output_tokens_per_second,
        chat_reasoning_effort: config.llm_legacy_chat_reasoning_effort,
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
            async_openai::error::OpenAIError::StreamError(_) => true,
            async_openai::error::OpenAIError::ApiError(response) => {
                response.status_code.as_u16() >= 500 || response.status_code.as_u16() == 429
            }
            _ => false,
        },
        LlmError::NoResponse => true,
        LlmError::ParseError(_) | LlmError::Routing(_) | LlmError::Cancelled => false,
    }
}

fn is_connection_error(error: &LlmError) -> bool {
    matches!(
        error,
        LlmError::Api(async_openai::error::OpenAIError::Reqwest(error)) if error.is_connect()
    )
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

fn default_max_concurrent_requests() -> u8 {
    1
}

fn default_chat_reasoning_effort() -> ReasoningEffort {
    ReasoningEffort::ServerDefault
}

fn default_true() -> bool {
    true
}

fn endpoint_key(base_url: &str) -> String {
    base_url.trim().trim_end_matches('/').to_ascii_lowercase()
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
    fn reasoning_effort_uses_none_to_disable_thinking() {
        assert_eq!(ReasoningEffort::Off.request_value(), Some("none"));
        assert_eq!(ReasoningEffort::ServerDefault.request_value(), None);
        assert_eq!(ReasoningEffort::Low.request_value(), Some("low"));
    }

    #[test]
    fn decisions_reserve_their_configured_budget() {
        let router = InferenceRouter::from_config(&AppConfig::default());

        assert_eq!(
            router.decision_context_reserve_tokens(None),
            AppConfig::default().llm_max_tokens
        );
        assert_eq!(router.decision_context_reserve_tokens(Some(4_096)), 4_096);
    }

    #[test]
    fn decisions_are_always_capped_at_8k() {
        let router = InferenceRouter::from_config(&AppConfig {
            llm_max_tokens: 32_767,
            ..AppConfig::default()
        });

        assert_eq!(router.decision_max_tokens(None), 8_192);
        assert_eq!(router.decision_max_tokens(Some(1_024)), 1_024);
        assert_eq!(router.decision_max_tokens(Some(32_767)), 8_192);
        assert_eq!(router.decision_max_tokens(Some(0)), 64);
        assert_eq!(
            router.decision_context_reserve_tokens(None),
            router.decision_max_tokens(None)
        );
    }

    #[test]
    fn same_endpoint_shares_one_permit_across_accounts() {
        use crate::llm::InferenceRuntime;

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let runtime = InferenceRuntime::default();
            let endpoint = "http://shade:4000/v1";
            let _permit = runtime.acquire(endpoint, 1).await;
            assert_eq!(runtime.available_permits(endpoint, 1), 0);
            assert_eq!(runtime.available_permits("http://other:4000/v1", 1), 1);
        });
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
    fn retries_stream_transport_failures() {
        let error = LlmError::Api(async_openai::error::OpenAIError::StreamError(Box::new(
            async_openai::error::StreamError::EventStream("connection reset".into()),
        )));

        assert!(is_retryable(&error));
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
    fn endpoint_circuit_blocks_automatic_routing_but_not_a_manual_probe() {
        let db = Database::open(Path::new(":memory:")).unwrap();
        db.migrate().unwrap();
        db.with_llm_endpoint_status(|repo| {
            repo.mark_unavailable(
                "http://localhost:11434/v1",
                &(Utc::now() + chrono::Duration::minutes(5)).to_rfc3339(),
                "connection refused",
            )
        })
        .unwrap();
        let router = InferenceRouter::from_config(&AppConfig::default()).with_database(db);

        assert!(router.endpoint_circuit_open("default"));
        assert!(!router
            .with_endpoint_circuit_bypass()
            .endpoint_circuit_open("default"));
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
