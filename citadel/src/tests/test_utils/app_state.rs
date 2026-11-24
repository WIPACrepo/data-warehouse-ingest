//! AppState builder for router-level tests.

use std::sync::Arc;

use common::AuthMiddlewareState;

use crate::AppState;
use crate::config::CitadelConfig;
use crate::integrations::barbican::{InboxTransitioner, MockInboxTransitioner};
use crate::metrics::CitadelMetrics;
use crate::storage::manager::StorageManager;

/// Callback secret configured in [`test_app_state`].
pub const TEST_CALLBACK_SECRET: &str = "test-callback-secret";

/// MongoDB URI that fails fast: nothing listens on port 1 and server
/// selection gives up after 200 ms. A request that reaches the database
/// therefore returns `database_error`, which router tests use as proof that
/// authentication *passed* without needing a running MongoDB.
pub const UNREACHABLE_MONGO_URI: &str = "mongodb://127.0.0.1:1/?serverSelectionTimeoutMS=200";

/// Build an `AppState` with the given auth state and Barbican client.
pub async fn test_app_state_with(
    auth_state: Arc<AuthMiddlewareState>,
    barbican: Arc<dyn InboxTransitioner>,
) -> AppState {
    let config = CitadelConfig {
        job_executor: crate::config::JobExecutorType::Mock,
        ..CitadelConfig::default()
    };
    let db_client = mongodb::Client::with_uri_str(UNREACHABLE_MONGO_URI)
        .await
        .expect("client construction is lazy");
    AppState {
        config: Arc::new(config.clone()),
        auth_state,
        storage_manager: Arc::new(StorageManager::new(&config)),
        barbican_client: barbican,
        executor: Arc::new(crate::jobs::executor::MockExecutor::new(config.clone())),
        catalog_notifier: Arc::new(crate::integrations::catalog::NoOpCatalogNotifier),
        lta_notifier: Arc::new(crate::integrations::lta::NoOpLtaNotifier),
        metrics: Arc::new(CitadelMetrics::new().expect("metrics init")),
        db_client: Arc::new(db_client),
        callback_secret: Arc::new(tokio::sync::RwLock::new(Some(
            TEST_CALLBACK_SECRET.to_string(),
        ))),
    }
}

/// [`test_app_state_with`] using a Barbican mock with no expectations.
pub async fn test_app_state(auth_state: Arc<AuthMiddlewareState>) -> AppState {
    test_app_state_with(auth_state, Arc::new(MockInboxTransitioner::new())).await
}
