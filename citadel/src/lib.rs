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
