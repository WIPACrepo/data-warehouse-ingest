use crate::models::{CreateInbox, InboxDB};
use mongodb::bson::oid::ObjectId;

/// Creates a default test inbox with common values
pub fn create_default_test_inbox() -> InboxDB {
    InboxDB {
        id: Some(ObjectId::new()),
        name: "Default Test Inbox".to_string(),
        size: 100,
        active: true,
    }
}

/// Creates a test inbox with custom active status
pub fn create_test_inbox_with_status(name: &str, size: u64, active: bool) -> InboxDB {
    InboxDB {
        id: Some(ObjectId::new()),
        name: name.to_string(),
        size,
        active,
    }
}

/// Creates a test inbox without an ID (for insertion tests)
pub fn create_test_inbox_no_id(name: &str, size: u64) -> InboxDB {
    InboxDB {
        id: None,
        name: name.to_string(),
        size,
        active: true,
    }
}

/// Creates a default CreateInbox request
pub fn create_default_test_request() -> CreateInbox {
    CreateInbox {
        name: "Test Request".to_string(),
        size: 50,
    }
}

/// Asserts that two InboxDB instances are equal (ignoring ID)
pub fn assert_inboxes_equal_ignoring_id(a: &InboxDB, b: &InboxDB) {
    assert_eq!(a.name, b.name, "Inbox names don't match");
    assert_eq!(a.size, b.size, "Inbox sizes don't match");
    assert_eq!(a.active, b.active, "Inbox active status doesn't match");
}