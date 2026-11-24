//! Tests for Barbican's `/ready` probe (REVIEW #6): it must reflect a real
//! MongoDB ping, not a static "connected".

#[cfg(test)]
mod tests {
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
        routing::get,
    };
    use std::sync::Arc;
    use tower::ServiceExt;

    use crate::handlers::ready;
    use crate::test_utils::create_test_state;
    use common::MockInboxStore;

    async fn call_ready(mock_store: MockInboxStore) -> (StatusCode, serde_json::Value) {
        let state = create_test_state(Arc::new(mock_store));
        let app = Router::new().route("/ready", get(ready)).with_state(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/ready")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }

    #[tokio::test]
    async fn test_ready_when_ping_succeeds() {
        let mut mock_store = MockInboxStore::new();
        mock_store.expect_ping().times(1).returning(|| Ok(()));

        let (status, json) = call_ready(mock_store).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["status"], "ready");
        assert_eq!(json["mongodb"], "connected");
    }

    #[tokio::test]
    async fn test_not_ready_when_ping_fails() {
        let mut mock_store = MockInboxStore::new();
        mock_store
            .expect_ping()
            .times(1)
            .returning(|| Err(mongodb::error::Error::custom("down")));

        let (status, json) = call_ready(mock_store).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(json["status"], "not_ready");
        assert_eq!(json["mongodb"], "disconnected");
    }
}
