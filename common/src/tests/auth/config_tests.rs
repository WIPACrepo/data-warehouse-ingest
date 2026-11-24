//! Tests for AuthConfig
//!
//! All loading tests use `AuthConfig::from_lookup` with an in-memory map so
//! they never mutate the process environment (which raced under parallel
//! test execution with the old `from_env`).

#[cfg(test)]
mod tests {
    use crate::auth::config::AuthConfig;
    use crate::auth::error::ConfigError;
    use std::collections::{HashMap, HashSet};

    /// Build a lookup closure over a fixed set of variables.
    fn lookup(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    fn base_config() -> AuthConfig {
        AuthConfig {
            keycloak_url: "http://keycloak.example.com".to_string(),
            keycloak_realm: "my-realm".to_string(),
            keycloak_client_id: "test-client".to_string(),
            required_roles: vec!["read".to_string()],
            allowed_algorithms: vec!["RS256".to_string()].into_iter().collect(),
            jwks_refresh_secs: 60,
            leeway_ms: 5000,
        }
    }

    #[test]
    fn test_from_lookup_defaults() {
        let config = AuthConfig::from_lookup("BARBICAN", lookup(&[])).unwrap();

        assert_eq!(config.keycloak_url, "http://localhost:8080");
        assert_eq!(config.keycloak_realm, "data-warehouse");
        // Default audience is the lowercased prefix, not a placeholder.
        assert_eq!(config.keycloak_client_id, "barbican");
        assert_eq!(config.required_roles, vec!["read", "write"]);
        assert!(config.allowed_algorithms.contains("RS256"));
        assert_eq!(config.jwks_refresh_secs, 60);
        assert_eq!(config.leeway_ms, 5000);
    }

    #[test]
    fn test_from_lookup_reads_prefixed_names() {
        let config = AuthConfig::from_lookup(
            "CITADEL",
            lookup(&[
                ("CITADEL_KEYCLOAK_URL", "https://kc.example.com"),
                ("CITADEL_KEYCLOAK_REALM", "my-realm"),
                ("CITADEL_KEYCLOAK_CLIENT_ID", "my-client"),
                ("CITADEL_REQUIRED_ROLES", "admin, user"),
                ("CITADEL_ALLOWED_ALGORITHMS", "RS256,RS384"),
                ("CITADEL_JWKS_REFRESH_SECS", "120"),
                ("CITADEL_TOKEN_LEEWAY_MS", "10000"),
            ]),
        )
        .unwrap();

        assert_eq!(config.keycloak_url, "https://kc.example.com");
        assert_eq!(config.keycloak_realm, "my-realm");
        assert_eq!(config.keycloak_client_id, "my-client");
        assert_eq!(config.required_roles, vec!["admin", "user"]);
        assert!(config.allowed_algorithms.contains("RS384"));
        assert_eq!(config.jwks_refresh_secs, 120);
        assert_eq!(config.leeway_ms, 10000);
    }

    #[test]
    fn test_from_lookup_ignores_unprefixed_and_other_prefix() {
        let config = AuthConfig::from_lookup(
            "BARBICAN",
            lookup(&[
                ("KEYCLOAK_URL", "https://wrong.example.com"),
                ("CITADEL_KEYCLOAK_URL", "https://citadel.example.com"),
                ("KEYCLOAK_CLIENT_ID", "default-client"),
            ]),
        )
        .unwrap();
        assert_eq!(config.keycloak_url, "http://localhost:8080");
        assert_eq!(config.keycloak_client_id, "barbican");
    }

    #[test]
    fn test_from_lookup_trims_trailing_slash() {
        let config = AuthConfig::from_lookup(
            "BARBICAN",
            lookup(&[("BARBICAN_KEYCLOAK_URL", "https://kc.example.com/")]),
        )
        .unwrap();
        assert_eq!(config.keycloak_url, "https://kc.example.com");
    }

    #[test]
    fn test_from_lookup_rejects_malformed_number_naming_variable() {
        let err =
            AuthConfig::from_lookup("CITADEL", lookup(&[("CITADEL_TOKEN_LEEWAY_MS", "five")]))
                .unwrap_err();
        assert!(err.to_string().contains("CITADEL_TOKEN_LEEWAY_MS"), "{err}");
    }

    #[test]
    fn test_from_lookup_rejects_empty_required_roles() {
        let err =
            AuthConfig::from_lookup("BARBICAN", lookup(&[("BARBICAN_REQUIRED_ROLES", " , ")]))
                .unwrap_err();
        assert!(err.to_string().contains("BARBICAN_REQUIRED_ROLES"), "{err}");
    }

    #[test]
    fn test_from_lookup_rejects_localhost_in_production() {
        let err =
            AuthConfig::from_lookup("BARBICAN", lookup(&[("PRODUCTION", "true")])).unwrap_err();
        assert!(err.to_string().contains("BARBICAN_KEYCLOAK_URL"), "{err}");
    }

    #[test]
    fn test_validate_accepts_localhost_in_non_production() {
        let mut config = base_config();
        config.keycloak_url = "http://localhost:8080".to_string();
        assert!(config.validate_with(false, "X").is_ok());
    }

    #[test]
    fn test_validate_rejects_empty_algorithms() {
        let mut config = base_config();
        config.allowed_algorithms = HashSet::new();
        assert!(matches!(
            config.validate_with(false, "X"),
            Err(ConfigError::InvalidValue(_))
        ));
    }

    #[test]
    fn test_validate_rejects_symmetric_algorithms() {
        let mut config = base_config();
        config.allowed_algorithms = ["HS256".to_string()].into_iter().collect();
        assert!(matches!(
            config.validate_with(false, "X"),
            Err(ConfigError::InvalidValue(_))
        ));
    }

    #[test]
    fn test_validate_rejects_none_algorithm() {
        let mut config = base_config();
        config.allowed_algorithms = ["none".to_string()].into_iter().collect();
        assert!(matches!(
            config.validate_with(false, "X"),
            Err(ConfigError::InvalidValue(_))
        ));
    }

    #[test]
    fn test_validate_rejects_low_refresh_interval() {
        let mut config = base_config();
        config.jwks_refresh_secs = 30;
        assert!(matches!(
            config.validate_with(false, "X"),
            Err(ConfigError::InvalidValue(_))
        ));
    }

    #[test]
    fn test_validate_rejects_high_leeway() {
        let mut config = base_config();
        config.leeway_ms = 70000;
        assert!(matches!(
            config.validate_with(false, "X"),
            Err(ConfigError::InvalidValue(_))
        ));
    }

    #[test]
    fn test_jwks_url_has_no_hardcoded_auth_segment() {
        assert_eq!(
            base_config().jwks_url(),
            "http://keycloak.example.com/realms/my-realm/protocol/openid-connect/certs"
        );
    }

    #[test]
    fn test_issuer_has_no_hardcoded_auth_segment() {
        assert_eq!(
            base_config().issuer(),
            "http://keycloak.example.com/realms/my-realm"
        );
    }

    #[test]
    fn test_legacy_auth_prefix_kept_from_base_url() {
        let config = AuthConfig::from_lookup(
            "CITADEL",
            lookup(&[
                ("CITADEL_KEYCLOAK_URL", "https://kc.example.com/auth/"),
                ("CITADEL_KEYCLOAK_REALM", "IceCube"),
            ]),
        )
        .unwrap();
        assert_eq!(
            config.issuer(),
            "https://kc.example.com/auth/realms/IceCube"
        );
        assert_eq!(
            config.jwks_url(),
            "https://kc.example.com/auth/realms/IceCube/protocol/openid-connect/certs"
        );
    }

    /// jsonwebtoken's leeway is in seconds; ms must be converted (rounding up).
    #[test]
    fn test_leeway_secs_converts_from_ms() {
        let mut config = base_config();
        config.leeway_ms = 5000;
        assert_eq!(config.leeway_secs(), 5);
        config.leeway_ms = 1500;
        assert_eq!(config.leeway_secs(), 2);
        config.leeway_ms = 0;
        assert_eq!(config.leeway_secs(), 0);
        config.leeway_ms = 60_000;
        assert_eq!(config.leeway_secs(), 60);
    }

    #[test]
    fn test_audience_generation() {
        assert_eq!(base_config().audience(), "test-client");
    }

    #[tokio::test]
    async fn test_build_auth_state_uses_config_roles() {
        let mut config = base_config();
        config.required_roles = vec!["write".to_string()];
        let state = crate::auth::build_auth_state(&config, "citadel");
        assert_eq!(state.required_roles, vec!["write"]);
        assert_eq!(state.service_name, "citadel");
    }
}
