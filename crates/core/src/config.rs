use serde::{Deserialize, Serialize};

use crate::db::config::ConfigRepository;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub gmail_account: Option<String>,
    pub google_client_id: String,
    pub llm_base_url: String,
    pub llm_api_key: String,
    pub llm_default_model: String,
    pub llm_temperature: f32,
    pub llm_max_tokens: u32,
    pub polling_query: String,
    pub polling_interval_minutes: u32,
    pub polling_max_per_cycle: u32,
    pub polling_enabled: bool,
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
            polling_query: "is:unread".into(),
            polling_interval_minutes: 5,
            polling_max_per_cycle: 100,
            polling_enabled: true,
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
            gmail_account: repo.get("gmail.account").ok().flatten(),
            google_client_id: get("google.client_id", ""),
            llm_base_url: get("llm.base_url", &Self::default().llm_base_url),
            llm_api_key: get("llm.api_key", &Self::default().llm_api_key),
            llm_default_model: get("llm.default_model", &Self::default().llm_default_model),
            llm_temperature: get("llm.temperature", "0.3").parse().unwrap_or(0.3),
            llm_max_tokens: get("llm.max_tokens", "1024").parse().unwrap_or(1024),
            polling_query: get("polling.query", &Self::default().polling_query),
            polling_interval_minutes: get("polling.interval_minutes", "5").parse().unwrap_or(5),
            polling_max_per_cycle: get("polling.max_per_cycle", "100").parse().unwrap_or(100),
            polling_enabled: get("polling.enabled", "true").parse().unwrap_or(true),
            tray_theme: get("ui.tray_theme", "auto"),
        }
    }

    pub fn save(&self, repo: &ConfigRepository) -> rusqlite::Result<()> {
        let set = |key: &str, value: &str| -> rusqlite::Result<()> { repo.set(key, value) };

        if let Some(ref account) = self.gmail_account {
            set("gmail.account", account)?;
        }
        set("google.client_id", &self.google_client_id)?;
        set("llm.base_url", &self.llm_base_url)?;
        set("llm.api_key", &self.llm_api_key)?;
        set("llm.default_model", &self.llm_default_model)?;
        set("llm.temperature", &self.llm_temperature.to_string())?;
        set("llm.max_tokens", &self.llm_max_tokens.to_string())?;
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
        set("ui.tray_theme", &self.tray_theme)?;
        Ok(())
    }
}
