pub use common::test_utils::*;

#[cfg(test)]
pub use common::MockInboxStore;

use crate::{AppState, Config};
#[cfg(test)]
use std::sync::Arc;

#[cfg(test)]
pub fn create_test_state(store: Arc<MockInboxStore>) -> Arc<AppState> {
    let config = Config::default();

    let auth_state = crate::test_utils::test_auth_state();

    let metrics = Arc::new(crate::metrics::BarbicanMetrics::new().unwrap());

    Arc::new(AppState {
        store,
        citadel_client: None,
        config,
        auth_state,
        metrics,
    })
}

/// Auth middleware state for tests: a validator pointed at an unreachable
/// localhost Keycloak and the default external roles (`read`, `write`).
#[cfg(test)]
pub fn test_auth_state() -> Arc<common::AuthMiddlewareState> {
    let auth_config = common::AuthConfig {
        keycloak_url: "http://localhost:8080".to_string(),
        keycloak_realm: "test".to_string(),
        keycloak_client_id: "test-client".to_string(),
        required_roles: vec!["read".to_string(), "write".to_string()],
        allowed_algorithms: vec!["RS256".to_string()].into_iter().collect(),
        jwks_refresh_secs: 60,
        leeway_ms: 5000,
    };
    common::build_auth_state(&auth_config, "barbican")
}
