//! Integration tests (MongoDB) for failed publishes:
//! - executor submission fails → Barbican `publish_failure` is called and the
//!   job is marked failed (leaves the in-flight quota);
//! - Barbican `publish_start` fails → the queued job row is removed.
//!
//! Run with: `cargo test -p citadel --test compensation --features integration-tests`

#![cfg(feature = "integration-tests")]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Extension, Path, State};
use mongodb::bson::{doc, oid::ObjectId};

use citadel::AppState;
use citadel::config::CitadelConfig;
use citadel::error::{AppError, Result};
use citadel::handlers::compensate::codes;
use citadel::handlers::publish_inbox::{count_in_flight_jobs, publish_inbox};
use citadel::integrations::barbican::InboxTransitioner;
use citadel::jobs::executor::JobExecutor;
use citadel::jobs::types::JobStatusDetail;
use citadel::models::JobDB;
use common::auth::ResourceAccess;
use common::{Claims, InboxDB};

/// Records `(method, error_code)` for every call. All calls succeed except
/// `*_start` when `fail_start` is set.
#[derive(Default)]
struct RecordingTransitioner {
    calls: Mutex<Vec<(String, String)>>,
    fail_start: bool,
}

impl RecordingTransitioner {
    fn record(&self, method: &str, code: &str) -> Result<InboxDB> {
        self.calls
            .lock()
            .unwrap()
            .push((method.to_string(), code.to_string()));
        if self.fail_start && method.ends_with("_start") {
            return Err(AppError::Conflict("simulated start rejection".into()));
        }
        Ok(common::test_utils::create_test_inbox_with_id(
            ObjectId::new(),
            "x",
            0,
        ))
    }
}

#[async_trait::async_trait]
impl InboxTransitioner for RecordingTransitioner {
    async fn create_start(&self, _: &ObjectId, _: &str) -> Result<InboxDB> {
        self.record("create_start", "")
    }
    async fn create_progress(&self, _: &ObjectId, _: &str, _: &str, _: u64) -> Result<InboxDB> {
        self.record("create_progress", "")
    }
    async fn create_complete(&self, _: &ObjectId, _: &str, _: &str) -> Result<InboxDB> {
        self.record("create_complete", "")
    }
    async fn create_failure(&self, _: &ObjectId, _: &str, code: &str, _: &str) -> Result<InboxDB> {
        self.record("create_failure", code)
    }
    async fn publish_start(&self, _: &ObjectId, _: &str) -> Result<InboxDB> {
        self.record("publish_start", "")
    }
    async fn publish_progress(
        &self,
        _: &ObjectId,
        _: &str,
        _: &str,
        _: u64,
        _: u64,
        _: u64,
        _: u64,
    ) -> Result<InboxDB> {
        self.record("publish_progress", "")
    }
    async fn publish_complete(
        &self,
        _: &ObjectId,
        _: &str,
        _: u64,
        _: u64,
        _: &str,
    ) -> Result<InboxDB> {
        self.record("publish_complete", "")
    }
    async fn publish_failure(&self, _: &ObjectId, _: &str, code: &str, _: &str) -> Result<InboxDB> {
        self.record("publish_failure", code)
    }
    async fn delete_start(&self, _: &ObjectId, _: &str) -> Result<InboxDB> {
        self.record("delete_start", "")
    }
    async fn delete_progress(
        &self,
        _: &ObjectId,
        _: &str,
        _: &str,
        _: u64,
        _: u64,
    ) -> Result<InboxDB> {
        self.record("delete_progress", "")
    }
    async fn delete_complete(&self, _: &ObjectId, _: &str) -> Result<InboxDB> {
        self.record("delete_complete", "")
    }
    async fn delete_failure(&self, _: &ObjectId, _: &str, code: &str, _: &str) -> Result<InboxDB> {
        self.record("delete_failure", code)
    }
}

/// Executor whose submissions always fail.
struct FailingExecutor;

#[async_trait::async_trait]
impl JobExecutor for FailingExecutor {
    async fn submit(&self, _: &JobDB) -> Result<String> {
        Err(AppError::K8sApiError("simulated submit failure".into()))
    }
    async fn cancel(&self, _: &str) -> Result<()> {
        Ok(())
    }
    async fn status(&self, _: &str) -> Result<JobStatusDetail> {
        Err(AppError::K8sApiError("unused".into()))
    }
}

fn writer_claims() -> Claims {
    let mut ra = HashMap::new();
    ra.insert(
        "citadel".to_string(),
        ResourceAccess {
            roles: vec!["write".to_string()],
        },
    );
    Claims {
        sub: "tester".into(),
        iss: "iss".into(),
        aud: "citadel".into(),
        exp: usize::MAX,
        iat: 0,
        resource_access: Some(ra),
    }
}

/// Fresh database with one created inbox; returns the state, inbox id and client.
async fn setup(
    transitioner: Arc<RecordingTransitioner>,
) -> (AppState, ObjectId, mongodb::Client, CitadelConfig) {
    let config = CitadelConfig {
        mongodb_database: format!("citadel_compensation_{}", ObjectId::new()),
        ..CitadelConfig::from_env().unwrap_or_default()
    };
    let client = common::db_setup(&config.mongodb_uri, &config.mongodb_database)
        .await
        .expect("Failed to connect to MongoDB");

    // An inbox that has been created (has an inbox_path).
    let inbox_id = ObjectId::new();
    let mut inbox = common::test_utils::create_test_inbox_with_id(inbox_id, "comp", 0);
    inbox.inbox_path = Some("/data/inboxes/comp".into());
    citadel::db::inboxes_collection(&client, &config)
        .insert_one(&inbox)
        .await
        .unwrap();

    let auth_cfg = common::AuthConfig::from_lookup("CITADEL", |_| None).unwrap();
    let state = AppState {
        config: Arc::new(config.clone()),
        auth_state: common::build_auth_state(&auth_cfg, "citadel"),
        storage_manager: Arc::new(citadel::storage::manager::StorageManager::new(&config)),
        barbican_client: transitioner.clone(),
        executor: Arc::new(FailingExecutor),
        catalog_notifier: Arc::new(citadel::integrations::catalog::NoOpCatalogNotifier),
        lta_notifier: Arc::new(citadel::integrations::lta::NoOpLtaNotifier),
        metrics: Arc::new(citadel::metrics::CitadelMetrics::new().unwrap()),
        db_client: Arc::new(client.clone()),
        callback_secret: Arc::new(tokio::sync::RwLock::new(None)),
    };

    (state, inbox_id, client, config)
}

#[tokio::test]
async fn test_publish_submit_failure_is_compensated() {
    let transitioner = Arc::new(RecordingTransitioner::default());
    let (state, inbox_id, client, config) = setup(transitioner.clone()).await;

    let result = publish_inbox(
        State(state.clone()),
        Extension(writer_claims()),
        Path(inbox_id.to_hex()),
    )
    .await;

    // The original error is returned.
    assert!(matches!(result, Err(AppError::K8sApiError(_))));

    // Barbican saw publish_start then publish_failure(executor_error).
    let calls = transitioner.calls.lock().unwrap().clone();
    assert_eq!(
        calls,
        vec![
            ("publish_start".to_string(), String::new()),
            (
                "publish_failure".to_string(),
                codes::EXECUTOR_ERROR.to_string()
            ),
        ]
    );

    // The job is failed and no longer counts toward the quota.
    assert_eq!(
        count_in_flight_jobs(&state, "publish_inbox").await.unwrap(),
        0
    );
    let job = citadel::db::jobs_collection(&client, &config)
        .find_one(doc! { "inbox_id": inbox_id })
        .await
        .unwrap()
        .expect("job document");
    assert_eq!(job.status, citadel::jobs::types::JobStatus::Failed);

    client.database(&config.mongodb_database).drop().await.ok();
}

#[tokio::test]
async fn test_publish_start_failure_removes_queued_job() {
    let transitioner = Arc::new(RecordingTransitioner {
        fail_start: true,
        ..RecordingTransitioner::default()
    });
    let (state, inbox_id, client, config) = setup(transitioner.clone()).await;

    let result = publish_inbox(
        State(state.clone()),
        Extension(writer_claims()),
        Path(inbox_id.to_hex()),
    )
    .await;
    assert!(matches!(result, Err(AppError::Conflict(_))));

    // No failure transition (the inbox never left its state) ...
    let calls = transitioner.calls.lock().unwrap().clone();
    assert_eq!(calls, vec![("publish_start".to_string(), String::new())]);
    // ... and the queued job row is gone, so it cannot leak quota.
    assert_eq!(
        count_in_flight_jobs(&state, "publish_inbox").await.unwrap(),
        0
    );
    let job = citadel::db::jobs_collection(&client, &config)
        .find_one(doc! { "inbox_id": inbox_id })
        .await
        .unwrap();
    assert!(job.is_none());

    client.database(&config.mongodb_database).drop().await.ok();
}
