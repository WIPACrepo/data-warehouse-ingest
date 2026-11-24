//! Unit tests for `handlers::compensate` (no MongoDB: the job-status update
//! hits an unreachable server and must be swallowed).

use std::sync::Arc;

use mongodb::bson::oid::ObjectId;

use super::test_utils::test_app_state_with;
use crate::error::AppError;
use crate::handlers::compensate::{Operation, codes, compensate};
use crate::integrations::barbican::MockInboxTransitioner;

fn auth_state() -> Arc<common::AuthMiddlewareState> {
    let cfg = common::AuthConfig::from_lookup("CITADEL", |_| None).unwrap();
    common::build_auth_state(&cfg, "citadel")
}

fn inbox() -> common::InboxDB {
    common::test_utils::create_test_inbox_with_id(ObjectId::new(), "x", 0)
}

#[tokio::test]
async fn test_create_calls_create_failure_with_code_and_message() {
    let id = ObjectId::new();
    let mut mock = MockInboxTransitioner::new();
    mock.expect_create_failure()
        .withf(move |i, job, code, msg| {
            i == &id && job == "job-1" && code == codes::STORAGE_ERROR && msg.contains("disk full")
        })
        .times(1)
        .returning(|_, _, _, _| Box::pin(async { Ok(inbox()) }));
    let state = test_app_state_with(auth_state(), Arc::new(mock)).await;

    let err = AppError::InternalError("disk full".into());
    compensate(
        &state,
        Operation::Create,
        &id,
        "job-1",
        codes::STORAGE_ERROR,
        &err,
    )
    .await;
}

#[tokio::test]
async fn test_publish_calls_publish_failure_and_tolerates_db_error() {
    let id = ObjectId::new();
    let mut mock = MockInboxTransitioner::new();
    mock.expect_publish_failure()
        .withf(|_, job, code, _| job == "job-2" && code == codes::EXECUTOR_ERROR)
        .times(1)
        .returning(|_, _, _, _| Box::pin(async { Ok(inbox()) }));
    let state = test_app_state_with(auth_state(), Arc::new(mock)).await;

    // MongoDB is unreachable: marking the job failed fails and is only logged.
    let err = AppError::K8sApiError("submit failed".into());
    compensate(
        &state,
        Operation::Publish,
        &id,
        "job-2",
        codes::EXECUTOR_ERROR,
        &err,
    )
    .await;
}

#[tokio::test]
async fn test_delete_barbican_failure_is_swallowed() {
    let id = ObjectId::new();
    let mut mock = MockInboxTransitioner::new();
    mock.expect_delete_failure()
        .times(1)
        .returning(|_, _, _, _| {
            Box::pin(async { Err(AppError::BadGateway("barbican down".into())) })
        });
    let state = test_app_state_with(auth_state(), Arc::new(mock)).await;

    let err = AppError::K8sApiError("submit failed".into());
    // Must return normally (no panic, nothing raised).
    compensate(
        &state,
        Operation::Delete,
        &id,
        "job-3",
        codes::EXECUTOR_ERROR,
        &err,
    )
    .await;
}
