//! Integration tests requiring a running MongoDB instance
//!
//! Run with: cargo test-integration

#![cfg(feature = "integration-tests")]

use common::{
    adapters::MongoInboxStore,
    db::{db_collection, db_database, db_setup},
    models::InboxDB,
    test_utils::InboxBuilder,
    traits::InboxStore,
};
use mongodb::Collection;

#[tokio::test]
async fn test_db_setup_with_default_url() {
    let result = db_setup().await;
    let _ = result;
}

async fn clear_db() -> Result<(), mongodb::error::Error> {
    // 1. Fetch a cursor of all collection names
    let client = db_setup().await;
    let db = db_database(client);
    let collection_names = db.list_collection_names().await?;

    // 2. Iterate and drop each collection
    for name in collection_names {
        // Skip system collections if needed
        if !name.starts_with("system.") {
            db.collection::<mongodb::bson::Document>(&name)
                .drop()
                .await?;
            println!("Dropped collection: {}", name);
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_mongo_inbox_store_insert_and_find() {
    let client = db_setup().await;
    clear_db().await.unwrap();
    let collection: Collection<InboxDB> = db_collection(client, "tests");

    let store = MongoInboxStore::new(collection);

    let inbox = InboxBuilder::new()
        .with_name("Integration Test Inbox")
        .build();

    let result = store.insert_one(inbox.clone()).await;
    assert!(result.is_ok());

    // Then try to find it
    let filter = mongodb::bson::doc! { "name": "Integration Test Inbox" };
    let found = store.find_one(filter).await.unwrap();

    assert!(found.is_some());
    assert_eq!(found.unwrap().name, "Integration Test Inbox");

    clear_db().await.unwrap();
}
