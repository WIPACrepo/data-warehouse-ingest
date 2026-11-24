use crate::{
    MockInboxStore,
    models::{CreateInbox, InboxDB, InboxStatus},
};
use chrono::Utc;
use mongodb::bson::oid::ObjectId;

/// Creates a test inbox with the given name and size
pub fn create_test_inbox(name: &str, size: u64) -> InboxDB {
    let now = Utc::now();
    InboxDB {
        id: Some(ObjectId::new()),
        name: name.to_string(),
        size,
        status: InboxStatus::Active,
        warehouse_path: format!("/test/warehouse/{}", name),
        inbox_path: None,
        lifetime: None,
        archive_enabled: false,
        archive_sites: vec![],
        version: 0,
        current_job_id: None,
        progress: None,
        error: None,
        created_at: now,
        updated_at: now,
    }
}

/// Creates a test inbox with a specific ID
pub fn create_test_inbox_with_id(id: ObjectId, name: &str, size: u64) -> InboxDB {
    let now = Utc::now();
    InboxDB {
        id: Some(id),
        name: name.to_string(),
        size,
        status: InboxStatus::Active,
        warehouse_path: format!("/test/warehouse/{}", name),
        inbox_path: None,
        lifetime: None,
        archive_enabled: false,
        archive_sites: vec![],
        version: 0,
        current_job_id: None,
        progress: None,
        error: None,
        created_at: now,
        updated_at: now,
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
        warehouse_path: format!("/test/warehouse/{}", name),
        lifetime: None,
        archive_enabled: false,
        archive_sites: vec![],
    }
}
