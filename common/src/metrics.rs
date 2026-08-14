use axum::{Router, routing::get};
use axum_prometheus::metrics_exporter_prometheus::{Matcher, PrometheusBuilder};
use axum_prometheus::{
    AXUM_HTTP_REQUESTS_DURATION_SECONDS, PrometheusMetricLayer, utils::SECONDS_DURATION_BUCKETS,
};

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

pub async fn start_metrics_server() {
    let app = metrics_app();

    // NOTE: expose metrics endpoint on a different port
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3001")
        .await
        .unwrap();
    tracing::debug!("listening on {}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
}

pub fn router_prometheus(router: Router) -> Router {
    let metric_layer = PrometheusMetricLayer::new();
    router.layer(metric_layer)
}
