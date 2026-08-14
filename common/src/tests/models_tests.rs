use crate::models::{CreateInbox, InboxDB, InboxStatus};
use mongodb::bson::oid::ObjectId;
use serde_json;
use chrono::Utc;

#[test]
fn test_inbox_db_serialization_roundtrip() {
    let now = Utc::now();
    let inbox = InboxDB {
        id: Some(ObjectId::new()),
        name: "Test Inbox".to_string(),
        size: 1024,
        status: InboxStatus::Active,
        warehouse_path: "/test/path".to_string(),
        lifetime: None,
        archive_enabled: false,
        archive_sites: vec![],
        created_at: now,
        updated_at: now,
    };

    let serialized = serde_json::to_string(&inbox).expect("Failed to serialize");
    let deserialized: InboxDB = serde_json::from_str(&serialized).expect("Failed to deserialize");

    assert_eq!(inbox.name, deserialized.name);
    assert_eq!(inbox.size, deserialized.size);
    assert_eq!(inbox.status, deserialized.status);
    assert_eq!(inbox.warehouse_path, deserialized.warehouse_path);
}

#[test]
fn test_inbox_db_with_empty_name() {
    let now = Utc::now();
    let inbox = InboxDB {
        id: Some(ObjectId::new()),
        name: String::new(),
        size: 0,
        status: InboxStatus::Active,
        warehouse_path: "/test/empty".to_string(),
        lifetime: None,
        archive_enabled: false,
        archive_sites: vec![],
        created_at: now,
        updated_at: now,
    };

    assert_eq!(inbox.name, "");
    assert_eq!(inbox.size, 0);
}

#[test]
fn test_inbox_db_status_serialization() {
    let now = Utc::now();
    let inbox = InboxDB {
        id: Some(ObjectId::new()),
        name: "Status Test".to_string(),
        size: 100,
        status: InboxStatus::Publishing,
        warehouse_path: "/test/status".to_string(),
        lifetime: None,
        archive_enabled: false,
        archive_sites: vec![],
        created_at: now,
        updated_at: now,
    };

    let serialized = serde_json::to_string(&inbox).expect("Failed to serialize");
    assert!(serialized.contains("publishing"));
}

#[test]
fn test_create_inbox_serialization() {
    let create_inbox = CreateInbox {
        name: "New Inbox".to_string(),
        size: 2048,
        warehouse_path: "/test/create".to_string(),
        lifetime: None,
        archive_enabled: false,
        archive_sites: vec![],
    };

    let serialized = serde_json::to_string(&create_inbox).expect("Failed to serialize");
    let deserialized: CreateInbox = serde_json::from_str(&serialized).expect("Failed to deserialize");

    assert_eq!(create_inbox.name, deserialized.name);
    assert_eq!(create_inbox.size, deserialized.size);
    assert_eq!(create_inbox.warehouse_path, deserialized.warehouse_path);
}

#[test]
fn test_create_inbox_with_zero_size() {
    let create_inbox = CreateInbox {
        name: "Zero Size Inbox".to_string(),
        size: 0,
        warehouse_path: "/test/zero".to_string(),
        lifetime: None,
        archive_enabled: false,
        archive_sites: vec![],
    };

    assert_eq!(create_inbox.size, 0);
}

#[test]
fn test_create_inbox_with_archive_settings() {
    let create_inbox = CreateInbox {
        name: "Archive Inbox".to_string(),
        size: 100,
        warehouse_path: "/test/archive".to_string(),
        lifetime: Some(24),
        archive_enabled: true,
        archive_sites: vec!["site1".to_string(), "site2".to_string()],
    };

    assert_eq!(create_inbox.lifetime, Some(24));
    assert_eq!(create_inbox.archive_enabled, true);
    assert_eq!(create_inbox.archive_sites.len(), 2);
}

#[test]
fn test_inbox_db_clone() {
    let now = Utc::now();
    let inbox = InboxDB {
        id: Some(ObjectId::new()),
        name: "Clone Test".to_string(),
        size: 512,
        status: InboxStatus::Active,
        warehouse_path: "/test/clone".to_string(),
        lifetime: None,
        archive_enabled: false,
        archive_sites: vec![],
        created_at: now,
        updated_at: now,
    };

    let cloned = inbox.clone();
    
    assert_eq!(inbox.name, cloned.name);
    assert_eq!(inbox.size, cloned.size);
    assert_eq!(inbox.status, cloned.status);
}