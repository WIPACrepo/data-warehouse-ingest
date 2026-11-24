//! P1.1 optimistic-locking conflict tests for the transition handlers.
//!
//! Every transition handler applies a version-conditional
//! `update_one({_id, version}, ...)`. When the store reports
//! `matched_count: 0` the inbox was modified concurrently and the handler must
//! return `409 Conflict` with `{"error":"conflict", ...}` (previously this
//! case silently succeeded). One conflict test per operation (create/publish/
//! delete start) asserting the 409 + JSON body.

#[cfg(test)]
mod tests {
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
        routing::post,
    };
    use mockall::predicate;
    use mongodb::bson::{doc, oid::ObjectId};
    use serde_json::{Value, json};
    use std::sync::Arc;
    use tower::ServiceExt;

    use crate::handlers::transition::{create_start, delete_start, publish_start};
    use crate::test_utils::create_test_state;
    use common::test_utils::InboxBuilder;
    use common::{InboxStatus, MockInboxStore};

    /// Seed a mock store with an inbox in the given state and make the single
    /// versioned update report `matched_count: 0` (concurrent modification).
    fn seed_conflicting_store(status: InboxStatus) -> (Arc<MockInboxStore>, ObjectId) {
        let mut mock_store = MockInboxStore::new();
        let inbox_id = ObjectId::new();
        let inbox = InboxBuilder::new()
            .with_id(inbox_id)
            .with_name("conflict-inbox")
            .with_status(status.clone())
            .with_version(2)
            .build();

        mock_store
            .expect_find_one()
            .with(predicate::eq(doc! { "_id": inbox_id }))
            .times(1)
            .returning(move |_| Ok(Some(inbox.clone())));

        // matched_count = 0 => version-conditional update matched nothing.
        mock_store
            .expect_update_one()
            .with(predicate::always(), predicate::always())
            .times(1)
            .returning(|_, _| {
                let mut res = mongodb::results::UpdateResult::default();
                res.matched_count = 0;
                Ok(res)
            });

        (Arc::new(mock_store), inbox_id)
    }

    /// Extract the JSON body of a response.
    async fn response_json(response: axum::response::Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// Drive a request through a single-route router and assert 409 +
    /// `{"error":"conflict"}`.
    async fn assert_conflict_response(
        store: Arc<MockInboxStore>,
        _inbox_id: ObjectId,
        route: &str,
        uri: &str,
        body: &str,
    ) {
        let state = create_test_state(store);
        let app = Router::new()
            .route(route, post(route_handler(route)))
            .with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("Content-Type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            StatusCode::CONFLICT,
            "stale-version update must yield 409 for {}",
            route
        );
        let json = response_json(response).await;
        let expected = json!({ "error": "conflict" });
        assert_eq!(
            json["error"], expected["error"],
            "expected error code 'conflict', got {:?}",
            json
        );
        assert!(
            json["message"].as_str().is_some_and(|m| !m.is_empty()),
            "conflict body must include a non-empty message: {:?}",
            json
        );
    }

    /// Map a route string to its handler fn (small helper to keep routers
    /// above compact).
    fn route_handler(route: &str) -> axum::routing::MethodRouter<Arc<crate::AppState>> {
        match route {
            "/api/v1/inbox/{id}/create/start" => post(create_start),
            "/api/v1/inbox/{id}/publish/start" => post(publish_start),
            "/api/v1/inbox/{id}/delete/start" => post(delete_start),
            _ => panic!("unhandled route {route}"),
        }
    }

    #[tokio::test]
    async fn test_create_start_conflict_when_version_stale() {
        let (store, inbox_id) = seed_conflicting_store(InboxStatus::Active);
        assert_conflict_response(
            store,
            inbox_id,
            "/api/v1/inbox/{id}/create/start",
            &format!("/api/v1/inbox/{}/create/start", inbox_id.to_hex()),
            r#"{"job_id":"job-1"}"#,
        )
        .await;
    }

    #[tokio::test]
    async fn test_publish_start_conflict_when_version_stale() {
        let (store, inbox_id) = seed_conflicting_store(InboxStatus::Ready);
        assert_conflict_response(
            store,
            inbox_id,
            "/api/v1/inbox/{id}/publish/start",
            &format!("/api/v1/inbox/{}/publish/start", inbox_id.to_hex()),
            r#"{"job_id":"job-2"}"#,
        )
        .await;
    }

    #[tokio::test]
    async fn test_delete_start_conflict_when_version_stale() {
        let (store, inbox_id) = seed_conflicting_store(InboxStatus::Ready);
        assert_conflict_response(
            store,
            inbox_id,
            "/api/v1/inbox/{id}/delete/start",
            &format!("/api/v1/inbox/{}/delete/start", inbox_id.to_hex()),
            r#"{"job_id":"job-3"}"#,
        )
        .await;
    }
}
