use serde::Deserialize;
use std::env;

/// Application configuration loaded from environment variables
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    // HTTP Server
    pub bind_address: String,

    // MongoDB
    pub mongodb_uri: String,
    pub mongodb_database: String,
    pub mongodb_inbox_collection: String,

    // Keycloak authentication is *not* parsed here: it is loaded by
    // `common::AuthConfig::from_env_prefixed("BARBICAN")` (decision D10), which
    // reads `BARBICAN_KEYCLOAK_*`, `BARBICAN_REQUIRED_ROLES`, etc.

    // Citadel Integration. The Bearer token for Citadel is configured via
    // `common::ServiceTokenSource::from_env("BARBICAN", "CITADEL", ..)`:
    // BARBICAN_CITADEL_CLIENT_ID/_CLIENT_SECRET or _SERVICE_TOKEN_FILE.
    pub citadel_api_url: String,
    pub citadel_api_timeout_secs: u64,
    pub citadel_api_retry_max: u32,

    // Observability
    pub prometheus_port: u16,
}

impl Default for Config {
    /// Built-in defaults only — no environment access (tests rely on this).
    fn default() -> Self {
        Self {
            bind_address: "127.0.0.1:8080".to_string(),
            mongodb_uri: "mongodb://localhost:27017".to_string(),
            mongodb_database: "data_warehouse".to_string(),
            mongodb_inbox_collection: "inboxes".to_string(),
            citadel_api_url: "http://citadel:8081".to_string(),
            citadel_api_timeout_secs: 30,
            citadel_api_retry_max: 3,
            prometheus_port: 9091,
        }
    }
}

impl Config {
    /// Load configuration from environment variables (`BARBICAN_*`).
    ///
    /// Auth settings are loaded separately via
    /// `common::AuthConfig::from_env_prefixed("BARBICAN")`; the Citadel
    /// service token via `common::ServiceTokenSource::from_env`.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| env::var(name).ok())
    }

    /// Defaults overridden by any variables `lookup` returns. Malformed
    /// numbers are an error naming the variable (they used to fall back to
    /// the default silently).
    pub fn from_lookup<F>(lookup: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        fn parse<T: std::str::FromStr>(name: &str, raw: String) -> Result<T, ConfigError> {
            raw.trim()
                .parse()
                .map_err(|_| ConfigError::InvalidValue(name.to_string(), raw))
        }

        let mut config = Self::default();
        if let Some(v) = lookup("BARBICAN_BIND_ADDRESS") {
            config.bind_address = v;
        }
        if let Some(v) = lookup("BARBICAN_MONGODB_URI") {
            config.mongodb_uri = v;
        }
        if let Some(v) = lookup("BARBICAN_MONGODB_DATABASE") {
            config.mongodb_database = v;
        }
        if let Some(v) = lookup("BARBICAN_MONGODB_INBOX_COLLECTION") {
            config.mongodb_inbox_collection = v;
        }
        if let Some(v) = lookup("BARBICAN_CITADEL_API_URL") {
            config.citadel_api_url = v;
        }
        if let Some(v) = lookup("BARBICAN_CITADEL_API_TIMEOUT_SECS") {
            config.citadel_api_timeout_secs = parse("BARBICAN_CITADEL_API_TIMEOUT_SECS", v)?;
        }
        if let Some(v) = lookup("BARBICAN_CITADEL_API_RETRY_MAX") {
            config.citadel_api_retry_max = parse("BARBICAN_CITADEL_API_RETRY_MAX", v)?;
        }
        if let Some(v) = lookup("BARBICAN_PROMETHEUS_PORT") {
            config.prometheus_port = parse("BARBICAN_PROMETHEUS_PORT", v)?;
        }
        Ok(config)
    }
}

/// Configuration errors
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Missing required environment variable: {0}")]
    MissingRequired(String),

    #[error("Invalid value for {0}: {1}")]
    InvalidValue(String, String),

    #[error("Configuration validation failed: {0}")]
    ValidationFailed(String),
}
