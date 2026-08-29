use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum InboxStatus {
    Active,
    Creating,
    Ready,
    Publishing,
    Published,
    Failed,
    Deleting,
    Deleted,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct InboxDB {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    pub id: Option<ObjectId>,
    pub name: String,
    #[serde(default)]
    pub size: u64,
    pub status: InboxStatus,
    pub warehouse_path: String,
    #[serde(default)]
    pub inbox_path: Option<String>,
    #[serde(default)]
    pub lifetime: Option<u64>,
    #[serde(default)]
    pub archive_enabled: bool,
    #[serde(default)]
    pub archive_sites: Vec<String>,
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub current_job_id: Option<String>,
    #[serde(default)]
    pub progress: Option<InboxProgress>,
    #[serde(default)]
    pub error: Option<InboxError>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Progress tracking for inbox operations
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct InboxProgress {
    pub phase: String,
    #[serde(default)]
    pub total_files: u64,
    #[serde(default)]
    pub processed_files: u64,
    #[serde(default)]
    pub total_bytes: u64,
    #[serde(default)]
    pub transferred_bytes: u64,
    #[serde(default)]
    pub deleted_bytes: u64,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Error information for failed operations
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct InboxError {
    pub code: String,
    pub message: String,
    pub job_id: String,
    pub occurred_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CreateInbox {
    pub name: String,
    pub size: u64,
    pub warehouse_path: String,
    #[serde(default)]
    pub lifetime: Option<u64>,
    #[serde(default)]
    pub archive_enabled: bool,
    #[serde(default)]
    pub archive_sites: Vec<String>,
}
