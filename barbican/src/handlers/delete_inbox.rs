use axum::{extract::{Path, State}, http::StatusCode};
use mongodb::bson::{doc, oid::ObjectId};
use std::sync::Arc;

use common::{InboxStore, InboxStatus, AppError};

/// Handler to delete an inbox by ID (soft delete)
pub async fn delete_inbox(
    State(store): State<Arc<dyn InboxStore>>,
    Path(id): Path<ObjectId>,
) -> Result<StatusCode, AppError> {
    // Find the inbox first
    let inbox = store
        .find_one(doc! { "_id": id })
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?
        .ok_or_else(|| AppError::InboxNotFound(id.to_string()))?;
    
    // Check if inbox can be deleted (not publishing)
    if matches!(inbox.status, InboxStatus::Publishing) {
        return Err(AppError::InboxNotDeletable(
            "Inbox is currently being published".to_string()
        ));
    }
    
    // Update status to Deleted (soft delete)
    let update = doc! {
        "$set": {
            "status": "deleted",
            "updated_at": mongodb::bson::DateTime::now(),
        }
    };
    
    store
        .update_one(doc! { "_id": id }, update)
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;
    
    Ok(StatusCode::NO_CONTENT)
}