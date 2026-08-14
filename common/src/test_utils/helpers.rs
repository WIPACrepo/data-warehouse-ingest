use crate::models::{CreateInbox, InboxDB, InboxStatus};
use mongodb::bson::oid::ObjectId;
use chrono::Utc;

/// Creates a default test inbox with common values
pub fn create_default_test_inbox() -> InboxDB {
    let now = Utc::now();
    InboxDB {
        id: Some(ObjectId::new()),
        name: "Default Test Inbox".to_string(),
        size: 100,
        status: InboxStatus::Active,
        warehouse_path: "/test/warehouse/default".to_string(),
        lifetime: None,
        archive_enabled: false,
        archive_sites: vec![],
        created_at: now,
        updated_at: now,
    }
}

/// Creates a test inbox with custom status
pub fn create_test_inbox_with_status(name: &str, size: u64, status: InboxStatus) -> InboxDB {
    let now = Utc::now();
    InboxDB {
        id: Some(ObjectId::new()),
        name: name.to_string(),
        size,
        status,
        warehouse_path: format!("/test/warehouse/{}", name),
        lifetime: None,
        archive_enabled: false,
        archive_sites: vec![],
        created_at: now,
        updated_at: now,
    }
}

/// Creates a test inbox without an ID (for insertion tests)
pub fn create_test_inbox_no_id(name: &str, size: u64) -> InboxDB {
    let now = Utc::now();
    InboxDB {
        id: None,
        name: name.to_string(),
        size,
        status: InboxStatus::Active,
        warehouse_path: format!("/test/warehouse/{}", name),
        lifetime: None,
        archive_enabled: false,
        archive_sites: vec![],
        created_at: now,
        updated_at: now,
    }
}

/// Creates a default CreateInbox request
pub fn create_default_test_request() -> CreateInbox {
    CreateInbox {
        name: "Test Request".to_string(),
        size: 50,
        warehouse_path: "/test/warehouse/request".to_string(),
        lifetime: None,
        archive_enabled: false,
        archive_sites: vec![],
    }
}

/// Asserts that two InboxDB instances are equal (ignoring ID and timestamps)
pub fn assert_inboxes_equal_ignoring_id_and_timestamps(a: &InboxDB, b: &InboxDB) {
    assert_eq!(a.name, b.name, "Inbox names don't match");
    assert_eq!(a.size, b.size, "Inbox sizes don't match");
    assert_eq!(a.status, b.status, "Inbox status doesn't match");
    assert_eq!(a.warehouse_path, b.warehouse_path, "Warehouse paths don't match");
    assert_eq!(a.lifetime, b.lifetime, "Lifetime values don't match");
    assert_eq!(a.archive_enabled, b.archive_enabled, "Archive enabled flags don't match");
    assert_eq!(a.archive_sites, b.archive_sites, "Archive sites don't match");
}