use axum::Router;
use std::time::Duration;
use tower_http::{timeout::TimeoutLayer, trace::TraceLayer};
use tracing::info_span;

/// Router logging and timeout layer
pub fn router_logging_layer<T>(router: Router<T>) -> Router<T>
where
    T: std::clone::Clone + std::marker::Send + 'static + std::marker::Sync,
{
    router.layer((
        TraceLayer::new_for_http().make_span_with(|req: &axum::http::Request<axum::body::Body>| {
            let method = req.method();
            let uri = req.uri();

            // axum automatically adds this extension.
            let matched_path = req
                .extensions()
                .get::<axum::extract::MatchedPath>()
                .map(|matched_path| matched_path.as_str());

            info_span!("request", %method, %uri, matched_path)
        }),
        // Graceful shutdown will wait for outstanding requests to complete. Add a timeout so
        // requests don't hang forever.
        TimeoutLayer::with_status_code(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Duration::from_secs(10),
        ),
    ))
}
