use mongodb::{Client, Collection, Database};
use tracing::info;

/// Set up the database client
pub async fn db_setup() -> Client {
    // connecting to mongodb
    let db_connection_str = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        "mongodb://localhost:27017".to_string()
    });
    let client = Client::with_uri_str(db_connection_str.as_str()).await.unwrap();

    // pinging the database
    let db_name_str = std::env::var("DATABASE_NAME").unwrap_or_else(|_| {
        "data-warehouse-ingress".to_string()
    });
    client
        .database(db_name_str.as_str())
        .run_command(mongodb::bson::doc! { "ping": 1 })
        .await
        .unwrap();
    info!("Successfully connected to MongoDB {}/{}", db_connection_str.as_str(), db_name_str.as_str());

    client
}

/// Get database
pub fn db_database(client: Client) -> Database {
    let db_name_str = std::env::var("DATABASE_NAME").unwrap_or_else(|_| {
        "data-warehouse-ingress".to_string()
    });
    client.database(db_name_str.as_str())
}

/// Get a collection from the database
pub fn db_collection<T: Send + Sync>(client: Client, name: &str) -> Collection<T> {
    db_database(client).collection(name)
}