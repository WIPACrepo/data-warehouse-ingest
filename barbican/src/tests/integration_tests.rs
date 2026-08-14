use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
    routing::{delete, get, post},
};
use chrono::Utc;
use mockall::predicate;
use mongodb::bson::{doc, oid::ObjectId};
use std::sync::Arc;
use tower::ServiceExt;

use crate::{
    MockInboxStore,
    CreateInbox, InboxDB, InboxStatus,
};
use crate::handlers::{create_inbox, get_inbox, delete_inbox, publish_inbox};

/// End-to-end test for full inbox lifecycle
#[tokio::test]
async fn test_full_inbox_lifecycle() {
    let mut mock_store = MockInboxStore::new();

    let inbox_id = ObjectId::new();
    let now = Utc::now();
    
    let create_inbox_data = CreateInbox {
        name: "lifecycle-test".to_string(),
        size: 250,
        warehouse_path: "/test/lifecycle".to_string(),
        lifetime: None,
        archive_enabled: false,
        archive_sites: vec![],
    };
    
    let expected_inbox = InboxDB {
        id: Some(inbox_id),
        name: create_inbox_data.name.clone(),
        size: create_inbox_data.size,
        status: InboxStatus::Active,
        warehouse_path: create_inbox_data.warehouse_path.clone(),
        lifetime: None,
        archive_enabled: false,
        archive_sites: vec![],
        created_at: now,
        updated_at: now,
    };

    // Setup mock expectations for create
    // First find_one to check for duplicates (returns None)
    mock_store
        .expect_find_one()
        .with(predicate::eq(doc! { "warehouse_path": "/test/lifecycle" }))
        .times(1)
        .returning(move |_| Ok(None));
    
    // Then insert_one
    mock_store
        .expect_insert_one()
        .with(predicate::always())
        .times(1)
        .returning(move |_| Ok(mongodb::results::InsertOneResult::default()));

    // Setup mock expectations for get
    mock_store
        .expect_find_one()
        .with(predicate::eq(doc! { "_id": inbox_id }))
        .times(1)
        .returning(move |_| Ok(Some(expected_inbox.clone())));

    let store = Arc::new(mock_store);
    let app = Router::new()
        .route("/api/v1/inbox", post(create_inbox))
        .route("/api/v1/inbox/{id}", get(get_inbox))
        .with_state(store);

    // Step 1: Create inbox
    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/inbox")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_string(&create_inbox_data).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(create_response.status(), StatusCode::OK);

    // Step 2: Retrieve inbox
    let get_response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(&format!("/api/v1/inbox/{}", inbox_id.to_hex()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(get_response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(get_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let retrieved_inbox: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(retrieved_inbox["name"], "lifecycle-test");
    assert_eq!(retrieved_inbox["size"], 250);
    assert_eq!(retrieved_inbox["status"], "active");
}

/// Test delete operation
#[tokio::test]
async fn test_delete_inbox() {
    let mut mock_store = MockInboxStore::new();

    let inbox_id = ObjectId::new();
    let now = Utc::now();
    
    let inbox = InboxDB {
        id: Some(inbox_id),
        name: "delete-test".to_string(),
        size: 100,
        status: InboxStatus::Active,
        warehouse_path: "/test/delete".to_string(),
        lifetime: None,
        archive_enabled: false,
        archive_sites: vec![],
        created_at: now,
        updated_at: now,
    };

    mock_store
        .expect_find_one()
        .with(predicate::eq(doc! { "_id": inbox_id }))
        .times(1)
        .returning(move |_| Ok(Some(inbox.clone())));

    mock_store
        .expect_update_one()
        .with(predicate::always(), predicate::always())
        .times(1)
        .returning(|_, _| Ok(mongodb::results::UpdateResult::default()));

    let store = Arc::new(mock_store);
    let app = Router::new()
        .route("/api/v1/inbox/{id}", delete(delete_inbox))
        .with_state(store);

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(&format!("/api/v1/inbox/{}", inbox_id.to_hex()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

/// Test publish operation
#[tokio::test]
async fn test_publish_inbox() {
    let mut mock_store = MockInboxStore::new();

    let inbox_id = ObjectId::new();
    let now = Utc::now();
    
    let inbox = InboxDB {
        id: Some(inbox_id),
        name: "publish-test".to_string(),
        size: 100,
        status: InboxStatus::Active,
        warehouse_path: "/test/publish".to_string(),
        lifetime: None,
        archive_enabled: false,
        archive_sites: vec![],
        created_at: now,
        updated_at: now,
    };

    mock_store
        .expect_find_one()
        .with(predicate::eq(doc! { "_id": inbox_id }))
        .times(1)
        .returning(move |_| Ok(Some(inbox.clone())));

    mock_store
        .expect_update_one()
        .with(predicate::always(), predicate::always())
        .times(1)
        .returning(|_, _| Ok(mongodb::results::UpdateResult::default()));

    let store = Arc::new(mock_store);
    let app = Router::new()
        .route("/api/v1/inbox/{id}/publish", post(publish_inbox))
        .with_state(store);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&format!("/api/v1/inbox/{}/publish", inbox_id.to_hex()))
                .header("Content-Type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}