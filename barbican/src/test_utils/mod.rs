pub use common::test_utils::*;

#[cfg(test)]
pub use common::MockInboxStore;

#[cfg(test)]
use std::sync::Arc;
use crate::{AppState, Config};
use crate::auth::JwtValidator;

#[cfg(test)]
pub fn create_test_state(store: Arc<MockInboxStore>) -> Arc<AppState> {
    let config = Config::default();
    let jwt_validator = Arc::new(JwtValidator::new(config.clone()));
    Arc::new(AppState {
        store,
        citadel_client: None,
        config,
        jwt_validator,
    })
}