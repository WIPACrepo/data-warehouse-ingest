//! Database connection and collection helpers

use mongodb::{Client, Collection, Database};
use tracing::{info, warn};

use crate::error::AppError;

/// Set up the database client
///
/// Takes explicit connection string and database name so each service can
/// supply its own configuration without coupling to environment variables.
pub async fn db_setup(uri: &str, name: &str) -> Result<Client, AppError> {
    db_setup_with_shutdown(uri, name, std::future::pending).await
}

/// Set up the database client with shutdown signal support
///
/// This variant allows the connection process to be interrupted by a shutdown signal,
/// which is useful during startup when the application might be hung connecting to MongoDB.
///
/// # Arguments
/// * `uri` - MongoDB connection URI
/// * `name` - Database name
/// * `shutdown` - Function that returns a shutdown future (can be called multiple times)
pub async fn db_setup_with_shutdown<S, F>(
    uri: &str,
    name: &str,
    shutdown: S,
) -> Result<Client, AppError>
where
    S: Fn() -> F + Send + Sync,
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let client_fut = Client::with_uri_str(uri);

    tokio::select! {
        result = client_fut => {
            let client = result
                .map_err(|e| AppError::DatabaseError(format!("Failed to create MongoDB client: {e}")))?;

            // Store the database reference before the select
            let db = client.database(name);
            let ping_fut = db.run_command(mongodb::bson::doc! { "ping": 1 });

            tokio::select! {
                result = ping_fut => {
                    result
                        .map_err(|e| AppError::DatabaseError(format!("Failed to ping MongoDB: {e}")))?;

                    info!("Successfully connected to MongoDB {}/{}", uri, name);
                    Ok(client)
                }
                _ = shutdown() => {
                    warn!("Shutdown requested during MongoDB ping");
                    Err(AppError::DatabaseError("Shutdown requested during MongoDB connection".to_string()))
                }
            }
        }
        _ = shutdown() => {
            warn!("Shutdown requested during MongoDB client creation");
            Err(AppError::DatabaseError("Shutdown requested during MongoDB connection".to_string()))
        }
    }
}

/// Get database by name
pub fn db_database(client: &Client, name: &str) -> Database {
    client.database(name)
}

/// Get a collection from the database
pub fn db_collection<T: Send + Sync>(
    client: &Client,
    db_name: &str,
    collection_name: &str,
) -> Collection<T> {
    db_database(client, db_name).collection(collection_name)
}
