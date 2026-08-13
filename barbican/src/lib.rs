pub mod db_adapter;
pub mod db_trait;
pub mod handlers;

#[cfg(test)]
pub mod test_utils;

#[cfg(test)]
pub mod tests;

pub use db_trait::{InboxStore, MockInboxStore};
pub use handlers::{create_inbox, get_inbox};

use axum::{Router, middleware::from_fn, routing::{get, post}};
use common::{log_app_errors};
use mongodb::Collection;
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use mongodb::bson::{oid::ObjectId};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct InboxDB {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    pub id: Option<ObjectId>,
    pub name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default = "bool_true")]
    pub active: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CreateInbox {
    pub name: String,
    pub size: u64,
}

fn bool_true() -> bool {
    true
}

/// Create the application router with the given store
pub fn create_app(store: Arc<dyn InboxStore>) -> Router {
    let mut router = Router::new()
        .route("/api/v1/inbox", post(create_inbox))
        .route("/api/v1/inbox/{id}", get(get_inbox));
    router = common::router_logging_layer(router);
    router
        .layer(from_fn(log_app_errors))
        .with_state(store)
}

/// Serve the application on the given address
pub async fn serve(client: mongodb::Client, bind_address: &str) {
    use common::db_collection;
    use crate::db_adapter::MongoInboxStore;
    
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