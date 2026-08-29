//! Authentication configuration

use serde::Deserialize;
use std::collections::HashSet;

use crate::auth::error::ConfigError;

/// Authentication configuration (common for all services)
#[derive(Debug, Clone, Deserialize)]
pub struct AuthConfig {
    pub keycloak_url: String,
    pub keycloak_realm: String,
    pub keycloak_client_id: String,
    pub required_roles: Vec<String>,
    pub allowed_algorithms: HashSet<String>,
    pub jwks_refresh_secs: u64,
    pub leeway_ms: u64,
}

impl AuthConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let keycloak_url =
            std::env::var("KEYCLOAK_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());
        let keycloak_realm =
            std::env::var("KEYCLOAK_REALM").unwrap_or_else(|_| "data-warehouse".to_string());
        let keycloak_client_id =
            std::env::var("KEYCLOAK_CLIENT_ID").unwrap_or_else(|_| "default-client".to_string());
        let required_roles = std::env::var("REQUIRED_ROLES")
            .unwrap_or_else(|_| "read,write".to_string())
            .split(',')
            .map(|s| s.trim().to_string())
            .collect();
        let allowed_algorithms = std::env::var("ALLOWED_ALGORITHMS")
            .unwrap_or_else(|_| "RS256".to_string())
            .split(',')
            .map(|s| s.trim().to_string())
            .collect();
        let jwks_refresh_secs: u64 = std::env::var("JWKS_REFRESH_SECS")
            .unwrap_or_else(|_| "60".to_string())
            .parse()
            .unwrap_or(60);
        let leeway_ms: u64 = std::env::var("TOKEN_LEEWAY_MS")
            .unwrap_or_else(|_| "5000".to_string())
            .parse()
            .unwrap_or(5000);

        let config = Self {
            keycloak_url,
            keycloak_realm,
            keycloak_client_id,
            required_roles,
            allowed_algorithms,
            jwks_refresh_secs,
            leeway_ms,
        };

        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        // Check for insecure localhost in production
        if self.keycloak_url == "http://localhost:8080"
            && std::env::var("PRODUCTION").unwrap_or_else(|_| "false".to_string()) == "true"
        {
            return Err(ConfigError::InvalidValue(
                "KEYCLOAK_URL must not use localhost in production".to_string(),
            ));
        }

        // Validate algorithm whitelist
        if self.allowed_algorithms.is_empty() {
            return Err(ConfigError::InvalidValue(
                "At least one algorithm must be specified".to_string(),
            ));
        }

        // Reject insecure algorithms
        for alg in &self.allowed_algorithms {
            if alg.starts_with("HS") || alg == "none" {
                return Err(ConfigError::InvalidValue(format!(
                    "Symmetric algorithm '{}' not allowed",
                    alg
                )));
            }
        }

        // Validate refresh interval
        if self.jwks_refresh_secs < 60 {
            return Err(ConfigError::InvalidValue(
                "JWKS_REFRESH_SECS must be at least 60".to_string(),
            ));
        }

        // Validate leeway
        if self.leeway_ms > 60000 {
            return Err(ConfigError::InvalidValue(
                "TOKEN_LEEWAY_MS must not exceed 60000 (60 seconds)".to_string(),
            ));
        }

        Ok(())
    }

    pub fn jwks_url(&self) -> String {
        format!(
            "{}/auth/realms/{}/protocol/openid-connect/certs",
            self.keycloak_url, self.keycloak_realm
        )
    }

    pub fn issuer(&self) -> String {
        format!("{}/auth/realms/{}", self.keycloak_url, self.keycloak_realm)
    }

    pub fn audience(&self) -> String {
        self.keycloak_client_id.clone()
    }
}
