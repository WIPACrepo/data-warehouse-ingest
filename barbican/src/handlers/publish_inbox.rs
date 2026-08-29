use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use mongodb::bson::{doc, oid::ObjectId};
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;
use common::{AppError, InboxStatus};

/// Response body for inbox publish (202 Accepted)
#[derive(Debug, Serialize)]
pub struct PublishInboxResponse {
    pub inbox_id: String,
    pub status: String,
    pub message: String,
}

/// Handler to publish an inbox
///
/// Takes no request body. Validates the inbox is `Ready`, transitions it to
/// `Publishing`, fires the Citadel publish trigger asynchronously, and
/// returns `202 Accepted`.
pub async fn publish_inbox(
    State(state): State<Arc<AppState>>,
    Path(id): Path<ObjectId>,
) -> Result<(StatusCode, Json<PublishInboxResponse>), AppError> {
    // Find the inbox first
    let inbox = state.store
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

    // Update status to Publishing (version-conditional optimistic lock)
    let update = doc! {
        "$set": {
            "status": "publishing",
            "updated_at": mongodb::bson::DateTime::now(),
            "version": inbox.version + 1,
        }
    };

    crate::handlers::transition::apply_versioned_update(
        &state.store,
        &id,
        inbox.version,
        &update,
    )
    .await?;

    // Trigger Citadel publish operation (fire-and-forget). On failure, apply
    // the in-Barbican fallback (D2) so the inbox doesn't stay `Publishing`.
    if let Some(ref citadel) = state.citadel_client {
        let citadel_clone = citadel.clone();
        let store = state.store.clone();
        tokio::spawn(async move {
            match citadel_clone.trigger_publish(&id.to_hex()).await {
                Ok(resp) => tracing::info!("Citadel publish triggered: job_id={}", resp.job_id),
                Err(e) => {
                    tracing::error!("Failed to trigger Citadel publish: {}", e);
                    if let Err(ferr) = crate::handlers::transition::apply_trigger_failure(
                        &store,
                        &id,
                        "citadel_trigger_failed",
                        &format!("Citadel publish trigger failed: {}", e),
                    )
                    .await
                    {
                        tracing::warn!(
                            "Failed to apply publish-trigger failure fallback: {}",
                            ferr
                        );
                    }
                }
            }
        });
    }

    Ok((
        StatusCode::ACCEPTED,
        Json(PublishInboxResponse {
        inbox_id: id.to_hex(),
        status: "publishing".to_string(),
            message: "Inbox publish operation initiated".to_string(),
        }),
    ))
}
