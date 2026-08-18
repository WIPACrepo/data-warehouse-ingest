use common::models::{CreateInbox, InboxDB};
use mongodb::bson::oid::ObjectId;

/// Creates a test inbox with the given name and size
pub fn create_test_inbox(name: &str, size: u64) -> InboxDB {
    common::test_utils::InboxBuilder::new()
        .with_name(name)
        .with_size(size)
        .with_warehouse_path(&format!("/test/warehouse/{name}"))
        .build()
}

/// Creates a test inbox with a specific ID
pub fn create_test_inbox_with_id(id: ObjectId, name: &str, size: u64) -> InboxDB {
    common::test_utils::InboxBuilder::new()
        .with_id(id)
        .with_name(name)
        .with_size(size)
        .with_warehouse_path(&format!("/test/warehouse/{name}"))
        .build()
}

/// Creates a pre-configured mock store for testing
pub fn create_test_store() -> common::MockInboxStore {
    common::MockInboxStore::new()
}

/// Creates a CreateInbox request body for testing
pub fn create_test_request(name: &str, size: u64) -> CreateInbox {
    common::test_utils::CreateInboxBuilder::new()
        .with_name(name)
        .with_size(size)
        .with_warehouse_path(&format!("/test/warehouse/{name}"))
        .build()
}