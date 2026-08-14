use axum::{
    Json,
    extract::{Path, State},
};
use mongodb::bson::{doc, oid::ObjectId};
use std::sync::Arc;

use common::{AppError, AppJson, InboxDB, InboxStatus, InboxStore};
use serde::{Deserialize, Serialize};

/// Request body for publish operation (currently empty, reserved for future use)
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PublishRequest {}

/// Handler to publish an inbox to the data warehouse
pub async fn publish_inbox(
    State(store): State<Arc<dyn InboxStore>>,
    Path(id): Path<ObjectId>,
    Json(_input): Json<PublishRequest>,
) -> Result<AppJson<InboxDB>, AppError> {
    // Find the inbox first
    let mut inbox = store
        .find_one(doc! { "_id": id })
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?
        .ok_or_else(|| AppError::InboxNotFound(id.to_string()))?;

    // Check if inbox is already publishing
    if matches!(inbox.status, InboxStatus::Publishing) {
        return Err(AppError::InboxAlreadyPublishing(
            "Inbox is already being published".to_string(),
        ));
    }

    // Check if inbox is deleted
    if matches!(inbox.status, InboxStatus::Deleted) {
        return Err(AppError::InboxNotFound(
            "Inbox has been deleted".to_string(),
        ));
    }

    // Validate publish parameters (warehouse_path must exist)
    if inbox.warehouse_path.is_empty() {
        return Err(AppError::InvalidPublishParameters(
            "Missing warehouse_path in inbox metadata".to_string(),
        ));
    }

    // Update status to Publishing
    inbox.status = InboxStatus::Publishing;
    inbox.updated_at = chrono::Utc::now();

    // Update in database
    let update = doc! {
        "$set": {
            "status": "publishing",
            "updated_at": mongodb::bson::DateTime::now(),
        }
    };

    store
        .update_one(doc! { "_id": id }, update)
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

    // Return immediately with updated inbox
    // Note: In a full implementation, this would trigger an async job
    // that updates status to "published" or "failed" when complete

    Ok(AppJson::new(inbox))
}
