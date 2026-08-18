//! Integration tests for Citadel — requires MongoDB running locally.
//!
//! Run with: `cargo test -p citadel --test integration --features integration-tests`

#![cfg(feature = "integration-tests")]

use citadel::config::CitadelConfig;
use citadel::models::JobDB;
use citadel::jobs::types::{JobType, JobStatus};
use mongodb::bson::doc;
use mongodb::bson::oid::ObjectId;

/// Verify database connectivity and job collection CRUD
#[tokio::test]
async fn test_db_lifecycle() {
    let config = CitadelConfig::from_env().unwrap_or_default();
    let client = common::db_setup(&config.mongodb_uri, &config.mongodb_database)
        .await
        .expect("Failed to connect to MongoDB");

    let jobs_coll = client
        .database(&config.mongodb_database)
        .collection::<JobDB>("jobs");

    // Insert
    let job = JobDB::new(
        "integration-test-job".to_string(),
        ObjectId::new(),
        JobType::PublishInbox {
            inbox_id: ObjectId::new(),
            notify_catalog: true,
            notify_lta: true,
        },
        "integration",
    );
    let insert_result = jobs_coll.insert_one(job).await;
    assert!(insert_result.is_ok(), "Failed to insert job: {:?}", insert_result.err());

    // Query
    let found = jobs_coll
        .find_one(doc! { "job_id": "integration-test-job" })
        .await
        .expect("Failed to query job");
    assert!(found.is_some(), "Job not found after insert");
    assert_eq!(found.unwrap().status, JobStatus::Queued);

    // Cleanup
    jobs_coll
        .delete_one(doc! { "job_id": "integration-test-job" })
        .await
        .expect("Failed to delete job");
}

/// Verify callback_nonces collection CRUD
#[tokio::test]
async fn test_nonce_lifecycle() {
    let config = CitadelConfig::from_env().unwrap_or_default();
    let client = common::db_setup(&config.mongodb_uri, &config.mongodb_database)
        .await
        .expect("Failed to connect to MongoDB");

    let nonces_coll = client
        .database(&config.mongodb_database)
        .collection::<citadel::models::CallbackNonceDB>("callback_nonces");

    let nonce = citadel::models::CallbackNonceDB::new("test-nonce-123".to_string());
    nonces_coll
        .insert_one(nonce)
        .await
        .expect("Failed to insert nonce");

    let found = nonces_coll
        .find_one(doc! { "nonce": "test-nonce-123" })
        .await
        .expect("Failed to query nonce");
    assert!(found.is_some(), "Nonce not found after insert");

    // Replay detection
    let replay = nonces_coll
        .find_one(doc! { "nonce": "test-nonce-123" })
        .await
        .expect("Failed to detect replay");
    assert!(replay.is_some(), "Replay nonce should exist");

    // Cleanup
    nonces_coll
        .delete_one(doc! { "nonce": "test-nonce-123" })
        .await
        .expect("Failed to delete nonce");
}