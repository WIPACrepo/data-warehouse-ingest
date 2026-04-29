use std::{future::ready, str::FromStr, sync::Arc, time::{Duration, Instant}};

use axum::{
    Json, Router, extract::{FromRequest, MatchedPath, Path, Request, State, rejection::JsonRejection}, http::StatusCode, middleware::{Next, from_fn}, response::{IntoResponse, Response}, routing::{delete, get, post, put}
};
use axum_prometheus::{AXUM_HTTP_REQUESTS_DURATION_SECONDS, utils::SECONDS_DURATION_BUCKETS, PrometheusMetricLayer};
use axum_prometheus::metrics_exporter_prometheus::{Matcher, PrometheusBuilder};
use mongodb::{
    bson::doc,
    results::{DeleteResult, InsertOneResult, UpdateResult},
    Client, Collection,
};
use serde::{Deserialize, Serialize};
use tokio::{fs::create_dir, signal};
use tower_http::{metrics, timeout::TimeoutLayer, trace::TraceLayer};
use tracing::{Level, error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

// set up logging
pub fn logging_setup(crate_name: &str) {
    let mut validation_errors = Vec::new();

    // Define the parsing logic
    let mut get_level = |key: &str, default: &str| {
        let raw = std::env::var(key).unwrap_or_default();
        if raw.is_empty() {
            return default.to_string();
        }
        match Level::from_str(&raw.to_lowercase()) {
            Ok(level) => level.to_string(),
            Err(_) => {
                validation_errors.push(format!("Invalid log level '{}' for {}; falling back to '{}'", raw, key, default));
                default.to_string()
            }
        }
    };

    // Capture the resolved levels so we can log them later
    let overall = get_level("THIRD_PARTY_LOG_LEVEL", "warn");
    let http = get_level("HTTP_LOG_LEVEL", "debug");
    let crate_level = get_level("LOG_LEVEL", "info");
    let common_crate_name = env!("CARGO_CRATE_NAME");

    let filter_str = format!("{},tower_http={},{}={},{}={}", overall, http, common_crate_name, crate_level, crate_name, crate_level);

    // Initialize
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| filter_str.clone().into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Log the outcome
    for err in validation_errors {
        error!("{}", err);
    }

    info!("Logging initialized: global={}", filter_str);
}

// catch signals for graceful shutdown
pub async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("Shutting down");
}

// set up DB
pub async fn db_setup() -> Client {
    // connecting to mongodb
    let db_connection_str = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        "mongodb://localhost:27017".to_string()
    });
    let client = Client::with_uri_str(db_connection_str.as_str()).await.unwrap();

    // pinging the database
    let db_name_str = std::env::var("DATABASE_NAME").unwrap_or_else(|_| {
        "data-warehouse-ingress".to_string()
    });
    client
        .database(db_name_str.as_str())
        .run_command(doc! { "ping": 1 })
        .await
        .unwrap();
    tracing::info!("Successfully connected to MongoDB {}/{}", db_connection_str.as_str(), db_name_str.as_str());

    client
}

pub fn db_collection<T: Send + Sync>(client: Client, name: &str) -> Collection<T> {
    let db_name_str = std::env::var("DATABASE_NAME").unwrap_or_else(|_| {
        "data-warehouse-ingress".to_string()
    });
    client.database(db_name_str.as_str()).collection(name)
}

// Router logging and timeout layer
pub fn router_logging_layer<T>(router: Router<T>) -> Router<T>
    where T: std::clone::Clone, T: std::marker::Send + 'static, T: std::marker::Sync
{
    router
        .layer((
            TraceLayer::new_for_http()
                // Create our own span for the request and include the matched path. The matched
                // path is useful for figuring out which handler the request was routed to.
                .make_span_with(|req: &Request| {
                    let method = req.method();
                    let uri = req.uri();

                    // axum automatically adds this extension.
                    let matched_path = req
                        .extensions()
                        .get::<MatchedPath>()
                        .map(|matched_path| matched_path.as_str());

                    tracing::debug_span!("request", %method, %uri, matched_path)
                }),
            // Graceful shutdown will wait for outstanding requests to complete. Add a timeout so
            // requests don't hang forever.
            TimeoutLayer::with_status_code(
                StatusCode::INTERNAL_SERVER_ERROR,
                Duration::from_secs(10)
            ),
        ))
}


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
    Router::new()
        .route("/metrics", get(|| async move { metric_handle.render() }))
}

pub async fn start_metrics_server() {
    let app = metrics_app();

    // NOTE: expose metrics endpoint on a different port
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3001")
        .await
        .unwrap();
    tracing::debug!("listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app).await.unwrap();
}

pub fn router_prometheus(router: Router) -> Router {
    let metric_layer = PrometheusMetricLayer::new();
    router.layer(metric_layer)
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let result = 4;
        assert_eq!(result, 4);
    }
}
