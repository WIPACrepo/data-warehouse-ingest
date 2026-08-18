use std::sync::Arc;

use crate::{AppState, Config, create_app};
use crate::auth::JwtValidator;
use common::test_utils::create_test_store;

#[tokio::test]
async fn test_health_endpoint() {
    let mock_store = Arc::new(create_test_store());
    let config = Config::default();
    let jwt_validator = Arc::new(JwtValidator::new(config.clone()));
    let state = Arc::new(AppState {
        store: mock_store,
        citadel_client: None,
        config,
        jwt_validator,
    });
    
    let router = create_app(state);

    let server = axum_test::TestServer::new(router).unwrap();

    // Health endpoint should be accessible without auth
    let response = server.get("/health").await;
    response.assert_status_ok();
}