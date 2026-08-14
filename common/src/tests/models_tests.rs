use crate::models::{CreateInbox, InboxDB};
use mongodb::bson::oid::ObjectId;
use serde_json;

#[test]
fn test_inbox_db_serialization_roundtrip() {
    let inbox = InboxDB {
        id: Some(ObjectId::new()),
        name: "Test Inbox".to_string(),
        size: 1024,
        active: true,
    };

    let serialized = serde_json::to_string(&inbox).expect("Failed to serialize");
    let deserialized: InboxDB = serde_json::from_str(&serialized).expect("Failed to deserialize");

    assert_eq!(inbox.name, deserialized.name);
    assert_eq!(inbox.size, deserialized.size);
    assert_eq!(inbox.active, deserialized.active);
}

#[test]
fn test_inbox_db_with_empty_name() {
    let inbox = InboxDB {
        id: Some(ObjectId::new()),
        name: String::new(),
        size: 0,
        active: false,
    };

    assert_eq!(inbox.name, "");
    assert_eq!(inbox.size, 0);
    assert_eq!(inbox.active, false);
}

#[test]
fn test_inbox_db_default_active() {
    // Test that bool_true is used as default for active field
    let json = r#"{"name": "Test", "size": 100}"#;
    let inbox: InboxDB = serde_json::from_str(json).expect("Failed to deserialize");
    
    assert_eq!(inbox.name, "Test");
    assert_eq!(inbox.size, 100);
    assert_eq!(inbox.active, true); // Should default to true
}

#[test]
fn test_create_inbox_serialization() {
    let create_inbox = CreateInbox {
        name: "New Inbox".to_string(),
        size: 2048,
    };

    let serialized = serde_json::to_string(&create_inbox).expect("Failed to serialize");
    let deserialized: CreateInbox = serde_json::from_str(&serialized).expect("Failed to deserialize");

    assert_eq!(create_inbox.name, deserialized.name);
    assert_eq!(create_inbox.size, deserialized.size);
}

#[test]
fn test_create_inbox_with_zero_size() {
    let create_inbox = CreateInbox {
        name: "Zero Size Inbox".to_string(),
        size: 0,
    };

    assert_eq!(create_inbox.size, 0);
}

#[test]
fn test_inbox_db_clone() {
    let inbox = InboxDB {
        id: Some(ObjectId::new()),
        name: "Clone Test".to_string(),
        size: 512,
        active: true,
    };

    let cloned = inbox.clone();
    
    assert_eq!(inbox.name, cloned.name);
    assert_eq!(inbox.size, cloned.size);
    assert_eq!(inbox.active, cloned.active);
}