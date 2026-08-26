//! Tests for auth middleware

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::Request,
    };

    use crate::auth::claims::{Claims, ResourceAccess};
    use crate::auth::config::AuthConfig;
    use crate::auth::jwks_cache::JwksCache;
    use crate::auth::jwt_validator::JwtValidator;
    use crate::auth::middleware::{AuthMiddlewareState, ClaimsExtractor};
    use std::collections::HashMap;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_claims_extractor_trait() {
        let mut resource_access = HashMap::new();
        resource_access.insert(
            "test-service".to_string(),
            ResourceAccess {
                roles: vec!["read".to_string()],
            },
        );

        let claims = Claims {
            sub: "test-user".to_string(),
            iss: "http://localhost:8080/auth/realms/test".to_string(),
            aud: "test-client".to_string(),
            exp: 9999999999,
            iat: 0,
            resource_access: Some(resource_access),
        };

        let mut req = Request::builder().body(Body::empty()).unwrap();
        req.extensions_mut().insert(claims.clone());

        let extracted = req.claims();
        assert!(extracted.is_some());
        assert_eq!(extracted.unwrap().sub, "test-user");
    }

    #[tokio::test]
    async fn test_claims_extractor_returns_none_when_missing() {
        let req = Request::builder().body(Body::empty()).unwrap();
        let extracted = req.claims();
        assert!(extracted.is_none());
    }

    #[tokio::test]
    async fn test_auth_middleware_state_creation() {
        let jwks_cache = Arc::new(JwksCache::new(
            "http://localhost:8080/auth/realms/test/protocol/openid-connect/certs".to_string(),
            60,
        ));

        let config = AuthConfig {
            keycloak_url: "http://localhost:8080".to_string(),
            keycloak_realm: "test".to_string(),
            keycloak_client_id: "test-client".to_string(),
            required_roles: vec!["read".to_string()],
            allowed_algorithms: vec!["RS256".to_string()].into_iter().collect(),
            jwks_refresh_secs: 60,
            leeway_ms: 5000,
        };

        let validator = Arc::new(JwtValidator::new(jwks_cache, config));

        let state = AuthMiddlewareState {
            validator,
            required_roles: vec!["read".to_string()],
            service_name: "test-service".to_string(),
        };

        assert_eq!(state.service_name, "test-service");
        assert_eq!(state.required_roles, vec!["read"]);
    }

    #[tokio::test]
    async fn test_missing_auth_header_detection() {
        let req = Request::builder()
            .uri("/")
            .body(Body::empty())
            .unwrap();

        let auth_header = req
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok());

        assert!(auth_header.is_none());
    }

    #[tokio::test]
    async fn test_invalid_auth_header_format_detection() {
        let req = Request::builder()
            .uri("/")
            .header(axum::http::header::AUTHORIZATION, "InvalidFormat")
            .body(Body::empty())
            .unwrap();

        let auth_header = req
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok());

        assert!(auth_header.is_some());
        let header_value = auth_header.unwrap();
        assert!(!header_value.starts_with("Bearer "));
    }

    #[tokio::test]
    async fn test_valid_bearer_token_format() {
        let req = Request::builder()
            .uri("/")
            .header(axum::http::header::AUTHORIZATION, "Bearer token123")
            .body(Body::empty())
            .unwrap();

        let auth_header = req
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok());

        assert!(auth_header.is_some());
        let header_value = auth_header.unwrap();
        assert!(header_value.starts_with("Bearer "));
        
        let token = &header_value[7..];
        assert_eq!(token, "token123");
    }
}