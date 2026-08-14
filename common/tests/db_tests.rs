//! Integration tests requiring a running MongoDB instance
//! 
//! Run with: cargo test-integration
//! 
//! Requires DATABASE_URL environment variable to be set.


#![cfg(feature = "integration-tests")]

use common::{
    adapters::MongoInboxStore,
    db::{db_setup, db_collection},
    models::InboxDB,
    traits::InboxStore,
};
use mongodb::{Client, Collection};

#[tokio::test]
async fn test_db_setup_with_default_url() {
    // Clear any existing env vars to use defaults
    unsafe {
        std::env::remove_var("DATABASE_URL");
        std::env::remove_var("DATABASE_NAME");
    }
    
    let result = db_setup().await;
    let _ = result;
}

#[tokio::test]
async fn test_db_setup_with_custom_url() {
    unsafe {
        std::env::set_var("DATABASE_URL", "mongodb://localhost:27017");
        std::env::set_var("DATABASE_NAME", "test_db");
    }
    
    let result = db_setup().await;
    let _ = result;
    
    // Clean up
    unsafe {
        std::env::remove_var("DATABASE_URL");
        std::env::remove_var("DATABASE_NAME");
    }
}

#[tokio::test]
async fn test_mongo_inbox_store_insert_and_find() {
    // Skip if DATABASE_URL is not set
    if std::env::var("DATABASE_URL").is_err() {
        println!("Skipping test - no DATABASE_URL set");
        return;
    }
    
    let db_url = std::env::var("DATABASE_URL").unwrap();
    let db_name = std::env::var("DATABASE_NAME").unwrap_or_else(|_| "test_db".to_string());
    
    let client = Client::with_uri_str(&db_url).await.unwrap();
    let collection: Collection<InboxDB> = client.database(&db_name).collection("test_inboxes");
    let store = MongoInboxStore::new(collection);
    
    let inbox = InboxDB {
        id: None,
        name: "Integration Test Inbox".to_string(),
        size: 100,
        active: true,
    };
    
    let result = store.insert_one(inbox.clone()).await;
    assert!(result.is_ok());
    
    // Then try to find it
    let filter = mongodb::bson::doc! { "name": "Integration Test Inbox" };
    let found = store.find_one(filter).await.unwrap();
    
    assert!(found.is_some());
    assert_eq!(found.unwrap().name, "Integration Test Inbox");
}