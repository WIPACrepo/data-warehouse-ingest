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
    /// Reserved for future use — MongoDB operation timeout (not yet wired to MongoOptions).
    pub mongodb_timeout_secs: u64,
    /// Reserved for future use — MongoDB retry limit (not yet wired into store layer).
    pub mongodb_retry_max: u32,
    
    // Keycloak Authentication
    pub keycloak_url: String,
    pub keycloak_realm: String,
    pub keycloak_client_id: String,
    pub keycloak_client_secret: String,
    pub required_roles: Vec<String>,
    pub allowed_algorithms: Vec<String>,
    pub jwks_refresh_secs: u64,
    
    // Citadel Integration
    pub citadel_api_url: String,
    pub citadel_api_timeout_secs: u64,
    pub citadel_api_retry_max: u32,
    pub citadel_service_token_file: Option<String>,
    
    // File System
    pub inbox_base_path: String,
    
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
            mongodb_timeout_secs: env::var("BARBICAN_MONGODB_TIMEOUT_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(30),
            mongodb_retry_max: env::var("BARBICAN_MONGODB_RETRY_MAX")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3),
            keycloak_url: env::var("BARBICAN_KEYCLOAK_URL")
                .unwrap_or_else(|_| "http://localhost:8080".to_string()),
            keycloak_realm: env::var("BARBICAN_KEYCLOAK_REALM")
                .unwrap_or_else(|_| "data-warehouse".to_string()),
            keycloak_client_id: env::var("BARBICAN_KEYCLOAK_CLIENT_ID")
                .unwrap_or_else(|_| "barbican".to_string()),
            keycloak_client_secret: env::var("BARBICAN_KEYCLOAK_CLIENT_SECRET")
                .unwrap_or_else(|_| "".to_string()),
            required_roles: env::var("BARBICAN_REQUIRED_ROLES")
                .unwrap_or_else(|_| "read,write".to_string())
                .split(',')
                .map(|s| s.trim().to_string())
                .collect(),
            allowed_algorithms: env::var("BARBICAN_ALLOWED_ALGORITHMS")
                .unwrap_or_else(|_| "RS256".to_string())
                .split(',')
                .map(|s| s.trim().to_string())
                .collect(),
            jwks_refresh_secs: env::var("BARBICAN_JWKS_REFRESH_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(60),
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
            inbox_base_path: env::var("BARBICAN_INBOX_BASE_PATH")
                .unwrap_or_else(|_| "./data/inboxes".to_string()),
            prometheus_port: env::var("BARBICAN_PROMETHEUS_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(9091),
        }
    }
}

impl Config {
    /// Load configuration from environment variables
    pub fn from_env() -> Result<Self, ConfigError> {
        let config = Config::default();
        config.validate()?;
        Ok(config)
    }

    /// Validate configuration values
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.keycloak_client_secret.is_empty() {
            return Err(ConfigError::MissingRequired(
                "BARBICAN_KEYCLOAK_CLIENT_SECRET".to_string(),
            ));
        }

        if self.allowed_algorithms.iter().any(|alg| alg == "none") {
            return Err(ConfigError::InvalidValue(
                "BARBICAN_ALLOWED_ALGORITHMS".to_string(),
                "Algorithm 'none' is not allowed".to_string(),
            ));
        }

        Ok(())
    }

    /// Get the Keycloak JWKS URL
    pub fn jwks_url(&self) -> String {
        format!(
            "{}/realms/{}/protocol/openid-connect/certs",
            self.keycloak_url, self.keycloak_realm
        )
    }

    /// Get the Keycloak token URL
    pub fn token_url(&self) -> String {
        format!(
            "{}/realms/{}/protocol/openid-connect/token",
            self.keycloak_url, self.keycloak_realm
        )
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