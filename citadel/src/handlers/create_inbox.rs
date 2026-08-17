//! Create inbox handler

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    Json,
};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{error, info};

use crate::auth::AuthState;
use crate::error::{CitadelError, Result};
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
    Json(params): Json<CreateInboxParams>,
) -> Result<(StatusCode, Json<CreateInboxResponse>)> {
    // Check authorization
    if !auth_state.claims.can_write() {
        return Err(CitadelError::Forbidden(
            "Insufficient permissions to create inbox".to_string(),
        ));
    }

    let object_id = ObjectId::parse_str(&inbox_id)
        .map_err(|_| CitadelError::InboxNotFound(inbox_id.clone()))?;

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
        return Err(CitadelError::InvalidInboxState(
            "Inbox already has a path assigned".to_string(),
        ));
    }

    // Create inbox directory
    let inbox_path = state.storage_manager.create_inbox(&object_id).await?;

    // Update inbox state via Barbican API
    let _updated_inbox = state.barbican_client
        .transition_inbox_state(&object_id, "active")
        .await
        .map_err(|e| {
            error!("Failed to update inbox state in Barbican: {}", e);
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
            status: "active".to_string(),
        }),
    ))
}