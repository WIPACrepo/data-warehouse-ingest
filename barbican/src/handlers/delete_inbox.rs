use axum::{
    extract::{Path, State},
    http::StatusCode,
};
use mongodb::bson::{doc, oid::ObjectId};
use std::sync::Arc;

use crate::AppState;
use common::{AppError, InboxStatus};

/// Handler to delete an inbox by ID (soft delete)
pub async fn delete_inbox(
    State(state): State<Arc<AppState>>,
    Path(id): Path<ObjectId>,
) -> Result<StatusCode, AppError> {
    // Find the inbox first
    let inbox = state.store
        .find_one(doc! { "_id": id })
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?
        .ok_or_else(|| AppError::InboxNotFound(id.to_string()))?;

    // Check if inbox can be deleted (only from Ready or Failed states)
    if !matches!(inbox.status, InboxStatus::Ready | InboxStatus::Failed) {
        return Err(AppError::InboxNotDeletable(format!(
            "Inbox cannot be deleted from state {:?}",
            inbox.status
        )));
    }

    // Update status to Deleting
    let update = doc! {
        "$set": {
            "status": "deleting",
            "updated_at": mongodb::bson::DateTime::now(),
        }
    };

    state.store
        .update_one(doc! { "_id": id }, update)
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

    // Trigger Citadel delete operation
    if let Some(ref citadel) = state.citadel_client {
        let citadel_clone = citadel.clone();
        let inbox_id_str = id.to_hex();
        tokio::spawn(async move {
            match citadel_clone.trigger_delete(&inbox_id_str).await {
                Ok(resp) => tracing::info!("Citadel delete triggered: job_id={}", resp.job_id),
                Err(e) => tracing::error!("Failed to trigger Citadel delete: {}", e),
            }
        });
    }

    Ok(StatusCode::NO_CONTENT)
}