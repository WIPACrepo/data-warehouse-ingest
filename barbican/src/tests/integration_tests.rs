use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
    routing::{get, post},
};
use mockall::predicate;
use mongodb::bson::{doc, oid::ObjectId};
use std::sync::Arc;
use tower::ServiceExt;

use crate::{
    MockInboxStore,
    CreateInbox, InboxDB,
};
use crate::handlers::{create_inbox, get_inbox};

/// End-to-end test for full inbox lifecycle
#[tokio::test]
async fn test_full_inbox_lifecycle() {
    let mut mock_store = MockInboxStore::new();

    let inbox_id = ObjectId::new();
    let create_inbox_data = CreateInbox {
        name: "lifecycle-test".to_string(),
        size: 250,
    };
    let expected_inbox = InboxDB {
        id: Some(inbox_id),
        name: create_inbox_data.name.clone(),
        size: create_inbox_data.size,
        active: true,
    };

    // Setup mock expectations for create
    // InsertOneResult is #[non_exhaustive], use default()
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
    assert_eq!(retrieved_inbox["active"], true);
}

/// Test concurrent operations
#[tokio::test]
async fn test_concurrent_operations() {
    let mut mock_store = MockInboxStore::new();

    let inbox_id_1 = ObjectId::new();
    let inbox_id_2 = ObjectId::new();

    // Setup mock for multiple creates (2 concurrent)
    mock_store
        .expect_insert_one()
        .times(2)
        .returning_st(move |_| Ok(mongodb::results::InsertOneResult::default()));

    // Setup mock for multiple finds
    let inbox_1 = InboxDB {
        id: Some(inbox_id_1),
        name: "concurrent-1".to_string(),
        size: 100,
        active: true,
    };
    let inbox_2 = InboxDB {
        id: Some(inbox_id_2),
        name: "concurrent-2".to_string(),
        size: 200,
        active: true,
    };

    mock_store
        .expect_find_one()
        .with(predicate::eq(doc! { "_id": inbox_id_1 }))
        .times(1)
        .returning(move |_| Ok(Some(inbox_1.clone())));

    mock_store
        .expect_find_one()
        .with(predicate::eq(doc! { "_id": inbox_id_2 }))
        .times(1)
        .returning(move |_| Ok(Some(inbox_2.clone())));

    let store = Arc::new(mock_store);
    let app = Router::new()
        .route("/api/v1/inbox", post(create_inbox))
        .route("/api/v1/inbox/{id}", get(get_inbox))
        .with_state(store);

    // Run concurrent requests
    let create_req_1 = create_inbox_request("concurrent-1", 100);
    let create_req_2 = create_inbox_request("concurrent-2", 200);

    let (resp1, resp2) = tokio::join!(
        app.clone().oneshot(create_req_1),
        app.clone().oneshot(create_req_2)
    );

    assert_eq!(resp1.unwrap().status(), StatusCode::OK);
    assert_eq!(resp2.unwrap().status(), StatusCode::OK);

    // Verify retrieves work
    let get_req_1 = Request::builder()
        .method("GET")
        .uri(&format!("/api/v1/inbox/{}", inbox_id_1.to_hex()))
        .body(Body::empty())
        .unwrap();

    let get_req_2 = Request::builder()
        .method("GET")
        .uri(&format!("/api/v1/inbox/{}", inbox_id_2.to_hex()))
        .body(Body::empty())
        .unwrap();

    let (get_resp1, get_resp2) = tokio::join!(
        app.clone().oneshot(get_req_1),
        app.clone().oneshot(get_req_2)
    );

    assert_eq!(get_resp1.unwrap().status(), StatusCode::OK);
    assert_eq!(get_resp2.unwrap().status(), StatusCode::OK);
}

fn create_inbox_request(name: &str, size: u64) -> Request<Body> {
    let create_data = CreateInbox {
        name: name.to_string(),
        size,
    };
    Request::builder()
        .method("POST")
        .uri("/api/v1/inbox")
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_string(&create_data).unwrap()))
        .unwrap()
}