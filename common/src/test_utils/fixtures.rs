use crate::{MockInboxStore, models::{CreateInbox, InboxDB}};
use mongodb::bson::oid::ObjectId;

/// Creates a test inbox with the given name and size
pub fn create_test_inbox(name: &str, size: u64) -> InboxDB {
    InboxDB {
        id: Some(ObjectId::new()),
        name: name.to_string(),
        size,
        active: true,
    }
}

/// Creates a test inbox with a specific ID
pub fn create_test_inbox_with_id(id: ObjectId, name: &str, size: u64) -> InboxDB {
    InboxDB {
        id: Some(id),
        name: name.to_string(),
        size,
        active: true,
    }
}

/// Creates a pre-configured mock store for testing
pub fn create_test_store() -> MockInboxStore {
    MockInboxStore::new()
}

/// Creates a CreateInbox request body for testing
pub fn create_test_request(name: &str, size: u64) -> CreateInbox {
    CreateInbox {
        name: name.to_string(),
        size,
    }
}