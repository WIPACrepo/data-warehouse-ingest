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
use tokio::signal;
use tracing::{info, error};
use tracing_subscriber::{fmt, prelude::*};

use citadel::config::CitadelConfig;
use citadel::auth::JwksCache;
use citadel::db::db_setup;
use citadel::middleware::auth::auth_middleware;
use citadel::middleware::logging::logging_middleware;
use citadel::handlers;
use citadel::integrations::barbican::BarbicanClient;
use citadel::integrations::catalog::{CatalogNotifier, NoOpCatalogNotifier};
use citadel::integrations::lta::{LtaNotifier, NoOpLtaNotifier};
use citadel::jobs::executor::{JobExecutor, KubernetesExecutor, MockExecutor};
use citadel::storage::manager::StorageManager;
use citadel::metrics::CitadelMetrics;
use citadel::AppState;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::registry()
        .with(fmt::layer())
        .init();

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

    // Set up database connection
    let db_client = db_setup(&config).await?;

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
    };

    // Build router
    let app = Router::new()
        // Health and readiness endpoints (no auth required)
        .route("/health", get(handlers::health::health))
        .route("/ready", get(handlers::ready::ready_with_client))
        // All API routes require authentication
        .nest(
            "/api/v1",
            Router::new()
                .route("/inbox/{id}/create", post(handlers::create_inbox::create_inbox))
                .route("/inbox/{id}/publish", post(handlers::publish_inbox::publish_inbox))
                .route("/inbox/{id}/delete", post(handlers::delete_inbox::delete_inbox))
                .route("/job/{job_id}", get(handlers::get_job::get_job))
                .route("/jobs", get(handlers::list_jobs::list_jobs))
                .route("/job/{job_id}/cancel", post(handlers::cancel_job::cancel_job))
                .route("/job/callback", post(handlers::job_callback::job_callback))
                .layer(axum::middleware::from_fn_with_state(app_state.clone(), auth_middleware))
        )
        // Add global logging middleware
        .layer(axum::middleware::from_fn(logging_middleware))
        .with_state(app_state.clone());

    // Start metrics server on separate port
    let metrics_app_state = app_state.clone();
    let metrics_port = app_state.config.prometheus_port;
    tokio::spawn(async move {
        let metrics_router = Router::new()
            .route("/metrics", get(move || {
                let state = metrics_app_state.clone();
                async move {
                    match state.metrics.registry.gather().first() {
                        Some(mf) => {
                            let encoder = prometheus::TextEncoder::new();
                            let mut buffer = String::new();
                            if let Err(e) = encoder.encode_utf8(&[mf.clone()], &mut buffer) {
                                error!("Failed to encode metrics: {}", e);
                                return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "Failed to encode metrics".to_string());
                            }
                            (axum::http::StatusCode::OK, buffer)
                        }
                        None => (axum::http::StatusCode::OK, String::new()),
                    }
                }
            }));
        
        let metrics_addr = format!("0.0.0.0:{}", metrics_port);
        info!("Starting metrics server on {}", metrics_addr);
        
        let listener = TcpListener::bind(&metrics_addr).await
            .unwrap_or_else(|e| {
                error!("Failed to bind metrics server: {}", e);
                std::process::exit(1);
            });
        axum::serve(listener, metrics_router)
            .await
            .unwrap_or_else(|e| error!("Metrics server error: {}", e));
    });

    // Start main HTTP server
    let addr = app_state.config.bind_address.clone();
    info!("Starting Citadel HTTP server on {}", addr);

    let listener = TcpListener::bind(&addr).await
        .unwrap_or_else(|e| {
            error!("Failed to bind server: {}", e);
            std::process::exit(1);
        });
    
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap_or_else(|e| {
            error!("Server error: {}", e);
        });

    Ok(())
}

async fn shutdown_signal() {
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

    info!("Shutdown signal received, shutting down gracefully");
}