//! Prometheus metrics for Barbican (P4.3, decision D4(iv)).
//!
//! Two instruments are owned by Barbican, matching the documented sample:
//!
//! - `barbican_inboxes_total{status}` — a gauge over the `inboxes`
//!   collection. It is refreshed on a background interval (see
//!   [`BarbicanMetrics::refresh_inbox_gauge`]) rather than incremented per write, because the
//!   collection is also mutated by Citadel through the transition endpoints.
//! - `barbican_api_requests_total{endpoint,method,status}` — a per-request
//!   counter recorded by [`record_request_metrics`]. `endpoint` is the matched
//!   route template (e.g. `/api/v1/inbox/{id}/publish/complete`), not the raw
//!   URL, so cardinality stays bounded.

use axum::{
    Router,
    body::Body,
    extract::{MatchedPath, State},
    http::{Request, StatusCode},
    middleware::Next,
    response::Response,
};
use mongodb::bson::doc;
use prometheus::{GaugeVec, IntCounterVec, Opts, Registry};
use std::sync::Arc;
use tracing::error;

use common::{InboxDB, InboxStatus, InboxStore};

/// Barbican-owned prometheus metrics.
pub struct BarbicanMetrics {
    pub registry: Registry,
    /// `barbican_inboxes_total{status}` — inbox counts by status, refreshed
    /// periodically by [`BarbicanMetrics::refresh_inbox_gauge`].
    pub inboxes_total: GaugeVec,
    /// `barbican_api_requests_total{endpoint,method,status}` — HTTP request
    /// counter, incremented by [`record_request_metrics`].
    pub api_requests_total: IntCounterVec,
}

/// `InboxStatus` -> snake_case string (mirrors `#[serde(rename_all =
/// "snake_case")]` on the enum).
fn status_label(status: &InboxStatus) -> &'static str {
    match status {
        InboxStatus::Active => "active",
        InboxStatus::Creating => "creating",
        InboxStatus::Ready => "ready",
        InboxStatus::Publishing => "publishing",
        InboxStatus::Published => "published",
        InboxStatus::Failed => "failed",
        InboxStatus::Deleting => "deleting",
        InboxStatus::Deleted => "deleted",
    }
}

impl BarbicanMetrics {
    pub fn new() -> Result<Self, prometheus::Error> {
        let registry = Registry::new();

        let inboxes_total = GaugeVec::new(
            Opts::new(
                "barbican_inboxes_total",
                "Number of inboxes by status (refreshed periodically)",
            ),
            &["status"],
        )?;
        registry.register(Box::new(inboxes_total.clone()))?;

        let api_requests_total = IntCounterVec::new(
            Opts::new(
                "barbican_api_requests_total",
                "HTTP requests handled by Barbican",
            ),
            &["endpoint", "method", "status"],
        )?;
        registry.register(Box::new(api_requests_total.clone()))?;

        Ok(Self {
            registry,
            inboxes_total,
            api_requests_total,
        })
    }

    /// Re-collect the inbox-count gauge from the store (all statuses).
    ///
    /// Zeroes every known status label first so stale labels (e.g. a status
    /// that no longer has any inboxes) do not linger.
    pub async fn refresh_inbox_gauge(&self, store: &Arc<dyn InboxStore>) {
        let statuses = [
            InboxStatus::Active,
            InboxStatus::Creating,
            InboxStatus::Ready,
            InboxStatus::Publishing,
            InboxStatus::Published,
            InboxStatus::Failed,
            InboxStatus::Deleting,
            InboxStatus::Deleted,
        ];
        for s in statuses {
            self.inboxes_total
                .with_label_values(&[status_label(&s)])
                .set(0.0);
        }
        let sample: Vec<InboxDB> = match store.find_many(doc! {}, Default::default()).await {
            Ok(inboxes) => inboxes,
            Err(e) => {
                error!("Failed to refresh barbican_inboxes_total gauge: {e}");
                return;
            }
        };
        for inbox in sample {
            self.inboxes_total
                .with_label_values(&[status_label(&inbox.status)])
                .inc();
        }
    }

    /// Background task: refresh the inbox-count gauge every `interval_secs`.
    ///
    /// The first refresh happens at startup (the initial `tick()` fires
    /// immediately), then on the interval.
    pub fn spawn_inbox_refresh_loop(
        self: &Arc<Self>,
        store: Arc<dyn InboxStore>,
        interval_secs: u64,
    ) {
        let metrics = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
            ticker.tick().await; // immediate first fire
            loop {
                ticker.tick().await;
                metrics.refresh_inbox_gauge(&store).await;
            }
        });
    }
}

/// Response-recording middleware for the request counter.
///
/// `metrics` is provided via axum State (wire with
/// `from_fn_with_state(metrics.clone(), record_request_metrics)`). Records
/// `barbican_api_requests_total{endpoint,method,status}` after the handler
/// runs, using the matched route template for `endpoint`.
pub async fn record_request_metrics(
    State(metrics): State<Arc<BarbicanMetrics>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let method = req.method().to_string();
    let endpoint = req
        .extensions()
        .get::<MatchedPath>()
        .map(|mp| mp.as_str().to_string())
        .unwrap_or_else(|| req.uri().path().to_string());
    let response = next.run(req).await;
    let status = response.status().as_u16().to_string();
    metrics
        .api_requests_total
        .with_label_values(&[&endpoint, &method, &status])
        .inc();
    response
}

/// Build a `/metrics` router that renders this registry (for
/// `common::start_metrics_server_with_router`).
pub fn metrics_router(metrics: Arc<BarbicanMetrics>) -> Router<()> {
    use axum::routing::get;
    Router::new().route(
        "/metrics",
        get(move || {
            let metrics = metrics.clone();
            async move {
                let metric_families = metrics.registry.gather();
                if metric_families.is_empty() {
                    return (StatusCode::OK, String::new());
                }
                let encoder = prometheus::TextEncoder::new();
                let mut buffer = String::new();
                match encoder.encode_utf8(&metric_families, &mut buffer) {
                    Ok(()) => (StatusCode::OK, buffer),
                    Err(e) => {
                        error!("Failed to encode metrics: {e}");
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "Failed to encode metrics".to_string(),
                        )
                    }
                }
            }
        }),
    )
}
