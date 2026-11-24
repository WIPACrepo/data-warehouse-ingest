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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;

    #[test]
    fn router_logging_layer_applies_layers() {
        // Build a minimal router and apply the logging layer.
        // This is a compile-time check that `router_logging_layer` accepts a generic Router<T>
        // and returns a Router<T> with TraceLayer + TimeoutLayer applied.
        let handler = || async { "ok" };
        let base: Router<()> = Router::new().route("/ping", get(handler));
        let layered = router_logging_layer(base);

        // Verify the layered router still compiles and is a Router.
        let _: Router<()> = layered;
    }
}
