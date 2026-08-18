use axum::{
    Json,
    extract::{Path, State},
};
use mongodb::bson::{doc, oid::ObjectId};
use std::sync::Arc;

use crate::AppState;
use common::{AppError, AppJson, InboxDB, InboxStatus};
use serde::{Deserialize, Serialize};

/// Request body for publish operation (currently empty, reserved for future use)
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PublishRequest {}

/// Handler to publish an inbox
pub async fn publish_inbox(
    State(state): State<Arc<AppState>>,
    Path(id): Path<ObjectId>,
    Json(_input): Json<PublishRequest>,
) -> Result<AppJson<InboxDB>, AppError> {
    // Find the inbox first
    let mut inbox = state.store
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

    // Check if inbox is in a terminal state
    if matches!(inbox.status, InboxStatus::Published | InboxStatus::Deleted) {
        return Err(AppError::InvalidStateTransition(format!(
            "Cannot publish inbox in state {:?}",
            inbox.status
        )));
    }

    // Must be Ready to publish
    if !matches!(inbox.status, InboxStatus::Ready) {
        return Err(AppError::InvalidStateTransition(format!(
            "Cannot publish inbox in state {:?}; must be Ready",
            inbox.status
        )));
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

    state.store
        .update_one(doc! { "_id": id }, update)
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

    // Trigger Citadel publish operation
    if let Some(ref citadel) = state.citadel_client {
        let citadel_clone = citadel.clone();
        let inbox_id_str = id.to_hex();
        tokio::spawn(async move {
            match citadel_clone.trigger_publish(&inbox_id_str).await {
                Ok(resp) => tracing::info!("Citadel publish triggered: job_id={}", resp.job_id),
                Err(e) => tracing::error!("Failed to trigger Citadel publish: {}", e),
            }
        });
    }

    Ok(AppJson::new(inbox))
}