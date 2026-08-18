//! Create inbox handler

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    Json,
};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};
use tracing::{error, info};

use crate::auth::AuthState;
use crate::error::{AppError, Result};
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct CreateInboxParams {
    pub warehouse_path: String,
}

#[derive(Debug, Serialize)]
pub struct CreateInboxResponse {
    pub inbox_id: String,
    pub inbox_path: String,
    pub status: String,
}

pub async fn create_inbox(
    State(state): State<AppState>,
    Extension(auth_state): Extension<AuthState>,
    Path(inbox_id): Path<String>,
    Json(_params): Json<CreateInboxParams>,
) -> Result<(StatusCode, Json<CreateInboxResponse>)> {
    // Check authorization
    if !auth_state.claims.can_write() {
        return Err(AppError::Forbidden(
            "Insufficient permissions to create inbox".to_string(),
        ));
    }

    let object_id = ObjectId::parse_str(&inbox_id)
        .map_err(|_| AppError::InboxNotFound(inbox_id.clone()))?;

    // Get inbox metadata from Barbican
    let inbox = state.barbican_client
        .get_inbox(&object_id)
        .await
        .map_err(|e| {
            error!("Failed to get inbox from Barbican: {}", e);
            e
        })?;

    // Validate inbox state - should be Active with no inbox_path
    if inbox.inbox_path.is_some() {
        return Err(AppError::InvalidStateTransition(
            "Inbox already has a path assigned".to_string(),
        ));
    }

    // Create a job_id for tracking this create operation
    let job_id = uuid::Uuid::new_v4().to_string();

    // Signal start of creation to Barbican
    state.barbican_client
        .create_start(&object_id, &job_id)
        .await
        .map_err(|e| {
            error!("Failed to signal create start to Barbican: {}", e);
            e
        })?;

    // Create inbox directory
    let inbox_path = state.storage_manager.create_inbox(&object_id).await?;

    // Signal completion to Barbican
    state.barbican_client
        .create_complete(&object_id, &job_id, &inbox_path.to_string_lossy())
        .await
        .map_err(|e| {
            error!("Failed to signal create complete to Barbican: {}", e);
            e
        })?;

    info!(
        "Created inbox {} at path {:?}",
        inbox_id, inbox_path
    );

    Ok((
        StatusCode::CREATED,
        Json(CreateInboxResponse {
            inbox_id,
            inbox_path: inbox_path.to_string_lossy().to_string(),
            status: "ready".to_string(),
        }),
    ))
}