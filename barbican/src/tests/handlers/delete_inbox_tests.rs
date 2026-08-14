#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        Router,
        routing::delete,
    };
    use mockall::predicate;
    use mongodb::bson::{doc, oid::ObjectId};
    use std::sync::Arc;
    use tower::ServiceExt;

    use crate::handlers::delete_inbox;
    use common::test_utils::create_test_inbox_with_id;
    use common::{InboxStatus, MockInboxStore};

    #[tokio::test]
    async fn test_delete_inbox_success() {
        let mut mock_store = MockInboxStore::new();

        let inbox_id = ObjectId::new();
        let mut inbox = create_test_inbox_with_id(inbox_id, "test-inbox", 100);
        inbox.status = InboxStatus::Active;

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

    #[tokio::test]
    async fn test_delete_inbox_not_found() {
        let mut mock_store = MockInboxStore::new();

        let inbox_id = ObjectId::new();

        mock_store
            .expect_find_one()
            .with(predicate::eq(doc! { "_id": inbox_id }))
            .times(1)
            .returning(move |_| Ok(None));

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

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_delete_inbox_already_publishing() {
        let mut mock_store = MockInboxStore::new();

        let inbox_id = ObjectId::new();
        let mut inbox = create_test_inbox_with_id(inbox_id, "test-inbox", 100);
        inbox.status = InboxStatus::Publishing;

        mock_store
            .expect_find_one()
            .with(predicate::eq(doc! { "_id": inbox_id }))
            .times(1)
            .returning(move |_| Ok(Some(inbox.clone())));

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

        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn test_delete_inbox_already_deleted() {
        let mut mock_store = MockInboxStore::new();

        let inbox_id = ObjectId::new();
        let mut inbox = create_test_inbox_with_id(inbox_id, "deleted-inbox", 100);
        inbox.status = InboxStatus::Deleted;

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

    #[tokio::test]
    async fn test_delete_inbox_database_error() {
        let mut mock_store = MockInboxStore::new();

        let inbox_id = ObjectId::new();
        let db_error = mongodb::error::Error::custom("Database error");

        mock_store
            .expect_find_one()
            .with(predicate::eq(doc! { "_id": inbox_id }))
            .times(1)
            .returning(move |_| Err(db_error.clone()));

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

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn test_delete_inbox_invalid_id() {
        let mock_store = MockInboxStore::new();
        let store = Arc::new(mock_store);
        let app = Router::new()
            .route("/api/v1/inbox/{id}", delete(delete_inbox))
            .with_state(store);

        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/v1/inbox/invalid-id")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}