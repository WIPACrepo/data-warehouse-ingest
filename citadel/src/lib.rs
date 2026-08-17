//! Citadel - Backend service for protected data area management
//! 
//! Citadel manages inbox lifecycle, file transfers, and notifications to downstream systems.
//! It consists of two binaries:
//! - `citadel`: Long-running HTTP service
//! - `citadel-job`: Job executor for file transfers (runs as K8s Job or local process)

pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod handlers;
pub mod integrations;
pub mod jobs;
pub mod metrics;
pub mod middleware;
pub mod models;
pub mod polling;
pub mod storage;

pub use config::CitadelConfig;
pub use error::{CitadelError, Result};

use std::sync::Arc;
use mongodb::Client;
use crate::auth::JwksCache;
use crate::storage::manager::StorageManager;
use crate::integrations::barbican::BarbicanClient;
use crate::jobs::executor::JobExecutor;
use crate::integrations::catalog::CatalogNotifier;
use crate::integrations::lta::LtaNotifier;
use crate::metrics::CitadelMetrics;

/// Application state shared across handlers
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<CitadelConfig>,
    pub jwks_cache: Arc<JwksCache>,
    pub storage_manager: Arc<StorageManager>,
    pub barbican_client: Arc<BarbicanClient>,
    pub executor: Arc<dyn JobExecutor>,
    pub catalog_notifier: Arc<dyn CatalogNotifier>,
    pub lta_notifier: Arc<dyn LtaNotifier>,
    pub metrics: Arc<CitadelMetrics>,
    pub db_client: Arc<Client>,
}