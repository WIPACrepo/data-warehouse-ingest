//! Database connection and collection helpers

use mongodb::{Client, Collection, Database};
use tracing::info;

use crate::config::CitadelConfig;
use crate::error::{CitadelError, Result};

/// Set up the database client
pub async fn db_setup(config: &CitadelConfig) -> Result<Client> {
    let client = Client::with_uri_str(&config.mongodb_uri)
        .await
        .map_err(|e| CitadelError::DatabaseError(format!("Failed to create MongoDB client: {}", e)))?;

    // Pinging the database
    let db = client.database(&config.mongodb_database);
    db.run_command(mongodb::bson::doc! { "ping": 1 })
        .await
        .map_err(|e| CitadelError::DatabaseError(format!("Failed to ping MongoDB: {}", e)))?;
    
    info!(
        "Successfully connected to MongoDB {}/{}",
        config.mongodb_uri, config.mongodb_database
    );

    Ok(client)
}

/// Get database
pub fn db_database(client: &Client, config: &CitadelConfig) -> Database {
    client.database(&config.mongodb_database)
}

/// Get a collection from the database
pub fn db_collection<T: Send + Sync>(client: &Client, config: &CitadelConfig, name: &str) -> Collection<T> {
    client.database(&config.mongodb_database).collection(name)
}

/// Collection names
pub const COLLECTION_INBOXES: &str = "inboxes";
pub const COLLECTION_JOBS: &str = "jobs";
pub const COLLECTION_CALLBACK_NONCES: &str = "callback_nonces";

/// Get the inboxes collection (shared with Barbican, read-only for Citadel)
pub fn inboxes_collection(client: &Client, config: &CitadelConfig) -> Collection<common::models::InboxDB> {
    db_collection(client, config, COLLECTION_INBOXES)
}

/// Get the jobs collection (Citadel-owned)
pub fn jobs_collection(client: &Client, config: &CitadelConfig) -> Collection<crate::models::JobDB> {
    db_collection(client, config, COLLECTION_JOBS)
}

/// Get the callback nonces collection (Citadel-owned)
pub fn callback_nonces_collection(client: &Client, config: &CitadelConfig) -> Collection<crate::models::CallbackNonceDB> {
    db_collection(client, config, COLLECTION_CALLBACK_NONCES)
}