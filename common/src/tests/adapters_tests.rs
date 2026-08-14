use crate::adapters::MongoInboxStore;
use crate::models::InboxDB;
use crate::traits::InboxStore;
use mongodb::{Client, Collection};

#[test]
fn test_mongo_inbox_store_new() {
    // We can't create a real collection without MongoDB, but we can test the constructor
    // This is a compile-time check that the type exists
    let _type_check: fn(Collection<InboxDB>) -> MongoInboxStore = MongoInboxStore::new;
}

#[tokio::test]
async fn test_mongo_inbox_store_insert_one() {
    // This test requires a running MongoDB instance
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
        name: "Test Inbox".to_string(),
        size: 100,
        active: true,
    };
    
    let result = store.insert_one(inbox).await;
    
    // Should succeed if MongoDB is running
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_mongo_inbox_store_find_one() {
    // This test requires a running MongoDB instance
    if std::env::var("DATABASE_URL").is_err() {
        println!("Skipping test - no DATABASE_URL set");
        return;
    }
    
    let db_url = std::env::var("DATABASE_URL").unwrap();
    let db_name = std::env::var("DATABASE_NAME").unwrap_or_else(|_| "test_db".to_string());
    
    let client = Client::with_uri_str(&db_url).await.unwrap();
    let collection: Collection<InboxDB> = client.database(&db_name).collection("test_inboxes");
    let store = MongoInboxStore::new(collection);
    
    // First insert a document
    let inbox = InboxDB {
        id: None,
        name: "Find Test".to_string(),
        size: 200,
        active: true,
    };
    
    store.insert_one(inbox.clone()).await.unwrap();
    
    // Then try to find it
    let filter = mongodb::bson::doc! { "name": "Find Test" };
    let result = store.find_one(filter).await;
    
    assert!(result.is_ok());
    let found = result.unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().name, "Find Test");
}