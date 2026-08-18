use crate::adapters::MongoInboxStore;
use mongodb::Collection;

/// Compile-time check that the constructor type is valid
#[test]
fn test_mongo_inbox_store_new() {
    let _type_check: fn(Collection<crate::models::InboxDB>) -> MongoInboxStore =
        MongoInboxStore::new;
}