//! Citadel - Backend service for protected data area management
//!
//! Citadel manages inbox lifecycle, file transfers, and notifications to downstream systems.
//! It consists of two binaries:
//! - `citadel`: Long-running HTTP service
//! - `citadel-job`: Job executor for file transfers (runs as K8s Job or local process)

use axum::{Router, routing::get};
use futures::FutureExt;
use futures::future::Shared;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{error, info};

use citadel::config::CitadelConfig;
use citadel::integrations::barbican::{BarbicanClient, InboxTransitioner};
use citadel::integrations::catalog::{CatalogNotifier, NoOpCatalogNotifier};
use citadel::integrations::lta::{LtaNotifier, NoOpLtaNotifier};
use citadel::jobs::executor::{JobExecutor, KubernetesExecutor, MockExecutor};
use citadel::metrics::CitadelMetrics;
use citadel::polling::job_status::JobStatusPoller;
use citadel::secret_reloader::{load_callback_secret, start_secret_watcher};
use citadel::storage::manager::StorageManager;
use citadel::{AppState, AuthConfig, build_auth_state};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // P1.2: Initialize logging via common
    common::logging_setup(env!("CARGO_CRATE_NAME"));

    // Create shutdown signal early so Ctrl+C works during startup
    let shutdown_fut = common::shutdown_signal();
    let shutdown_shared: Shared<_> = shutdown_fut.shared();
    let shutdown_clone = shutdown_shared.clone();

    // Load configuration
    let config = CitadelConfig::from_env().map_err(|e| {
        error!("Failed to load configuration: {}", e);
        e
    })?;

    info!("Starting Citadel service");
    info!("Bind address: {}", config.bind_address);
    info!("Prometheus port: {}", config.prometheus_port);
    info!("Job executor: {:?}", config.job_executor);

    // P1.1: Set up database connection via common with shutdown support
    let db_client =
        common::db_setup_with_shutdown(&config.mongodb_uri, &config.mongodb_database, move || {
            shutdown_clone.clone()
        })
        .await
        .map_err(|e| {
            error!("Failed to connect to database: {}", e);
            e
        })?;

    // Ensure the callback-nonces TTL index (P2.2). Idempotent; a failure is
    // logged, not fatal.
    citadel::db::ensure_citadel_indexes(&db_client, &config).await;

    // Set up JWKS cache and auth components
    // Reads CITADEL_KEYCLOAK_*, CITADEL_REQUIRED_ROLES, ... (decision D10).
    let auth_config = AuthConfig::from_env_prefixed("CITADEL").map_err(|e| {
        error!("Failed to load auth config: {}", e);
        e
    })?;
    let auth_state = build_auth_state(&auth_config, "citadel");

    // Set up storage manager
    let storage_manager = StorageManager::new(&config);

    // Set up metrics (BarbicanClient needs the metrics handle for breaker/errors)
    let metrics = Arc::new(CitadelMetrics::new().map_err(|e| {
        error!("Failed to initialize metrics: {}", e);
        e
    })?);

    // Set up Barbican client (wired with metrics for P4.4 metering)
    // Service token for Barbican's internal routes (client-credentials grant,
    // secret from CITADEL_BARBICAN_CLIENT_SECRET). None = no Authorization.
    let barbican_token_source =
        common::ServiceTokenSource::from_env("CITADEL", "BARBICAN", &auth_config).map_err(|e| {
            error!("Invalid Barbican service-token configuration: {}", e);
            e
        })?;
    let mut barbican = BarbicanClient::new(config.clone()).with_metrics(metrics.clone());
    match barbican_token_source {
        Some(source) => {
            barbican =
                barbican.with_token_provider(Arc::new(common::ServiceTokenProvider::new(source)));
        }
        None => tracing::warn!(
            "No CITADEL_BARBICAN_CLIENT_ID/SECRET configured: calls to Barbican carry no \
             Authorization header and will be rejected by a JWT-protected Barbican"
        ),
    }
    let barbican_client: Arc<dyn InboxTransitioner> = Arc::new(barbican);
    let executor: Arc<dyn JobExecutor> = match config.job_executor {
        citadel::config::JobExecutorType::Kubernetes => {
            Arc::new(KubernetesExecutor::new(config.clone()).await?)
        }
        citadel::config::JobExecutorType::Mock => Arc::new(MockExecutor::new(config.clone())),
    };

    // Set up notifiers (using no-op implementations for now)
    let catalog_notifier: Arc<dyn CatalogNotifier> = Arc::new(NoOpCatalogNotifier);
    let lta_notifier: Arc<dyn LtaNotifier> = Arc::new(NoOpLtaNotifier);

    // Load callback secret
    let callback_secret: Arc<tokio::sync::RwLock<Option<String>>> = Arc::new(
        tokio::sync::RwLock::new(load_callback_secret(&config.job_callback_secret_file)),
    );

    // Create shared application state
    let app_state = AppState {
        config: Arc::new(config.clone()),
        auth_state,
        storage_manager: Arc::new(storage_manager),
        barbican_client: barbican_client.clone(),
        executor,
        catalog_notifier,
        lta_notifier,
        metrics: metrics.clone(),
        db_client: Arc::new(db_client),
        callback_secret: callback_secret.clone(),
    };

    // Build router (route layout and auth per route group: citadel::create_app)
    let app = citadel::create_app(app_state.clone());

    // P1.6: Apply common logging layer (TraceLayer + TimeoutLayer)
    let app = common::router_logging_layer(app);

    // Start metrics server on separate port using common infrastructure
    let metrics_state = app_state.clone();
    let metrics_router = Router::new().route(
        "/metrics",
        get(move || {
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
                    return (
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        "Failed to encode metrics".to_string(),
                    );
                }
                (axum::http::StatusCode::OK, buffer)
            }
        }),
    );
    if let Err(e) =
        common::start_metrics_server_with_router(app_state.config.prometheus_port, metrics_router)
            .await
    {
        error!("Failed to start metrics server: {}", e);
    }

    // Start background job status poller
    let poller_config = app_state.config.clone();
    let poller_executor = app_state.executor.clone();
    let poller_db = app_state.db_client.clone();
    let poller_barbican = app_state.barbican_client.clone();
    let poller_metrics = app_state.metrics.clone();
    let poller = JobStatusPoller::new(
        poller_config.as_ref().clone(),
        poller_executor,
        poller_db,
        poller_barbican,
        poller_metrics,
    );
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

    let listener = TcpListener::bind(&addr).await.unwrap_or_else(|e| {
        error!("Failed to bind server: {}", e);
        std::process::exit(1);
    });

    // P1.3: Use the shared shutdown signal (created early for Ctrl+C during startup)
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_shared)
        .await
        .unwrap_or_else(|e| {
            error!("Server error: {}", e);
        });

    Ok(())
}
