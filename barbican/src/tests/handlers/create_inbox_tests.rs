#[cfg(test)]
mod tests {
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
        routing::post,
    };
    use mockall::predicate;
    use mongodb::bson::oid::ObjectId;
    use std::sync::Arc;
    use tower::ServiceExt;

    use crate::handlers::create_inbox;
    use common::MockInboxStore;
    use common::test_utils::{create_test_inbox, create_test_request};

    #[tokio::test]
    async fn test_create_inbox_success() {
        let mut mock_store = MockInboxStore::new();

        let expected_inbox = create_test_inbox("test-inbox", 100);
        let _inbox_id = expected_inbox.id.unwrap();

        // Expect find_one to check for duplicates (returns None = no duplicate)
        mock_store
            .expect_find_one()
            .with(predicate::always())
            .times(1)
            .returning(move |_| Ok(None));

        mock_store
            .expect_insert_one()
            .with(predicate::always())
            .times(1)
            .returning(move |_| {
                // InsertOneResult is #[non_exhaustive], use default()
                Ok(mongodb::results::InsertOneResult::default())
            });

        let store = Arc::new(mock_store);
        let app = Router::new()
            .route("/api/v1/inbox", post(create_inbox))
            .with_state(store);

        let request_body = create_test_request("test-inbox", 100);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/inbox")
                    .header("Content-Type", "application/json")
                    .body(Body::from(serde_json::to_string(&request_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_create_inbox_with_minimal_data() {
        let mut mock_store = MockInboxStore::new();

        let _inbox_id = ObjectId::new();

        // Expect find_one to check for duplicates (returns None = no duplicate)
        mock_store
            .expect_find_one()
            .with(predicate::always())
            .times(1)
            .returning(move |_| Ok(None));

        mock_store
            .expect_insert_one()
            .with(predicate::always())
            .times(1)
            .returning(move |_| {
                // InsertOneResult is #[non_exhaustive], use default()
                Ok(mongodb::results::InsertOneResult::default())
            });

        let store = Arc::new(mock_store);
        let app = Router::new()
            .route("/api/v1/inbox", post(create_inbox))
            .with_state(store);

        let request_body = create_test_request("minimal-inbox", 0);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/inbox")
                    .header("Content-Type", "application/json")
                    .body(Body::from(serde_json::to_string(&request_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_create_inbox_database_error() {
        let mut mock_store = MockInboxStore::new();

        // Expect find_one to check for duplicates (returns None = no duplicate)
        mock_store
            .expect_find_one()
            .with(predicate::always())
            .times(1)
            .returning(move |_| Ok(None));

        // Use Error::custom() since ErrorKind variants are #[non_exhaustive]
        let db_error = mongodb::error::Error::custom("Database connection failed");

        mock_store
            .expect_insert_one()
            .with(predicate::always())
            .times(1)
            .returning(move |_| Err(db_error.clone()));

        let store = Arc::new(mock_store);
        let app = Router::new()
            .route("/api/v1/inbox", post(create_inbox))
            .with_state(store);

        let request_body = create_test_request("test-inbox", 100);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/inbox")
                    .header("Content-Type", "application/json")
                    .body(Body::from(serde_json::to_string(&request_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn test_create_inbox_invalid_json() {
        let mock_store = MockInboxStore::new();
        let store = Arc::new(mock_store);
        let app = Router::new()
            .route("/api/v1/inbox", post(create_inbox))
            .with_state(store);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/inbox")
                    .header("Content-Type", "application/json")
                    .body(Body::from("invalid json"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_create_inbox_duplicate_warehouse_path() {
        let mut mock_store = MockInboxStore::new();

        let existing_inbox = create_test_inbox("existing-inbox", 100);

        // Expect find_one to return existing inbox (duplicate)
        mock_store
            .expect_find_one()
            .with(predicate::always())
            .times(1)
            .returning(move |_| Ok(Some(existing_inbox.clone())));

        let store = Arc::new(mock_store);
        let app = Router::new()
            .route("/api/v1/inbox", post(create_inbox))
            .with_state(store);

        let request_body = create_test_request("test-inbox", 100);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/inbox")
                    .header("Content-Type", "application/json")
                    .body(Body::from(serde_json::to_string(&request_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_create_inbox_missing_warehouse_path() {
        let mock_store = MockInboxStore::new();
        let store = Arc::new(mock_store);
        let app = Router::new()
            .route("/api/v1/inbox", post(create_inbox))
            .with_state(store);

        // Send request with empty warehouse_path
        let request_body = r#"{"name": "test", "size": 100, "warehouse_path": ""}"#;
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/inbox")
                    .header("Content-Type", "application/json")
                    .body(Body::from(request_body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
