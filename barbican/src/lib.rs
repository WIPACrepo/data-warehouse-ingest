pub mod handlers;

#[cfg(test)]
pub mod test_utils;

#[cfg(test)]
pub mod tests;

pub use common::{InboxDB, CreateInbox, InboxStore, MockInboxStore, MongoInboxStore};

use axum::{Router, middleware::from_fn, routing::{get, post}};
use common::{log_app_errors, db_collection};
use mongodb::Collection;
use std::sync::Arc;

/// Create the application router with the given store
pub fn create_app(store: Arc<dyn InboxStore>) -> Router {
    let mut router = Router::new()
        .route("/api/v1/inbox", post(handlers::create_inbox))
        .route("/api/v1/inbox/{id}", get(handlers::get_inbox));
    router = common::router_logging_layer(router);
    router
        .layer(from_fn(log_app_errors))
        .with_state(store)
}

/// Serve the application on the given address
pub async fn serve(client: mongodb::Client, bind_address: &str) {
    let listener = tokio::net::TcpListener::bind(bind_address)
        .await
        .unwrap();
    tracing::info!("Listening on {}", listener.local_addr().unwrap());
    let collection: Collection<InboxDB> = db_collection(client, "inbox");
    let store = Arc::new(MongoInboxStore::new(collection));
    let _ = axum::serve(listener, create_app(store))
        .with_graceful_shutdown(common::shutdown_signal())
        .await;
}