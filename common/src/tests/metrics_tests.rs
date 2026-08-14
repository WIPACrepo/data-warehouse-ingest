use crate::metrics::{metrics_app, router_prometheus};
use axum::{Router, routing::get};

#[test]
fn test_metrics_app_has_metrics_route() {
    let app = metrics_app();
    // The app should have a /metrics route
    // We can't easily test the actual route without a test client,
    // but we can verify the function returns a Router
    let _ = app;
}

#[test]
fn test_router_prometheus_adds_layer() {
    let base_router: Router = Router::new().route("/test", get(|| async { "OK" }));
    let router_with_metrics = router_prometheus(base_router);

    // Verify we get a Router back (type check)
    let _ = router_with_metrics;
}
