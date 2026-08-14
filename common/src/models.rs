use serde::{Deserialize, Serialize};
use mongodb::bson::oid::ObjectId;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum InboxStatus {
    Active,
    Publishing,
    Published,
    Failed,
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
    pub lifetime: Option<u64>,
    #[serde(default)]
    pub archive_enabled: bool,
    #[serde(default)]
    pub archive_sites: Vec<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
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