use axum::extract::{Json, State};
use std::sync::Arc;

use crate::AppState;
use common::{AppError, AppJson, CreateInbox, InboxDB, InboxStatus};

/// Handler to create a new inbox
pub async fn create_inbox(
    State(state): State<Arc<AppState>>,
    Json(input): Json<CreateInbox>,
) -> Result<AppJson<InboxDB>, AppError> {
    // Validate input
    if input.warehouse_path.is_empty() {
        return Err(AppError::ValidationError(
            "warehouse_path is required".to_string(),
        ));
    }

    // Check for existing inbox with same warehouse_path
    let existing = state.store
        .find_one(mongodb::bson::doc! { "warehouse_path": &input.warehouse_path })
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

    if existing.is_some() {
        return Err(AppError::ValidationError(
            "An inbox already exists for this warehouse path".to_string(),
        ));
    }

    let now = chrono::Utc::now();
    let inbox = InboxDB {
        id: None,
        name: input.name,
        size: input.size,
        status: InboxStatus::Active,
        warehouse_path: input.warehouse_path,
        inbox_path: None,
        lifetime: input.lifetime,
        archive_enabled: input.archive_enabled,
        archive_sites: input.archive_sites,
        version: 0,
        current_job_id: None,
        progress: None,
        error: None,
        created_at: now,
        updated_at: now,
    };

    let result = state.store
        .insert_one(inbox.clone())
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

    // Return the created inbox with its new ID
    let mut created_inbox = inbox;
    let inserted_id = result.inserted_id.as_object_id();
    created_inbox.id = inserted_id;

    // Trigger Citadel create operation
    if let Some(ref citadel) = state.citadel_client {
        let warehouse_path = created_inbox.warehouse_path.clone();
        let citadel_clone = citadel.clone();
        let inbox_id_str = inserted_id.unwrap().to_hex();
        tokio::spawn(async move {
            match citadel_clone.trigger_create(&inbox_id_str, &warehouse_path).await {
                Ok(resp) => tracing::info!("Citadel create triggered: job_id={}", resp.job_id),
                Err(e) => tracing::error!("Failed to trigger Citadel create: {}", e),
            }
        });
    }

    Ok(AppJson::new(created_inbox))
}