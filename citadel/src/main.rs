//! Citadel - Backend service for protected data area management
//!
//! Citadel manages inbox lifecycle, file transfers, and notifications to downstream systems.
//! It consists of two binaries:
//! - `citadel`: Long-running HTTP service
//! - `citadel-job`: Job executor for file transfers (runs as K8s Job or local process)

use std::sync::Arc;
use axum::{
    routing::{get, post},
    Router,
};
use tokio::net::TcpListener;
use tracing::{info, error};

use citadel::config::CitadelConfig;
use citadel::auth::JwksCache;
use citadel::middleware::auth::auth_middleware;
use citadel::handlers;
use citadel::integrations::barbican::BarbicanClient;
use citadel::integrations::catalog::{CatalogNotifier, NoOpCatalogNotifier};
use citadel::integrations::lta::{LtaNotifier, NoOpLtaNotifier};
use citadel::jobs::executor::{JobExecutor, KubernetesExecutor, MockExecutor};
use citadel::polling::job_status::JobStatusPoller;
use citadel::secret_reloader::{load_callback_secret, start_secret_watcher};
use citadel::storage::manager::StorageManager;
use citadel::metrics::CitadelMetrics;
use citadel::AppState;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // P1.2: Initialize logging via common
    common::logging_setup(env!("CARGO_CRATE_NAME"));

    // Load configuration
    let config = CitadelConfig::from_env()
        .map_err(|e| {
            error!("Failed to load configuration: {}", e);
            e
        })?;

    info!("Starting Citadel service");
    info!("Bind address: {}", config.bind_address);
    info!("Prometheus port: {}", config.prometheus_port);
    info!("Job executor: {:?}", config.job_executor);

    // P1.1: Set up database connection via common
    let db_client = common::db_setup(&config.mongodb_uri, &config.mongodb_database)
        .await
        .map_err(|e| {
            error!("Failed to connect to database: {}", e);
            e
        })?;

    // Set up JWKS cache
    let jwks_cache = JwksCache::new(config.clone());

    // Set up storage manager
    let storage_manager = StorageManager::new(&config);

    // Set up Barbican client
    let barbican_client = BarbicanClient::new(config.clone());

    // Set up job executor
    let executor: Arc<dyn JobExecutor> = match config.job_executor {
        citadel::config::JobExecutorType::Kubernetes => {
            Arc::new(KubernetesExecutor::new(config.clone()).await?)
        }
        citadel::config::JobExecutorType::Mock => {
            Arc::new(MockExecutor::new(config.clone()))
        }
    };

    // Set up notifiers (using no-op implementations for now)
    let catalog_notifier: Arc<dyn CatalogNotifier> = Arc::new(NoOpCatalogNotifier);
    let lta_notifier: Arc<dyn LtaNotifier> = Arc::new(NoOpLtaNotifier);

    // Set up metrics
    let metrics = CitadelMetrics::new()
        .map_err(|e| {
            error!("Failed to initialize metrics: {}", e);
            e
        })?;

    // Load callback secret
    let callback_secret: Arc<tokio::sync::RwLock<Option<String>>> = Arc::new(
        tokio::sync::RwLock::new(load_callback_secret(&config.job_callback_secret_file))
    );

    // Create shared application state
    let app_state = AppState {
        config: Arc::new(config.clone()),
        jwks_cache: Arc::new(jwks_cache),
        storage_manager: Arc::new(storage_manager),
        barbican_client: Arc::new(barbican_client),
        executor,
        catalog_notifier,
        lta_notifier,
        metrics: Arc::new(metrics),
        db_client: Arc::new(db_client),
        callback_secret: callback_secret.clone(),
    };

    // Build router
    let api_routes = Router::new()
        .route("/inbox/{id}/create", post(handlers::create_inbox::create_inbox))
        .route("/inbox/{id}/publish", post(handlers::publish_inbox::publish_inbox))
        .route("/inbox/{id}/delete", post(handlers::delete_inbox::delete_inbox))
        .route("/job/{job_id}", get(handlers::get_job::get_job))
        .route("/jobs", get(handlers::list_jobs::list_jobs))
        .route("/job/{job_id}/cancel", post(handlers::cancel_job::cancel_job))
        .route("/job/callback", post(handlers::job_callback::job_callback))
        .layer(axum::middleware::from_fn_with_state(app_state.clone(), auth_middleware));

    // Combine routes
    let app = Router::new()
        // Health and readiness endpoints (no auth required)
        .route("/health", get(handlers::health::health))
        .route("/ready", get(handlers::ready::ready_with_client))
        // All API routes require authentication
        .nest("/api/v1", api_routes)
        .with_state(app_state.clone());

    // P1.6: Apply common logging layer (TraceLayer + TimeoutLayer)
    let app = common::router_logging_layer(app);

    // Start metrics server on separate port using common infrastructure
    let metrics_state = app_state.clone();
    let metrics_router = Router::new()
        .route("/metrics", get(move || {
            let state = metrics_state.clone();
            async move {
                let metric_families = state.metrics.registry.gather();
                if metric_families.is_empty() {
                    return (axum::http::StatusCode::OK, String::new());
                }
                let encoder = prometheus::TextEncoder::new();
                let mut buffer = String::new();
                if let Err(e) = encoder.encode_utf8(&metric_families, &mut buffer) {
                    error!("Failed to encode metrics: {}", e);
                    return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "Failed to encode metrics".to_string());
                }
                (axum::http::StatusCode::OK, buffer)
            }
        }));
    if let Err(e) = common::start_metrics_server_with_router(
        app_state.config.prometheus_port,
        metrics_router,
    )
    .await
    {
        error!("Failed to start metrics server: {}", e);
    }

    // Start background job status poller
    let poller_config = app_state.config.clone();
    let poller_executor = app_state.executor.clone();
    let poller_db = app_state.db_client.clone();
    let poller = JobStatusPoller::new(poller_config.as_ref().clone(), poller_executor, poller_db);
    tokio::spawn(async move {
        if let Err(e) = poller.run().await {
            error!("Job status poller error: {}", e);
        }
    });
    info!("Background job status poller started");

    // Start callback secret file watcher
    let watcher_store = app_state.callback_secret.clone();
    let watcher_path = config.job_callback_secret_file.clone();
    tokio::spawn(async move {
        start_secret_watcher(watcher_path, watcher_store).await;
    });
    info!("Callback secret hot-reload watcher started");

    // Start main HTTP server
    let addr = app_state.config.bind_address.clone();
    info!("Starting Citadel HTTP server on {}", addr);

    let listener = TcpListener::bind(&addr).await
        .unwrap_or_else(|e| {
            error!("Failed to bind server: {}", e);
            std::process::exit(1);
        });

    // P1.3: Use common::shutdown_signal
    axum::serve(listener, app)
        .with_graceful_shutdown(common::shutdown_signal())
        .await
        .unwrap_or_else(|e| {
            error!("Server error: {}", e);
        });

    Ok(())
}