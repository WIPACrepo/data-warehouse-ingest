//! Citadel - Backend service for protected data area management
//!
//! Citadel manages inbox lifecycle, file transfers, and notifications to downstream systems.
//! It consists of two binaries:
//! - `citadel`: Long-running HTTP service
//! - `citadel-job`: Job executor for file transfers (runs as K8s Job or local process)
//!
//! # Documentation
//!
//! For comprehensive documentation, see the [`docs`] module, which links to the
//! markdown files in `src/docs`:
//!
//! - `README.md` - Main index and quick start
//! - `architecture.md` - System architecture and design
//! - `api-reference.md` - Complete API documentation
//! - `state-machine.md` - Job lifecycle and inbox coordination
//! - `authentication.md` - JWT auth, RBAC, and callback security
//! - `barbican-integration.md` - Frontend coordination
//! - `configuration.md` - Environment variables
//! - `development.md` - Development guide
//! - `recursive-transfer.md` - Recursive transfer engine and symlink safety
//! - `symlink-limitations.md` - Symlink edge cases and accepted limits
//! - `security-considerations.md` - Threat model, hardening, incident response

pub mod config;
pub mod db;
pub mod docs;
pub mod error;
pub mod handlers;
pub mod integrations;
pub mod jobs;
pub mod metrics;
pub mod middleware;
pub mod models;
pub mod polling;
pub mod secret_reloader;
pub mod storage;

#[cfg(test)]
pub mod tests;

pub use common::{
    AuthConfig, AuthMiddlewareState, Claims, ClaimsExtractor, JwksCache, JwtValidator,
    auth_middleware, build_auth_state,
};
pub use config::CitadelConfig;
pub use error::{AppError, Result};

use crate::integrations::barbican::InboxTransitioner;
use crate::integrations::catalog::CatalogNotifier;
use crate::integrations::lta::LtaNotifier;
use crate::jobs::executor::JobExecutor;
use crate::metrics::CitadelMetrics;
use crate::storage::manager::StorageManager;
use axum::{
    Router,
    routing::{get, post},
};
use mongodb::Client;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Application state shared across handlers
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<CitadelConfig>,
    pub auth_state: Arc<AuthMiddlewareState>,
    pub storage_manager: Arc<StorageManager>,
    /// Barbican state-transition client (trait object for testability)
    pub barbican_client: Arc<dyn InboxTransitioner>,
    pub executor: Arc<dyn JobExecutor>,
    pub catalog_notifier: Arc<dyn CatalogNotifier>,
    pub lta_notifier: Arc<dyn LtaNotifier>,
    pub metrics: Arc<CitadelMetrics>,
    pub db_client: Arc<Client>,
    /// Callback secret stored with hot-reload support
    pub callback_secret: Arc<RwLock<Option<String>>>,
}

/// Build the Citadel HTTP router (without the logging layer, which `main`
/// adds via `common::router_logging_layer`).
///
/// Three route groups, each with its own authentication:
/// - **Public**: `/health`, `/ready` — no auth.
/// - **User** (`/api/v1/inbox/*`, `/api/v1/job/{id}`, `/api/v1/jobs`, cancel):
///   Bearer JWT with one of `CITADEL_REQUIRED_ROLES` (`common::auth_middleware`).
/// - **Job** (`/api/v1/job/callback`, `/api/v1/job/progress`): either a Bearer
///   JWT with Citadel `admin`, or — with no `Authorization` header — the
///   `citadel-job` credentials (X-Callback-Secret + timestamp + nonce, D9),
///   checked in the handlers. See [`middleware::job_auth`].
pub fn create_app(app_state: AppState) -> Router {
    let user_routes = Router::new()
        .route(
            "/inbox/{id}/create",
            post(handlers::create_inbox::create_inbox),
        )
        .route(
            "/inbox/{id}/publish",
            post(handlers::publish_inbox::publish_inbox),
        )
        .route(
            "/inbox/{id}/delete",
            post(handlers::delete_inbox::delete_inbox),
        )
        .route("/job/{job_id}", get(handlers::get_job::get_job))
        .route("/jobs", get(handlers::list_jobs::list_jobs))
        .route(
            "/job/{job_id}/cancel",
            post(handlers::cancel_job::cancel_job),
        )
        .layer(axum::middleware::from_fn_with_state(
            app_state.auth_state.clone(),
            common::auth_middleware,
        ));

    let job_routes = Router::new()
        .route("/job/callback", post(handlers::job_callback::job_callback))
        .route("/job/progress", post(handlers::job_progress::job_progress))
        .layer(axum::middleware::from_fn_with_state(
            app_state.clone(),
            middleware::job_auth::job_caller_auth,
        ));

    Router::new()
        .route("/health", get(handlers::health::health))
        .route("/ready", get(handlers::ready::ready_with_client))
        .nest("/api/v1", user_routes.merge(job_routes))
        .with_state(app_state)
}
