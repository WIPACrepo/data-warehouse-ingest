//! Authentication configuration
//!
//! Parsing lives here so every service validates JWTs identically, but each
//! service owns its own environment-variable namespace: the service passes a
//! prefix (e.g. `"BARBICAN"`) and this module reads `{PREFIX}_KEYCLOAK_URL`,
//! `{PREFIX}_REQUIRED_ROLES`, etc. See `common/src/docs/auth.md` (decision D10).

use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Arc;

use crate::auth::error::ConfigError;
use crate::auth::{AuthMiddlewareState, JwksCache, JwtValidator};

/// Default Keycloak base URL (development only; rejected when `PRODUCTION=true`).
pub const DEFAULT_KEYCLOAK_URL: &str = "http://localhost:8080";
/// Default Keycloak realm.
pub const DEFAULT_KEYCLOAK_REALM: &str = "data-warehouse";
/// Default required roles (any one grants access; `admin` always passes).
pub const DEFAULT_REQUIRED_ROLES: &str = "read,write";
/// Default JWT signature algorithm allow-list.
pub const DEFAULT_ALLOWED_ALGORITHMS: &str = "RS256";
/// Default JWKS refresh interval (seconds); also the minimum allowed.
pub const DEFAULT_JWKS_REFRESH_SECS: u64 = 60;
/// Default clock-skew leeway for `exp`/`nbf` (milliseconds).
pub const DEFAULT_TOKEN_LEEWAY_MS: u64 = 5000;

/// Authentication configuration (common for all services)
#[derive(Debug, Clone, Deserialize)]
pub struct AuthConfig {
    /// Keycloak base URL, *including* any path prefix such as `/auth` used by
    /// legacy (< 17) Keycloak deployments. No trailing slash.
    pub keycloak_url: String,
    pub keycloak_realm: String,
    /// Expected `aud` claim. Keycloak only puts the client ID in `aud` when an
    /// audience mapper is configured on the client.
    pub keycloak_client_id: String,
    /// Roles accepted by [`crate::auth::auth_middleware`]; empty disables the
    /// role gate (valid Bearer token only).
    pub required_roles: Vec<String>,
    pub allowed_algorithms: HashSet<String>,
    pub jwks_refresh_secs: u64,
    pub leeway_ms: u64,
}

impl AuthConfig {
    /// Load from the process environment using `{prefix}_*` variable names.
    pub fn from_env_prefixed(prefix: &str) -> Result<Self, ConfigError> {
        Self::from_lookup(prefix, |name| std::env::var(name).ok())
    }

    /// Load using an arbitrary variable lookup (testable without mutating the
    /// process environment).
    ///
    /// Reads `{prefix}_KEYCLOAK_URL`, `{prefix}_KEYCLOAK_REALM`,
    /// `{prefix}_KEYCLOAK_CLIENT_ID`, `{prefix}_REQUIRED_ROLES`,
    /// `{prefix}_ALLOWED_ALGORITHMS`, `{prefix}_JWKS_REFRESH_SECS`,
    /// `{prefix}_TOKEN_LEEWAY_MS`, plus the global `PRODUCTION` flag.
    pub fn from_lookup<F>(prefix: &str, lookup: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let var = |suffix: &str| format!("{}_{}", prefix, suffix);
        let get = |suffix: &str| lookup(&var(suffix));
        let get_u64 = |suffix: &str, default: u64| -> Result<u64, ConfigError> {
            match get(suffix) {
                None => Ok(default),
                Some(raw) => raw.trim().parse().map_err(|_| {
                    ConfigError::InvalidValue(format!(
                        "{} must be a non-negative integer, got '{}'",
                        var(suffix),
                        raw
                    ))
                }),
            }
        };

        let keycloak_url = get("KEYCLOAK_URL")
            .unwrap_or_else(|| DEFAULT_KEYCLOAK_URL.to_string())
            .trim_end_matches('/')
            .to_string();
        let keycloak_realm =
            get("KEYCLOAK_REALM").unwrap_or_else(|| DEFAULT_KEYCLOAK_REALM.to_string());
        // Default audience is the service's own name (e.g. "barbican").
        let keycloak_client_id =
            get("KEYCLOAK_CLIENT_ID").unwrap_or_else(|| prefix.to_ascii_lowercase());
        let required_roles = split_list(
            &get("REQUIRED_ROLES").unwrap_or_else(|| DEFAULT_REQUIRED_ROLES.to_string()),
        );
        // An empty list disables the role gate in `auth_middleware`. That mode
        // is only for routes a service deliberately builds in code (D9), never
        // something an env typo should be able to switch on.
        if required_roles.is_empty() {
            return Err(ConfigError::InvalidValue(format!(
                "{} must name at least one role",
                var("REQUIRED_ROLES")
            )));
        }
        let allowed_algorithms = split_list(
            &get("ALLOWED_ALGORITHMS").unwrap_or_else(|| DEFAULT_ALLOWED_ALGORITHMS.to_string()),
        )
        .into_iter()
        .collect();
        let jwks_refresh_secs = get_u64("JWKS_REFRESH_SECS", DEFAULT_JWKS_REFRESH_SECS)?;
        let leeway_ms = get_u64("TOKEN_LEEWAY_MS", DEFAULT_TOKEN_LEEWAY_MS)?;

        let config = Self {
            keycloak_url,
            keycloak_realm,
            keycloak_client_id,
            required_roles,
            allowed_algorithms,
            jwks_refresh_secs,
            leeway_ms,
        };

        let production = lookup("PRODUCTION").is_some_and(|v| v == "true");
        config.validate_with(production, prefix)?;
        Ok(config)
    }

    /// Validate using the process-wide `PRODUCTION` flag.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let production = std::env::var("PRODUCTION").is_ok_and(|v| v == "true");
        self.validate_with(production, "<SERVICE>")
    }

    /// Validate; `prefix` is only used to name variables in error messages.
    pub fn validate_with(&self, production: bool, prefix: &str) -> Result<(), ConfigError> {
        if production && self.keycloak_url == DEFAULT_KEYCLOAK_URL {
            return Err(ConfigError::InvalidValue(format!(
                "{}_KEYCLOAK_URL must not use localhost in production",
                prefix
            )));
        }

        if self.allowed_algorithms.is_empty() {
            return Err(ConfigError::InvalidValue(format!(
                "{}_ALLOWED_ALGORITHMS must name at least one algorithm",
                prefix
            )));
        }

        // Reject symmetric/unsigned algorithms: Keycloak signs with RSA/EC keys.
        for alg in &self.allowed_algorithms {
            if alg.starts_with("HS") || alg == "none" {
                return Err(ConfigError::InvalidValue(format!(
                    "{}_ALLOWED_ALGORITHMS: symmetric algorithm '{}' not allowed",
                    prefix, alg
                )));
            }
        }

        if self.jwks_refresh_secs < DEFAULT_JWKS_REFRESH_SECS {
            return Err(ConfigError::InvalidValue(format!(
                "{}_JWKS_REFRESH_SECS must be at least {}",
                prefix, DEFAULT_JWKS_REFRESH_SECS
            )));
        }

        if self.leeway_ms > 60_000 {
            return Err(ConfigError::InvalidValue(format!(
                "{}_TOKEN_LEEWAY_MS must not exceed 60000 (60 seconds)",
                prefix
            )));
        }

        Ok(())
    }

    /// JWKS endpoint: `{keycloak_url}/realms/{realm}/protocol/openid-connect/certs`.
    pub fn jwks_url(&self) -> String {
        format!(
            "{}/realms/{}/protocol/openid-connect/certs",
            self.keycloak_url, self.keycloak_realm
        )
    }

    /// Expected `iss` claim: `{keycloak_url}/realms/{realm}`.
    pub fn issuer(&self) -> String {
        format!("{}/realms/{}", self.keycloak_url, self.keycloak_realm)
    }

    /// Token endpoint (client-credentials grant):
    /// `{keycloak_url}/realms/{realm}/protocol/openid-connect/token`.
    pub fn token_url(&self) -> String {
        format!(
            "{}/realms/{}/protocol/openid-connect/token",
            self.keycloak_url, self.keycloak_realm
        )
    }

    /// Clock-skew leeway in whole seconds, as `jsonwebtoken::Validation::leeway`
    /// expects. Rounded up so a non-zero millisecond setting never becomes 0.
    /// (Previously `leeway_ms` was passed through unconverted, so the 5000 ms
    /// default allowed 5000 s ≈ 83 min of expiry slack.)
    pub fn leeway_secs(&self) -> u64 {
        self.leeway_ms.div_ceil(1000)
    }

    /// Expected `aud` claim.
    pub fn audience(&self) -> String {
        self.keycloak_client_id.clone()
    }
}

/// Build the shared auth middleware state (JWKS cache → validator → state)
/// that every service previously assembled by hand.
pub fn build_auth_state(config: &AuthConfig, service_name: &str) -> Arc<AuthMiddlewareState> {
    let jwks_cache = Arc::new(JwksCache::new(config.jwks_url(), config.jwks_refresh_secs));
    let validator = Arc::new(JwtValidator::new(jwks_cache, config.clone()));
    Arc::new(AuthMiddlewareState {
        validator,
        required_roles: config.required_roles.clone(),
        service_name: service_name.to_string(),
    })
}

/// Split a comma-separated list, trimming whitespace and dropping empties.
fn split_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}
