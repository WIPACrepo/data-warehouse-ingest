//! Integration tests requiring a running MongoDB instance
//!
//! Run with: `cargo test --package common --features integration-tests,test-utils`

#![cfg(all(feature = "integration-tests", feature = "test-utils"))]

use common::{
    adapters::MongoInboxStore,
    db::{db_collection, db_database, db_setup},
    models::InboxDB,
    test_utils::InboxBuilder,
    traits::InboxStore,
};
use mongodb::Collection;

fn env_uri() -> String {
    std::env::var("DATABASE_URL").unwrap_or_else(|_| "mongodb://localhost:27017".to_string())
}

fn env_db() -> String {
    std::env::var("DATABASE_NAME").unwrap_or_else(|_| "data_warehouse".to_string())
}

#[tokio::test]
async fn test_db_setup_with_default_url() {
    let uri = env_uri();
    let db_name = env_db();
    let result = db_setup(&uri, &db_name).await;
    assert!(result.is_ok());
}

async fn clear_db(client: &mongodb::Client) -> Result<(), mongodb::error::Error> {
    let db_name = env_db();
    let db = db_database(client, &db_name);
    let collection_names = db.list_collection_names().await?;

    for name in collection_names {
        if !name.starts_with("system.") {
            db.collection::<mongodb::bson::Document>(&name)
                .drop()
                .await?;
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_mongo_inbox_store_insert_and_find() {
    let uri = env_uri();
    let db_name = env_db();
    let client = db_setup(&uri, &db_name).await.expect("db_setup failed");
    clear_db(&client).await.expect("clear_db failed");

    let collection: Collection<InboxDB> = db_collection(&client, &db_name, "tests");
    let store = MongoInboxStore::new(collection);

    let inbox = InboxBuilder::new()
        .with_name("Integration Test Inbox")
        .build();

    let result = store.insert_one(inbox.clone()).await;
    assert!(result.is_ok());

    let filter = mongodb::bson::doc! { "name": "Integration Test Inbox" };
    let found = store.find_one(filter).await.unwrap();

    assert!(found.is_some());
    assert_eq!(found.unwrap().name, "Integration Test Inbox");

    clear_db(&client).await.expect("clear_db cleanup failed");
}
