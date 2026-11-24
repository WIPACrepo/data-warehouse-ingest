//! Common - Shared library for the data warehouse ingestion system
//!
//! `common` is a workspace library crate shared by the two services of the data
//! warehouse ingestion system:
//!
//! - [**Barbican**](../barbican/index.html) - user-facing frontend that manages inbox metadata
//! - [**Citadel**](../citadel/index.html) - backend that controls protected data areas
//!
//! It defines the shared inbox data model, the storage abstraction, unified error
//! handling, and a set of infrastructure utilities (config/logging, database
//! helpers, metrics, and HTTP middleware) so the services never drift apart.
//!
//! # Documentation
//!
//! For comprehensive documentation, see the [`docs`] module, which links to the
//! markdown files in `src/docs`:
//!
//! - [`README.md`](docs/README.md) - Main index with quick start and overview
//! - [`overview.md`](docs/overview.md) - How `common` fits into the architecture
//! - [`models.md`](docs/models.md) - Shared data model (inboxes, statuses, progress)
//! - [`auth.md`](docs/auth.md) - JWT/Keycloak auth: prefixed env config, validation, middleware
//! - [`infrastructure.md`](docs/infrastructure.md) - Shared utilities (config, db, error, metrics, middleware)
//! - [`testing.md`](docs/testing.md) - Test utilities and conventions
//!
//! # Key Exports
//!
//! - **Models**: [`CreateInbox`], [`InboxDB`], [`InboxError`], [`InboxProgress`], [`InboxStatus`]
//! - **Storage**: [`InboxStore`] trait, [`MongoInboxStore`] adapter
//! - **Errors**: [`AppError`], [`AppJson`], [`log_app_errors`]
//! - **Infrastructure**: [`db_collection`], [`db_setup`], [`logging_setup`], [`shutdown_signal`],
//!   [`metrics_app`], [`router_prometheus`], [`start_metrics_server`], [`router_logging_layer`]

pub mod adapters;
#[cfg(feature = "auth")]
pub mod auth;
pub mod config;
pub mod db;
pub mod docs;
pub mod error;
pub mod metrics;
pub mod middleware;
pub mod models;
#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;
pub mod traits;

pub use adapters::MongoInboxStore;
#[cfg(feature = "auth")]
pub use auth::{
    AuthConfig, AuthError, AuthMiddlewareState, Claims, ClaimsExtractor, ConfigError, Jwk, Jwks,
    JwksCache, JwtValidator, auth_middleware, build_auth_state,
};
pub use config::{logging_setup, shutdown_signal};
pub use db::{db_collection, db_database, db_setup, db_setup_with_shutdown};
pub use error::{AppError, AppJson, log_app_errors};
#[cfg(feature = "auth")]
pub use metrics::init_auth_metrics;
pub use metrics::{
    metrics_app, router_prometheus, start_metrics_server, start_metrics_server_with_router,
};
pub use middleware::router_logging_layer;
pub use models::{CreateInbox, InboxDB, InboxError, InboxProgress, InboxStatus};
pub use traits::InboxStore;

#[cfg(any(test, feature = "test-utils"))]
pub use traits::MockInboxStore;

#[cfg(test)]
mod tests;
