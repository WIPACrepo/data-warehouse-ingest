//! Barbican - Data Warehouse Ingestion Frontend Service
//!
//! Barbican is a long-running HTTP service that provides a secure API for managing
//! inbox metadata and coordinating data ingestion operations with the Citadel backend.
//!
//! # Documentation
//!
//! For comprehensive documentation, see the [`docs`] module:
//!
//! - [`docs::architecture`](crate::docs) - System architecture and design
//! - [`docs::api-reference`](crate::docs) - Complete API documentation
//! - [`docs::state-machine`](crate::docs) - Inbox lifecycle
//! - [`docs::authentication`](crate::docs) - JWT auth and RBAC
//! - [`docs::citadel-integration`](crate::docs) - Backend coordination
//! - [`docs::configuration`](crate::docs) - Environment variables
//! - [`docs::development`](crate::docs) - Development guide
//!
//! # Architecture
//!
//! ```text
//! +-------------+      JWT      +-------------+
//! |  External   | ------------> |  Barbican   |
//! |  Clients    |               |  (Frontend) |
//! +-------------+               +------+------+
//!                                       |
//!                                       | JWT (internal role)
//!                                       v
//!                               +-------------+
//!                               |  Citadel    |
//!                               |  (Backend)  |
//!                               +------+------+
//! ```
//!
//! # Key Features
//!
//! - **Inbox Metadata Management**: CRUD operations for inbox records in MongoDB
//! - **State Machine Orchestration**: Manages inbox lifecycle through well-defined states
//! - **JWT Authentication**: Keycloak-based authentication with role authorization
//! - **Automatic Citadel Triggers**: Coordinates backend operations transparently
//! - **Progress Tracking**: Real-time updates during long-running operations
//! - **Optimistic Locking**: Prevents concurrent modification conflicts
//!
//! # API Endpoints
//!
//! ## External (Client-Facing)
//!
//! - `POST /api/v1/inbox` - Create inbox metadata
//! - `GET /api/v1/inbox/{id}` - Get inbox details
//! - `GET /api/v1/inbox` - List all inboxes
//! - `DELETE /api/v1/inbox/{id}` - Request inbox deletion
//! - `POST /api/v1/inbox/{id}/publish` - Publish inbox to warehouse
//!
//! ## Internal (Citadel Callbacks)
//!
//! - `/api/v1/inbox/{id}/create/{start,progress,complete,failure}`
//! - `/api/v1/inbox/{id}/publish/{start,progress,complete,failure}`
//! - `/api/v1/inbox/{id}/delete/{start,progress,complete,failure}`
//!
//! # Configuration
//!
//! Configuration is loaded from environment variables. See [`config::Config`]
//! for service settings and [`AuthConfig::from_env_prefixed`] (with prefix
//! `BARBICAN`) for Keycloak/JWT settings.
//!
//! # Example
//!
//! ```rust,no_run
//! use barbican::{AuthConfig, Config, serve};
//! use mongodb::Client;
//!
//! #[tokio::main]
//! async fn main() {
//!     let config = Config::from_env().expect("Failed to load configuration");
//!     // Reads BARBICAN_KEYCLOAK_URL, BARBICAN_REQUIRED_ROLES, ... (D10)
//!     let auth_config = AuthConfig::from_env_prefixed("BARBICAN")
//!         .expect("Failed to load auth configuration");
//!     let client = Client::with_uri_str(&config.mongodb_uri)
//!         .await
//!         .expect("Failed to connect to MongoDB");
//!
//!     serve(client, config, auth_config).await;
//! }
//! ```

pub mod clients;
pub mod config;
pub mod db;
pub mod docs;
pub mod handlers;
pub mod metrics;
pub mod middleware;

#[cfg(test)]
pub mod test_utils;

#[cfg(test)]
pub mod tests;

pub use common::{
    AuthConfig, AuthError, AuthMiddlewareState, Claims, ClaimsExtractor, ConfigError, CreateInbox,
    InboxDB, InboxStatus, InboxStore, Jwk, Jwks, JwksCache, JwtValidator, MongoInboxStore,
    auth_middleware,
};
pub use config::Config;

use crate::clients::CitadelClient;
use axum::{
    Router,
    routing::{delete, get, post},
};
use common::{db_collection, log_app_errors};
use mongodb::Collection;
use std::sync::Arc;

/// Application state shared across all handlers
#[derive(Clone)]
pub struct AppState {
    pub store: Arc<dyn InboxStore>,
    pub citadel_client: Option<CitadelClient>,
    pub config: Config,
    pub auth_state: Arc<AuthMiddlewareState>,
    pub metrics: Arc<metrics::BarbicanMetrics>,
}

impl AppState {
    /// Build the application state.
    ///
    /// `auth_state` is built by the caller (see [`serve`]) from
    /// `AuthConfig::from_env_prefixed("BARBICAN")` via
    /// [`common::build_auth_state`], so configuration errors surface at
    /// startup as a `Result` instead of a panic here.
    pub fn new(
        store: Arc<dyn InboxStore>,
        config: Config,
        auth_state: Arc<AuthMiddlewareState>,
    ) -> Self {
        let citadel_client = CitadelClient::new(&config).ok();

        let metrics = Arc::new(
            metrics::BarbicanMetrics::new().expect("Failed to initialize Barbican metrics"),
        );
        metrics.spawn_inbox_refresh_loop(store.clone(), 30);

        Self {
            store,
            citadel_client,
            config,
            auth_state,
            metrics,
        }
    }
}

/// Create the application router with the given state
pub fn create_app(state: Arc<AppState>) -> Router {
    // External, client-facing routes — role-gated via `state.auth_state`
    // (the configured `required_roles`).
    let external_routes = Router::new()
        .route("/api/v1/inbox", post(handlers::create_inbox))
        .route("/api/v1/inbox", get(handlers::list_inboxes))
        .route("/api/v1/inbox/{id}", get(handlers::get_inbox))
        .route("/api/v1/inbox/{id}", delete(handlers::delete_inbox))
        .route("/api/v1/inbox/{id}/publish", post(handlers::publish_inbox))
        .with_state(state.clone());

    // Internal (Citadel callback) routes — Bearer-only, NO role gate (D9).
    // Citadel is the only legitimate caller and authenticates to Barbican; an
    // empty `required_roles` list makes `auth_middleware` skip the role check
    // (any valid service token passes — see common::auth_middleware).
    let internal_auth_state = Arc::new(AuthMiddlewareState {
        validator: state.auth_state.validator.clone(),
        required_roles: Vec::new(),
        service_name: "barbican".to_string(),
    });

    let internal_routes = Router::new()
        // Create operation
        .route(
            "/api/v1/inbox/{id}/create/start",
            post(handlers::create_start),
        )
        .route(
            "/api/v1/inbox/{id}/create/progress",
            post(handlers::create_progress),
        )
        .route(
            "/api/v1/inbox/{id}/create/complete",
            post(handlers::create_complete),
        )
        .route(
            "/api/v1/inbox/{id}/create/failure",
            post(handlers::create_failure),
        )
        // Publish operation
        .route(
            "/api/v1/inbox/{id}/publish/start",
            post(handlers::publish_start),
        )
        .route(
            "/api/v1/inbox/{id}/publish/progress",
            post(handlers::publish_progress),
        )
        .route(
            "/api/v1/inbox/{id}/publish/complete",
            post(handlers::publish_complete),
        )
        .route(
            "/api/v1/inbox/{id}/publish/failure",
            post(handlers::publish_failure),
        )
        // Delete operation
        .route(
            "/api/v1/inbox/{id}/delete/start",
            post(handlers::delete_start),
        )
        .route(
            "/api/v1/inbox/{id}/delete/progress",
            post(handlers::delete_progress),
        )
        .route(
            "/api/v1/inbox/{id}/delete/complete",
            post(handlers::delete_complete),
        )
        .route(
            "/api/v1/inbox/{id}/delete/failure",
            post(handlers::delete_failure),
        )
        .with_state(state.clone())
        .layer(axum::middleware::from_fn_with_state(
            internal_auth_state,
            common::auth_middleware,
        ));

    // Apply the role-gated auth middleware to external routes
    let external_routes_with_auth = external_routes.layer(axum::middleware::from_fn_with_state(
        state.auth_state.clone(),
        common::auth_middleware,
    ));

    // Public routes (no auth required)
    let public_routes = Router::new()
        .route("/health", get(handlers::health))
        .route("/ready", get(handlers::ready))
        .with_state(state.clone());

    // Combine routes
    let router = public_routes
        .merge(external_routes_with_auth)
        .merge(internal_routes);

    // Record per-request metrics (endpoint template + method + status). Applied
    // after auth so the counter only sees requests that reached routing.
    let router = router.layer(axum::middleware::from_fn_with_state(
        state.metrics.clone(),
        metrics::record_request_metrics,
    ));

    // Add logging layer
    let router = common::router_logging_layer(router);

    // Add error logging layer - need to use the store from state
    router
        .layer(axum::middleware::from_fn(log_app_errors))
        .with_state(state.store.clone())
}

/// Serve the application on the given address
///
/// `auth_config` is loaded by the caller with
/// `AuthConfig::from_env_prefixed("BARBICAN")` (decision D10).
pub async fn serve(client: mongodb::Client, config: Config, auth_config: AuthConfig) {
    let listener = tokio::net::TcpListener::bind(&config.bind_address)
        .await
        .unwrap();
    tracing::info!("Listening on {}", listener.local_addr().unwrap());

    let collection: Collection<InboxDB> = db_collection(
        &client,
        &config.mongodb_database,
        &config.mongodb_inbox_collection,
    );

    // Ensure the unique warehouse_path index (P1.5, decision D3) so that
    // concurrent duplicate-path creates are rejected with a 409, not just the
    // fast in-process check. Idempotent; failures are logged, not fatal.
    crate::db::ensure_inbox_indexes(&collection).await;

    let store = Arc::new(MongoInboxStore::new(collection));

    let auth_state = common::build_auth_state(&auth_config, "barbican");
    let state = Arc::new(AppState::new(store, config, auth_state));
    let app = create_app(state.clone());

    // Serve Barbican's own prometheus registry on the metrics port (P4.3).
    // Citadel's metrics server renders its registry the same way; Barbican
    // previously used the plain start_metrics_server (default registry), which
    // did not include barbican_inboxes_total / barbican_api_requests_total.
    let metrics_router = metrics::metrics_router(state.metrics.clone());
    if let Err(e) =
        common::start_metrics_server_with_router(state.config.prometheus_port, metrics_router).await
    {
        tracing::error!("Failed to start metrics server: {e}");
    }

    let _ = axum::serve(listener, app)
        .with_graceful_shutdown(common::shutdown_signal())
        .await;
}
