use std::sync::Arc;

use crate::{AppState, Config, create_app};
use common::{AuthConfig, JwksCache, JwtValidator, AuthMiddlewareState};
use common::test_utils::create_test_store;

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
    
    let state = Arc::new(AppState {
        store: mock_store,
        citadel_client: None,
        config,
        auth_state,
    });
    
    let router = create_app(state);

    let server = axum_test::TestServer::new(router).unwrap();

    // Health endpoint should be accessible without auth
    let response = server.get("/health").await;
    response.assert_status_ok();
}