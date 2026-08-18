#[cfg(test)]
mod tests {
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
        routing::get,
    };
    use mockall::predicate;
    use mongodb::bson::{doc, oid::ObjectId};
    use std::sync::Arc;
    use tower::ServiceExt;

    use crate::test_utils::create_test_state;
    use crate::handlers::get_inbox;
    use common::MockInboxStore;
    use common::test_utils::create_test_inbox_with_id;

    #[tokio::test]
    async fn test_get_inbox_success() {
        let mut mock_store = MockInboxStore::new();

        let inbox_id = ObjectId::new();
        let expected_inbox = create_test_inbox_with_id(inbox_id, "test-inbox", 100);

        mock_store
            .expect_find_one()
            .with(predicate::eq(doc! { "_id": inbox_id }))
            .times(1)
            .returning(move |_| Ok(Some(expected_inbox.clone())));

        let store = Arc::new(mock_store);
        let state = create_test_state(store.clone());
        let app = Router::new()
            .route("/api/v1/inbox/{id}", get(get_inbox))
            .with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(&format!("/api/v1/inbox/{}", inbox_id.to_hex()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_get_inbox_with_all_fields() {
        let mut mock_store = MockInboxStore::new();

        let inbox_id = ObjectId::new();
        let expected_inbox = create_test_inbox_with_id(inbox_id, "full-inbox", 500);

        mock_store
            .expect_find_one()
            .with(predicate::eq(doc! { "_id": inbox_id }))
            .times(1)
            .returning(move |_| Ok(Some(expected_inbox.clone())));

        let store = Arc::new(mock_store);
        let state = create_test_state(store.clone());
        let app = Router::new()
            .route("/api/v1/inbox/{id}", get(get_inbox))
            .with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(&format!("/api/v1/inbox/{}", inbox_id.to_hex()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let inbox: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(inbox["name"], "full-inbox");
        assert_eq!(inbox["size"], 500);
        assert_eq!(inbox["status"], "active");
    }

    #[tokio::test]
    async fn test_get_inbox_not_found() {
        let mut mock_store = MockInboxStore::new();

        let inbox_id = ObjectId::new();

        mock_store
            .expect_find_one()
            .with(predicate::eq(doc! { "_id": inbox_id }))
            .times(1)
            .returning(move |_| Ok(None));

        let store = Arc::new(mock_store);
        let state = create_test_state(store.clone());
        let app = Router::new()
            .route("/api/v1/inbox/{id}", get(get_inbox))
            .with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(&format!("/api/v1/inbox/{}", inbox_id.to_hex()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_get_inbox_invalid_id() {
        let mock_store = MockInboxStore::new();
        let store = Arc::new(mock_store);
        let state = create_test_state(store.clone());
        let app = Router::new()
            .route("/api/v1/inbox/{id}", get(get_inbox))
            .with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/inbox/invalid-id")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_get_inbox_database_error() {
        let mut mock_store = MockInboxStore::new();

        let inbox_id = ObjectId::new();
        // Use Error::custom() since ErrorKind variants are #[non_exhaustive]
        let db_error = mongodb::error::Error::custom("Database query failed");

        mock_store
            .expect_find_one()
            .with(predicate::eq(doc! { "_id": inbox_id }))
            .times(1)
            .returning(move |_| Err(db_error.clone()));

        let store = Arc::new(mock_store);
        let state = create_test_state(store.clone());
        let app = Router::new()
            .route("/api/v1/inbox/{id}", get(get_inbox))
            .with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(&format!("/api/v1/inbox/{}", inbox_id.to_hex()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
