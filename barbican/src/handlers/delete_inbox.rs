use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use mongodb::bson::{doc, oid::ObjectId};
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;
use common::{AppError, InboxStatus};

/// Response body for inbox deletion (202 Accepted)
#[derive(Debug, Serialize)]
pub struct DeleteInboxResponse {
    pub inbox_id: String,
    pub status: String,
    pub message: String,
}

/// Handler to delete an inbox by ID (soft delete)
///
/// Sets the inbox to `Deleting` and fires the Citadel delete trigger
/// asynchronously, returning `202 Accepted`.
pub async fn delete_inbox(
    State(state): State<Arc<AppState>>,
    Path(id): Path<ObjectId>,
) -> Result<(StatusCode, Json<DeleteInboxResponse>), AppError> {
    // Find the inbox first
    let inbox = state
        .store
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

    // Update status to Deleting (version-conditional optimistic lock)
    let update = doc! {
        "$set": {
            "status": "deleting",
            "updated_at": mongodb::bson::DateTime::now(),
            "version": inbox.version + 1,
        }
    };

    crate::handlers::transition::apply_versioned_update(&state.store, &id, inbox.version, &update)
        .await?;

    // Trigger Citadel delete operation (fire-and-forget). On failure, apply
    // the in-Barbican fallback (D2) so the inbox doesn't stay `Deleting`.
    if let Some(ref citadel) = state.citadel_client {
        let citadel_clone = citadel.clone();
        let store = state.store.clone();
        tokio::spawn(async move {
            match citadel_clone.trigger_delete(&id.to_hex()).await {
                Ok(resp) => tracing::info!("Citadel delete triggered: job_id={}", resp.job_id),
                Err(e) => {
                    tracing::error!("Failed to trigger Citadel delete: {}", e);
                    if let Err(ferr) = crate::handlers::transition::apply_trigger_failure(
                        &store,
                        &id,
                        "citadel_trigger_failed",
                        &format!("Citadel delete trigger failed: {}", e),
                    )
                    .await
                    {
                        tracing::warn!("Failed to apply delete-trigger failure fallback: {}", ferr);
                    }
                }
            }
        });
    }

    Ok((
        StatusCode::ACCEPTED,
        Json(DeleteInboxResponse {
            inbox_id: id.to_hex(),
            status: "deleting".to_string(),
            message: "Inbox deletion initiated".to_string(),
        }),
    ))
}
