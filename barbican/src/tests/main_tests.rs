use std::sync::Arc;

use crate::{AppState, Config, create_app};
use common::test_utils::create_test_store;
use common::{AuthConfig, AuthMiddlewareState, JwksCache, JwtValidator};

#[tokio::test]
async fn test_health_endpoint() {
    let mock_store = Arc::new(create_test_store());
    let config = Config::default();

    // Initialize common auth components for testing
    let auth_config = AuthConfig {
        keycloak_url: "http://localhost:8080".to_string(),
        keycloak_realm: "test".to_string(),
        keycloak_client_id: "test-client".to_string(),
        required_roles: vec!["read".to_string()],
        allowed_algorithms: vec!["RS256".to_string()].into_iter().collect(),
        jwks_refresh_secs: 60,
        leeway_ms: 5000,
    };
    let jwks_cache = Arc::new(JwksCache::new(
        auth_config.jwks_url(),
        auth_config.jwks_refresh_secs,
    ));
    let validator = Arc::new(JwtValidator::new(jwks_cache, auth_config));
    let auth_state = Arc::new(AuthMiddlewareState {
        validator,
        required_roles: config.required_roles.clone(),
        service_name: "barbican".to_string(),
    });

    let metrics = Arc::new(crate::metrics::BarbicanMetrics::new().unwrap());

    let state = Arc::new(AppState {
        store: mock_store,
        citadel_client: None,
        config,
        auth_state,
        metrics,
    });

    let router = create_app(state);

    let server = axum_test::TestServer::new(router).unwrap();

    // Health endpoint should be accessible without auth
    let response = server.get("/health").await;
    response.assert_status_ok();
}

/// D9/P4.6: Barbican's 12 internal `*/{start,progress,complete,failure}`
/// transition routes are Bearer-only (no role gate) but still require a valid
/// JWT. An unauthenticated request must get a 401 with a JSON error body
/// (D7), not a bare status code and certainly not 200.
#[tokio::test]
async fn test_internal_action_route_requires_bearer_token() {
    let mock_store = Arc::new(create_test_store());
    let config = Config::default();

    let auth_config = AuthConfig {
        keycloak_url: "http://localhost:8080".to_string(),
        keycloak_realm: "test".to_string(),
        keycloak_client_id: "test-client".to_string(),
        required_roles: vec!["read".to_string()],
        allowed_algorithms: vec!["RS256".to_string()].into_iter().collect(),
        jwks_refresh_secs: 60,
        leeway_ms: 5000,
    };
    let jwks_cache = Arc::new(JwksCache::new(
        auth_config.jwks_url(),
        auth_config.jwks_refresh_secs,
    ));
    let validator = Arc::new(JwtValidator::new(jwks_cache, auth_config));
    let auth_state = Arc::new(AuthMiddlewareState {
        validator,
        required_roles: config.required_roles.clone(),
        service_name: "barbican".to_string(),
    });

    let metrics = Arc::new(crate::metrics::BarbicanMetrics::new().unwrap());

    let state = Arc::new(AppState {
        store: mock_store,
        citadel_client: None,
        config,
        auth_state,
        metrics,
    });

    let router = create_app(state);
    let server = axum_test::TestServer::new(router).unwrap();

    // No Authorization header → 401, not 200/403-without-body.
    let response = server
        .post("/api/v1/inbox/000000000000000000000000/publish/start")
        .await;

    let status = response.status_code();
    assert_eq!(
        status, 401,
        "internal route must reject unauthenticated calls"
    );

    // D7: the rejection body is JSON {error, message}, not an empty shell.
    let body = response.text();
    let json: serde_json::Value = serde_json::from_str(&body)
        .unwrap_or_else(|_| panic!("expected JSON error body, got: {body}"));
    assert_eq!(json["error"], "auth_error");
    assert!(
        json["message"].as_str().map_or(false, |m| !m.is_empty()),
        "auth_error message must be non-empty"
    );
}

/// D9/P4.6: an invalid Bearer token (not a valid JWT) is also rejected with a
/// 401 JSON body on an internal route.
#[tokio::test]
async fn test_internal_action_route_rejects_invalid_bearer_token() {
    let mock_store = Arc::new(create_test_store());
    let config = Config::default();

    let auth_config = AuthConfig {
        keycloak_url: "http://localhost:8080".to_string(),
        keycloak_realm: "test".to_string(),
        keycloak_client_id: "test-client".to_string(),
        required_roles: vec!["read".to_string()],
        allowed_algorithms: vec!["RS256".to_string()].into_iter().collect(),
        jwks_refresh_secs: 60,
        leeway_ms: 5000,
    };
    let jwks_cache = Arc::new(JwksCache::new(
        auth_config.jwks_url(),
        auth_config.jwks_refresh_secs,
    ));
    let validator = Arc::new(JwtValidator::new(jwks_cache, auth_config));
    let auth_state = Arc::new(AuthMiddlewareState {
        validator,
        required_roles: config.required_roles.clone(),
        service_name: "barbican".to_string(),
    });

    let metrics = Arc::new(crate::metrics::BarbicanMetrics::new().unwrap());

    let state = Arc::new(AppState {
        store: mock_store,
        citadel_client: None,
        config,
        auth_state,
        metrics,
    });

    let router = create_app(state);
    let server = axum_test::TestServer::new(router).unwrap();

    let response = server
        .post("/api/v1/inbox/000000000000000000000000/publish/start")
        .authorization_bearer("definitely.not.a.jwt")
        .await;

    let status = response.status_code();
    assert_eq!(status, 401, "invalid bearer must be rejected with 401");

    let body = response.text();
    let json: serde_json::Value = serde_json::from_str(&body)
        .unwrap_or_else(|_| panic!("expected JSON error body, got: {body}"));
    assert_eq!(json["error"], "auth_error");
    assert!(
        json["message"].as_str().map_or(false, |m| !m.is_empty()),
        "auth_error message must be non-empty"
    );
}
