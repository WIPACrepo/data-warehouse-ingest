//! Barbican metrics unit tests (P4.3, decision D4(iv)).
//!
//! Covers the two Barbican-owned instruments:
//! - `barbican_inboxes_total{status}` — the gauge refresh against a mock
//!   store;
//! - `barbican_api_requests_total{endpoint,method,status}` — the
//!   response-recording middleware, exercised through the real router so
//!   axum attaches the matched route template.

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::Request,
    };
    use mockall::predicate;
    use std::sync::Arc;
    use tower::ServiceExt;

    use crate::metrics::BarbicanMetrics;
    use crate::test_utils::create_test_state;
    use common::{InboxStatus, MockInboxStore};

    #[tokio::test]
    async fn test_inbox_gauge_refresh_counts_by_status() {
        use common::test_utils::InboxBuilder;

        let mut mock_store = MockInboxStore::new();

        // Two Published, one Failed inbox.
        let published1 = InboxBuilder::new().with_name("pub1").with_status(InboxStatus::Published).build();
        let published2 = InboxBuilder::new().with_name("pub2").with_status(InboxStatus::Published).build();
        let failed = InboxBuilder::new().with_name("fail1").with_status(InboxStatus::Failed).build();
        let all = vec![published1, published2, failed];

        mock_store
            .expect_find_many()
            .with(predicate::always(), predicate::always())
            .times(1)
            .returning(move |_, _| Ok(all.clone()));

        let store: Arc<dyn common::InboxStore> = Arc::new(mock_store);
        let metrics = BarbicanMetrics::new().unwrap();

        metrics.refresh_inbox_gauge(&store).await;

        let families = metrics.registry.gather();
        let inboxes = families
            .iter()
            .find(|f| f.get_name() == "barbican_inboxes_total")
            .expect("barbican_inboxes_total must be registered");

        let mut by_status = std::collections::HashMap::new();
        for m in inboxes.get_metric() {
            let label = m
                .get_label()
                .iter()
                .find(|l| l.get_name() == "status")
                .map(|l| l.get_value().to_string())
                .unwrap_or_default();
            by_status.insert(label, m.get_gauge().get_value() as u64);
        }

        assert_eq!(by_status.get("published"), Some(&2));
        assert_eq!(by_status.get("failed"), Some(&1));
        // Zeroed labels linger, not stale-counted.
        assert_eq!(by_status.get("active"), Some(&0));
    }

    #[tokio::test]
    async fn test_api_requests_counter_records_endpoint_method_status() {
        let mock_store = MockInboxStore::new();
        let store = Arc::new(mock_store);
        let state = create_test_state(store.clone());
        let app = crate::create_app(state.clone());

        // Hit /health (public route) so the middleware records a metric.
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response.status().is_success());

        let families = state.metrics.registry.gather();
        let requests = families
            .iter()
            .find(|f| f.get_name() == "barbican_api_requests_total")
            .expect("barbican_api_requests_total must be registered");

        let mut found = false;
        for m in requests.get_metric() {
            let labels: std::collections::HashMap<&str, &str> = m
                .get_label()
                .iter()
                .map(|l| (l.get_name(), l.get_value()))
                .collect();
            if labels.get("method") == Some(&"GET")
                && labels.get("status") == Some(&"200")
                && labels.get("endpoint").is_some()
            {
                assert_eq!(
                    labels.get("endpoint"),
                    Some(&"/health"),
                    "endpoint label must be the matched route template"
                );
                assert!(m.get_counter().get_value() >= 1.0);
                found = true;
            }
        }
        assert!(found, "expected a GET /health 200 request metric");
    }
}
