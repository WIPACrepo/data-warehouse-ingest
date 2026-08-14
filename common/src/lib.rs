pub mod adapters;
pub mod config;
pub mod db;
pub mod error;
pub mod metrics;
pub mod middleware;
pub mod models;
pub mod test_utils;
pub mod traits;

pub use adapters::MongoInboxStore;
pub use config::{logging_setup, shutdown_signal};
pub use db::{db_collection, db_setup};
pub use error::{AppError, AppJson, log_app_errors};
pub use metrics::{metrics_app, router_prometheus, start_metrics_server};
pub use middleware::router_logging_layer;
pub use models::{CreateInbox, InboxDB, InboxStatus};
pub use traits::{InboxStore, MockInboxStore};

#[cfg(test)]
mod tests;
