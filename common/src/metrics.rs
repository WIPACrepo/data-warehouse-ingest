//! Prometheus metrics helpers

use axum::{Router, routing::get};
use axum_prometheus::metrics_exporter_prometheus::{Matcher, PrometheusBuilder};
use axum_prometheus::{
    AXUM_HTTP_REQUESTS_DURATION_SECONDS, PrometheusMetricLayer, utils::SECONDS_DURATION_BUCKETS,
};

use crate::error::AppError;

/// Create a standalone metrics router that serves `/metrics`
pub fn metrics_app() -> Router {
    // This is the default if you use `PrometheusMetricLayer::pair`.
    let metric_handle = PrometheusBuilder::new()
        .set_buckets_for_metric(
            Matcher::Full(AXUM_HTTP_REQUESTS_DURATION_SECONDS.to_string()),
            SECONDS_DURATION_BUCKETS,
        )
        .unwrap()
        .install_recorder()
        .unwrap();
    Router::new().route("/metrics", get(|| async move { metric_handle.render() }))
}

/// Start a background metrics server on the given port
pub async fn start_metrics_server(port: u16) -> Result<(), AppError> {
    let app = metrics_app();
    start_metrics_server_with_router(port, app).await
}

/// Start a background metrics server on the given port using a custom router.
///
/// Use this when the service has its own prometheus registry (e.g. Citadel's
/// `CitadelMetrics`) and needs a `/metrics` endpoint that renders it.
pub async fn start_metrics_server_with_router(
    port: u16,
    app: Router,
) -> Result<(), AppError> {
    let bind_addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .map_err(|e| AppError::InternalError(format!("Failed to bind metrics server on {bind_addr}: {e}")))?;

    tracing::info!("Metrics server listening on {}", bind_addr);

    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!(error = ?e, "Metrics server failed");
        }
    });

    Ok(())
}

/// Add a Prometheus metric layer to an existing router
pub fn router_prometheus(router: Router) -> Router {
    let metric_layer = PrometheusMetricLayer::new();
    router.layer(metric_layer)
}