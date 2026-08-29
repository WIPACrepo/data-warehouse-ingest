use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    routing::{delete, get, post},
};
use chrono::Utc;
use mockall::predicate;
use mongodb::bson::{doc, oid::ObjectId};
use std::sync::Arc;
use tower::ServiceExt;

use crate::handlers::{create_inbox, delete_inbox, get_inbox, publish_inbox};
use crate::{AppState, Config, CreateInbox, InboxDB, InboxStatus};
use common::{AuthConfig, AuthMiddlewareState, JwksCache, JwtValidator, MockInboxStore};

/// Helper to create test AppState
fn create_test_state(store: Arc<MockInboxStore>) -> Arc<AppState> {
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

    Arc::new(AppState {
        store,
        citadel_client: None,
        config,
        auth_state,
        metrics,
    })
}

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
        status: InboxStatus::Creating,
        warehouse_path: create_inbox_data.warehouse_path.clone(),
        inbox_path: None,
        lifetime: None,
        archive_enabled: false,
        archive_sites: vec![],
        version: 0,
        current_job_id: None,
        progress: None,
        error: None,
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
    let state = create_test_state(store.clone());
    let app = Router::new()
        .route("/api/v1/inbox", post(create_inbox))
        .route("/api/v1/inbox/{id}", get(get_inbox))
        .with_state(state);

    // Step 1: Create inbox
    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/inbox")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&create_inbox_data).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(create_response.status(), StatusCode::ACCEPTED);

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
    assert_eq!(retrieved_inbox["status"], "creating");
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
        status: InboxStatus::Ready,
        warehouse_path: "/test/delete".to_string(),
        inbox_path: None,
        lifetime: None,
        archive_enabled: false,
        archive_sites: vec![],
        version: 0,
        current_job_id: None,
        progress: None,
        error: None,
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
        .returning(|_, _| {
            let mut res = mongodb::results::UpdateResult::default();
            res.matched_count = 1;
            res.modified_count = 1;
            Ok(res)
        });

    let store = Arc::new(mock_store);
    let state = create_test_state(store.clone());
    let app = Router::new()
        .route("/api/v1/inbox/{id}", delete(delete_inbox))
        .with_state(state);

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

    assert_eq!(response.status(), StatusCode::ACCEPTED);
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
        status: InboxStatus::Ready,
        warehouse_path: "/test/publish".to_string(),
        inbox_path: None,
        lifetime: None,
        archive_enabled: false,
        archive_sites: vec![],
        version: 0,
        current_job_id: None,
        progress: None,
        error: None,
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
        .returning(|_, _| {
            let mut res = mongodb::results::UpdateResult::default();
            res.matched_count = 1;
            res.modified_count = 1;
            Ok(res)
        });

    let store = Arc::new(mock_store);
    let state = create_test_state(store.clone());
    let app = Router::new()
        .route("/api/v1/inbox/{id}/publish", post(publish_inbox))
        .with_state(state);

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

    assert_eq!(response.status(), StatusCode::ACCEPTED);
}
