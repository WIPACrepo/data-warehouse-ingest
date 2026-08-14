#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        Router,
        routing::post,
    };
    use mockall::predicate;
    use mongodb::bson::oid::ObjectId;
    use std::sync::Arc;
    use tower::ServiceExt;

    use crate::{
        handlers::create_inbox,
    };
    use common::test_utils::{create_test_inbox, create_test_request};
    use common::MockInboxStore;

    #[tokio::test]
    async fn test_create_inbox_success() {
        let mut mock_store = MockInboxStore::new();

        let expected_inbox = create_test_inbox("test-inbox", 100);
        let inbox_id = expected_inbox.id.unwrap();
        
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

        let inbox_id = ObjectId::new();
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
}