use axum::{extract::{State, Json}};
use std::sync::Arc;

use common::{InboxStore, CreateInbox, InboxDB, InboxStatus, AppError, AppJson};

/// Handler to create a new inbox
pub async fn create_inbox(
    State(store): State<Arc<dyn InboxStore>>,
    Json(input): Json<CreateInbox>,
) -> Result<AppJson<InboxDB>, AppError> {
    // Validate input
    if input.warehouse_path.is_empty() {
        return Err(AppError::ValidationError(
            "warehouse_path is required".to_string()
        ));
    }
    
    // Check for existing inbox with same warehouse_path
    let existing = store
        .find_one(mongodb::bson::doc! { "warehouse_path": &input.warehouse_path })
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;
    
    if existing.is_some() {
        return Err(AppError::ValidationError(
            "An inbox already exists for this warehouse path".to_string()
        ));
    }
    
    let now = chrono::Utc::now();
    let inbox = InboxDB {
        id: None,
        name: input.name,
        size: input.size,
        status: InboxStatus::Active,
        warehouse_path: input.warehouse_path,
        lifetime: input.lifetime,
        archive_enabled: input.archive_enabled,
        archive_sites: input.archive_sites,
        created_at: now,
        updated_at: now,
    };

    let result = store
        .insert_one(inbox.clone())
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

    // Return the created inbox with its new ID
    let mut created_inbox = inbox;
    created_inbox.id = result.inserted_id.as_object_id();
    
    Ok(AppJson::new(created_inbox))
}