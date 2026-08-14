use serde::{Deserialize, Serialize};
use mongodb::bson::oid::ObjectId;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct InboxDB {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    pub id: Option<ObjectId>,
    pub name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default = "bool_true")]
    pub active: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CreateInbox {
    pub name: String,
    pub size: u64,
}

fn bool_true() -> bool {
    true
}