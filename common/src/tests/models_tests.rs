use crate::models::{CreateInbox, InboxStatus};
use crate::test_utils::{CreateInboxBuilder, InboxBuilder};
use serde_json;

#[test]
fn test_inbox_db_serialization_roundtrip() {
    let inbox = InboxBuilder::new()
        .with_name("Test Inbox")
        .with_size(1024)
        .with_warehouse_path("/test/path")
        .build();

    let serialized = serde_json::to_string(&inbox).expect("Failed to serialize");
    let deserialized: crate::models::InboxDB =
        serde_json::from_str(&serialized).expect("Failed to deserialize");

    assert_eq!(inbox.name, deserialized.name);
    assert_eq!(inbox.size, deserialized.size);
    assert_eq!(inbox.status, deserialized.status);
    assert_eq!(inbox.warehouse_path, deserialized.warehouse_path);
}

#[test]
fn test_inbox_db_with_empty_name() {
    let inbox = InboxBuilder::new().with_name("").with_size(0).build();

    assert_eq!(inbox.name, "");
    assert_eq!(inbox.size, 0);
}

#[test]
fn test_inbox_db_status_serialization() {
    let inbox = InboxBuilder::new()
        .with_name("Status Test")
        .with_size(100)
        .with_warehouse_path("/test/status")
        .with_status(InboxStatus::Publishing)
        .build();

    let serialized = serde_json::to_string(&inbox).expect("Failed to serialize");
    assert!(serialized.contains("publishing"));
}

#[test]
fn test_create_inbox_serialization() {
    let create_inbox = CreateInboxBuilder::new()
        .with_name("New Inbox")
        .with_size(2048)
        .with_warehouse_path("/test/create")
        .build();

    let serialized = serde_json::to_string(&create_inbox).expect("Failed to serialize");
    let deserialized: CreateInbox =
        serde_json::from_str(&serialized).expect("Failed to deserialize");

    assert_eq!(create_inbox.name, deserialized.name);
    assert_eq!(create_inbox.size, deserialized.size);
    assert_eq!(create_inbox.warehouse_path, deserialized.warehouse_path);
}

#[test]
fn test_create_inbox_with_zero_size() {
    let create_inbox = CreateInboxBuilder::new()
        .with_name("Zero Size Inbox")
        .with_size(0)
        .with_warehouse_path("/test/zero")
        .build();

    assert_eq!(create_inbox.size, 0);
}

#[test]
fn test_create_inbox_with_archive_settings() {
    let create_inbox = CreateInboxBuilder::new()
        .with_name("Archive Inbox")
        .with_size(100)
        .with_warehouse_path("/test/archive")
        .with_lifetime(24)
        .with_archive_enabled(true)
        .with_archive_sites(vec!["site1".to_string(), "site2".to_string()])
        .build();

    assert_eq!(create_inbox.lifetime, Some(24));
    assert_eq!(create_inbox.archive_enabled, true);
    assert_eq!(create_inbox.archive_sites.len(), 2);
}

#[test]
fn test_inbox_db_clone() {
    let inbox = InboxBuilder::new()
        .with_name("Clone Test")
        .with_size(512)
        .with_warehouse_path("/test/clone")
        .build();

    let cloned = inbox.clone();

    assert_eq!(inbox.name, cloned.name);
    assert_eq!(inbox.size, cloned.size);
    assert_eq!(inbox.status, cloned.status);
}
