//! Database connection and collection helpers

use mongodb::{Client, Collection, Database};
use tracing::info;

use crate::error::AppError;

/// Set up the database client
///
/// Takes explicit connection string and database name so each service can
/// supply its own configuration without coupling to environment variables.
pub async fn db_setup(uri: &str, name: &str) -> Result<Client, AppError> {
    let client = Client::with_uri_str(uri)
        .await
        .map_err(|e| AppError::DatabaseError(format!("Failed to create MongoDB client: {e}")))?;

    client
        .database(name)
        .run_command(mongodb::bson::doc! { "ping": 1 })
        .await
        .map_err(|e| AppError::DatabaseError(format!("Failed to ping MongoDB: {e}")))?;

    info!("Successfully connected to MongoDB {}/{}", uri, name);

    Ok(client)
}

/// Get database by name
pub fn db_database(client: &Client, name: &str) -> Database {
    client.database(name)
}

/// Get a collection from the database
pub fn db_collection<T: Send + Sync>(client: &Client, db_name: &str, collection_name: &str) -> Collection<T> {
    db_database(client, db_name).collection(collection_name)
}