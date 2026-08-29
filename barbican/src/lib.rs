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
//! Configuration is loaded from environment variables. See [`config::Config`] for details.
//!
//! # Example
//!
//! ```rust,no_run
//! use barbican::{Config, serve};
//! use mongodb::Client;
//!
//! #[tokio::main]
//! async fn main() {
//!     let config = Config::from_env().expect("Failed to load configuration");
//!     let client = Client::with_uri_str(&config.mongodb_uri)
//!         .await
//!         .expect("Failed to connect to MongoDB");
//!     
//!     serve(client, config).await;
//! }
//! ```

pub mod clients;
pub mod config;
pub mod db;
pub mod docs;
pub mod handlers;
pub mod middleware;

#[cfg(test)]
pub mod test_utils;

#[cfg(test)]
pub mod tests;

pub use common::{AuthConfig, AuthError, AuthMiddlewareState, Claims, ClaimsExtractor, ConfigError, Jwk, Jwks, JwksCache, JwtValidator, auth_middleware, CreateInbox, InboxDB, InboxStatus, InboxStore, MongoInboxStore};
pub use config::Config;

use axum::{
    Router,
    routing::{delete, get, post},
};
use common::{db_collection, log_app_errors};
use mongodb::Collection;
use std::sync::Arc;
use crate::clients::CitadelClient;

/// Application state shared across all handlers
#[derive(Clone)]
pub struct AppState {
    pub store: Arc<dyn InboxStore>,
    pub citadel_client: Option<CitadelClient>,
    pub config: Config,
    pub auth_state: Arc<AuthMiddlewareState>,
}

impl AppState {
    pub fn new(store: Arc<dyn InboxStore>, config: Config) -> Self {
        let citadel_client = CitadelClient::new(&config).ok();
        
        // Initialize common auth components
        let auth_config = AuthConfig::from_env()
            .expect("Failed to load auth config from environment");
        let jwks_cache = Arc::new(JwksCache::new(
            auth_config.jwks_url(),
            auth_config.jwks_refresh_secs,
        ));
        let validator = Arc::new(JwtValidator::new(jwks_cache, auth_config.clone()));
        let auth_state = Arc::new(AuthMiddlewareState {
            validator,
            required_roles: config.required_roles.clone(),
            service_name: "barbican".to_string(),
        });
        
        Self {
            store,
            citadel_client,
            config,
            auth_state,
        }
    }
}

/// Create the application router with the given state
pub fn create_app(state: Arc<AppState>) -> Router {
    // API routes with auth middleware applied per-route using with_state
    let api_routes = Router::new()
        // External API endpoints
        .route("/api/v1/inbox", post(handlers::create_inbox))
        .route("/api/v1/inbox", get(handlers::list_inboxes))
        .route("/api/v1/inbox/{id}", get(handlers::get_inbox))
        .route("/api/v1/inbox/{id}", delete(handlers::delete_inbox))
        .route("/api/v1/inbox/{id}/publish", post(handlers::publish_inbox))
        // Internal API endpoints (Citadel callbacks) - Create operation
        .route("/api/v1/inbox/{id}/create/start", post(handlers::create_start))
        .route("/api/v1/inbox/{id}/create/progress", post(handlers::create_progress))
        .route("/api/v1/inbox/{id}/create/complete", post(handlers::create_complete))
        .route("/api/v1/inbox/{id}/create/failure", post(handlers::create_failure))
        // Internal API endpoints - Publish operation
        .route("/api/v1/inbox/{id}/publish/start", post(handlers::publish_start))
        .route("/api/v1/inbox/{id}/publish/progress", post(handlers::publish_progress))
        .route("/api/v1/inbox/{id}/publish/complete", post(handlers::publish_complete))
        .route("/api/v1/inbox/{id}/publish/failure", post(handlers::publish_failure))
        // Internal API endpoints - Delete operation
        .route("/api/v1/inbox/{id}/delete/start", post(handlers::delete_start))
        .route("/api/v1/inbox/{id}/delete/progress", post(handlers::delete_progress))
        .route("/api/v1/inbox/{id}/delete/complete", post(handlers::delete_complete))
        .route("/api/v1/inbox/{id}/delete/failure", post(handlers::delete_failure))
        .with_state(state.clone());

    // Apply auth middleware to API routes
    let api_routes_with_auth = api_routes.layer(axum::middleware::from_fn_with_state(
        state.auth_state.clone(),
        common::auth_middleware,
    ));

    // Public routes (no auth required)
    let public_routes = Router::new()
        .route("/health", get(handlers::health))
        .route("/ready", get(handlers::ready))
        .with_state(state.clone());

    // Combine routes
    let router = public_routes.merge(api_routes_with_auth);
    
    // Add logging layer
    let router = common::router_logging_layer(router);
    
    // Add error logging layer - need to use the store from state
    router.layer(axum::middleware::from_fn(log_app_errors)).with_state(state.store.clone())
}

/// Serve the application on the given address
pub async fn serve(client: mongodb::Client, config: Config) {
    let listener = tokio::net::TcpListener::bind(&config.bind_address)
        .await
        .unwrap();
    tracing::info!("Listening on {}", listener.local_addr().unwrap());

    let collection: Collection<InboxDB> =
        db_collection(&client, &config.mongodb_database, &config.mongodb_inbox_collection);

    // Ensure the unique warehouse_path index (P1.5, decision D3) so that
    // concurrent duplicate-path creates are rejected with a 409, not just the
    // fast in-process check. Idempotent; failures are logged, not fatal.
    crate::db::ensure_inbox_indexes(&collection).await;

    let store = Arc::new(MongoInboxStore::new(collection));

    let state = Arc::new(AppState::new(store, config));
    let app = create_app(state);

    let _ = axum::serve(listener, app)
        .with_graceful_shutdown(common::shutdown_signal())
        .await;
}