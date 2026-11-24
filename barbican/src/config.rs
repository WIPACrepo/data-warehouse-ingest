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

    // Citadel Integration
    pub citadel_api_url: String,
    pub citadel_api_timeout_secs: u64,
    pub citadel_api_retry_max: u32,
    pub citadel_service_token_file: Option<String>,

    // Observability
    pub prometheus_port: u16,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind_address: env::var("BARBICAN_BIND_ADDRESS")
                .unwrap_or_else(|_| "127.0.0.1:8080".to_string()),
            mongodb_uri: env::var("BARBICAN_MONGODB_URI")
                .unwrap_or_else(|_| "mongodb://localhost:27017".to_string()),
            mongodb_database: env::var("BARBICAN_MONGODB_DATABASE")
                .unwrap_or_else(|_| "data_warehouse".to_string()),
            mongodb_inbox_collection: env::var("BARBICAN_MONGODB_INBOX_COLLECTION")
                .unwrap_or_else(|_| "inboxes".to_string()),
            citadel_api_url: env::var("BARBICAN_CITADEL_API_URL")
                .unwrap_or_else(|_| "http://citadel:8081".to_string()),
            citadel_api_timeout_secs: env::var("BARBICAN_CITADEL_API_TIMEOUT_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(30),
            citadel_api_retry_max: env::var("BARBICAN_CITADEL_API_RETRY_MAX")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3),
            citadel_service_token_file: env::var("BARBICAN_CITADEL_SERVICE_TOKEN_FILE").ok(),
            prometheus_port: env::var("BARBICAN_PROMETHEUS_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(9091),
        }
    }
}

impl Config {
    /// Load configuration from environment variables.
    ///
    /// Auth settings are loaded separately via
    /// `common::AuthConfig::from_env_prefixed("BARBICAN")`.
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Config::default())
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
