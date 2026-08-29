//! Tests for AuthConfig

#[cfg(test)]
mod tests {
    use crate::auth::config::AuthConfig;
    use crate::auth::error::ConfigError;
    use std::collections::HashSet;

    fn clear_auth_env() {
        unsafe {
            std::env::remove_var("KEYCLOAK_URL");
            std::env::remove_var("KEYCLOAK_REALM");
            std::env::remove_var("KEYCLOAK_CLIENT_ID");
            std::env::remove_var("REQUIRED_ROLES");
            std::env::remove_var("ALLOWED_ALGORITHMS");
            std::env::remove_var("JWKS_REFRESH_SECS");
            std::env::remove_var("TOKEN_LEEWAY_MS");
            std::env::remove_var("PRODUCTION");
        }
    }

    #[test]
    fn test_from_env_with_defaults() {
        clear_auth_env();
        let config = AuthConfig::from_env().expect("Should create config with defaults");

        assert_eq!(config.keycloak_url, "http://localhost:8080");
        assert_eq!(config.keycloak_realm, "data-warehouse");
        assert_eq!(config.keycloak_client_id, "default-client");
        assert_eq!(config.required_roles, vec!["read", "write"]);
        assert!(config.allowed_algorithms.contains("RS256"));
        assert_eq!(config.jwks_refresh_secs, 60);
        assert_eq!(config.leeway_ms, 5000);
    }

    #[test]
    fn test_from_env_with_custom_values() {
        clear_auth_env();
        unsafe {
            std::env::set_var("KEYCLOAK_URL", "http://keycloak.example.com");
            std::env::set_var("KEYCLOAK_REALM", "my-realm");
            std::env::set_var("KEYCLOAK_CLIENT_ID", "my-client");
            std::env::set_var("REQUIRED_ROLES", "admin,user");
            std::env::set_var("ALLOWED_ALGORITHMS", "RS256,RS384");
            std::env::set_var("JWKS_REFRESH_SECS", "120");
            std::env::set_var("TOKEN_LEEWAY_MS", "10000");
        }

        let config = AuthConfig::from_env().expect("Should create config with custom values");

        assert_eq!(config.keycloak_url, "http://keycloak.example.com");
        assert_eq!(config.keycloak_realm, "my-realm");
        assert_eq!(config.keycloak_client_id, "my-client");
        assert_eq!(config.required_roles, vec!["admin", "user"]);
        assert!(config.allowed_algorithms.contains("RS256"));
        assert!(config.allowed_algorithms.contains("RS384"));
        assert_eq!(config.jwks_refresh_secs, 120);
        assert_eq!(config.leeway_ms, 10000);

        clear_auth_env();
    }

    #[test]
    fn test_validate_rejects_localhost_in_production() {
        clear_auth_env();
        unsafe {
            std::env::set_var("PRODUCTION", "true");
        }

        let config = AuthConfig {
            keycloak_url: "http://localhost:8080".to_string(),
            keycloak_realm: "data-warehouse".to_string(),
            keycloak_client_id: "test-client".to_string(),
            required_roles: vec!["read".to_string()],
            allowed_algorithms: vec!["RS256".to_string()].into_iter().collect(),
            jwks_refresh_secs: 60,
            leeway_ms: 5000,
        };

        let result = config.validate();
        assert!(matches!(result, Err(ConfigError::InvalidValue(_))));

        clear_auth_env();
    }

    #[test]
    fn test_validate_accepts_localhost_in_non_production() {
        clear_auth_env();

        let config = AuthConfig {
            keycloak_url: "http://localhost:8080".to_string(),
            keycloak_realm: "data-warehouse".to_string(),
            keycloak_client_id: "test-client".to_string(),
            required_roles: vec!["read".to_string()],
            allowed_algorithms: vec!["RS256".to_string()].into_iter().collect(),
            jwks_refresh_secs: 60,
            leeway_ms: 5000,
        };

        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_rejects_empty_algorithms() {
        clear_auth_env();
        let config = AuthConfig {
            keycloak_url: "http://localhost:8080".to_string(),
            keycloak_realm: "data-warehouse".to_string(),
            keycloak_client_id: "test-client".to_string(),
            required_roles: vec!["read".to_string()],
            allowed_algorithms: HashSet::new(),
            jwks_refresh_secs: 60,
            leeway_ms: 5000,
        };

        let result = config.validate();
        assert!(matches!(result, Err(ConfigError::InvalidValue(_))));
    }

    #[test]
    fn test_validate_rejects_symmetric_algorithms() {
        clear_auth_env();
        let mut algorithms = HashSet::new();
        algorithms.insert("HS256".to_string());

        let config = AuthConfig {
            keycloak_url: "http://localhost:8080".to_string(),
            keycloak_realm: "data-warehouse".to_string(),
            keycloak_client_id: "test-client".to_string(),
            required_roles: vec!["read".to_string()],
            allowed_algorithms: algorithms,
            jwks_refresh_secs: 60,
            leeway_ms: 5000,
        };

        let result = config.validate();
        assert!(matches!(result, Err(ConfigError::InvalidValue(_))));
    }

    #[test]
    fn test_validate_rejects_none_algorithm() {
        clear_auth_env();
        let mut algorithms = HashSet::new();
        algorithms.insert("none".to_string());

        let config = AuthConfig {
            keycloak_url: "http://localhost:8080".to_string(),
            keycloak_realm: "data-warehouse".to_string(),
            keycloak_client_id: "test-client".to_string(),
            required_roles: vec!["read".to_string()],
            allowed_algorithms: algorithms,
            jwks_refresh_secs: 60,
            leeway_ms: 5000,
        };

        let result = config.validate();
        assert!(matches!(result, Err(ConfigError::InvalidValue(_))));
    }

    #[test]
    fn test_validate_rejects_low_refresh_interval() {
        clear_auth_env();
        let config = AuthConfig {
            keycloak_url: "http://localhost:8080".to_string(),
            keycloak_realm: "data-warehouse".to_string(),
            keycloak_client_id: "test-client".to_string(),
            required_roles: vec!["read".to_string()],
            allowed_algorithms: vec!["RS256".to_string()].into_iter().collect(),
            jwks_refresh_secs: 30,
            leeway_ms: 5000,
        };

        let result = config.validate();
        assert!(matches!(result, Err(ConfigError::InvalidValue(_))));
    }

    #[test]
    fn test_validate_rejects_high_leeway() {
        clear_auth_env();
        let config = AuthConfig {
            keycloak_url: "http://localhost:8080".to_string(),
            keycloak_realm: "data-warehouse".to_string(),
            keycloak_client_id: "test-client".to_string(),
            required_roles: vec!["read".to_string()],
            allowed_algorithms: vec!["RS256".to_string()].into_iter().collect(),
            jwks_refresh_secs: 60,
            leeway_ms: 70000,
        };

        let result = config.validate();
        assert!(matches!(result, Err(ConfigError::InvalidValue(_))));
    }

    #[test]
    fn test_jwks_url_generation() {
        clear_auth_env();
        let config = AuthConfig {
            keycloak_url: "http://keycloak.example.com".to_string(),
            keycloak_realm: "my-realm".to_string(),
            keycloak_client_id: "test-client".to_string(),
            required_roles: vec!["read".to_string()],
            allowed_algorithms: vec!["RS256".to_string()].into_iter().collect(),
            jwks_refresh_secs: 60,
            leeway_ms: 5000,
        };

        assert_eq!(
            config.jwks_url(),
            "http://keycloak.example.com/auth/realms/my-realm/protocol/openid-connect/certs"
        );
    }

    #[test]
    fn test_issuer_generation() {
        clear_auth_env();
        let config = AuthConfig {
            keycloak_url: "http://keycloak.example.com".to_string(),
            keycloak_realm: "my-realm".to_string(),
            keycloak_client_id: "test-client".to_string(),
            required_roles: vec!["read".to_string()],
            allowed_algorithms: vec!["RS256".to_string()].into_iter().collect(),
            jwks_refresh_secs: 60,
            leeway_ms: 5000,
        };

        assert_eq!(
            config.issuer(),
            "http://keycloak.example.com/auth/realms/my-realm"
        );
    }

    #[test]
    fn test_audience_generation() {
        clear_auth_env();
        let config = AuthConfig {
            keycloak_url: "http://keycloak.example.com".to_string(),
            keycloak_realm: "my-realm".to_string(),
            keycloak_client_id: "my-client-id".to_string(),
            required_roles: vec!["read".to_string()],
            allowed_algorithms: vec!["RS256".to_string()].into_iter().collect(),
            jwks_refresh_secs: 60,
            leeway_ms: 5000,
        };

        assert_eq!(config.audience(), "my-client-id");
    }
}
