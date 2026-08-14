use axum::{extract::{Path, State}};
use mongodb::bson::{doc, oid::ObjectId};
use std::sync::Arc;

use common::{InboxStore, InboxDB, AppError, AppJson};

/// Handler to get an inbox by ID
pub async fn get_inbox(
    State(store): State<Arc<dyn InboxStore>>,
    Path(id): Path<ObjectId>,
) -> Result<AppJson<InboxDB>, AppError> {
    let result = store
        .find_one(doc! { "_id": id })
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

    match result {
        Some(inbox) => Ok(AppJson::new(inbox)),
        None => Err(AppError::InboxNotFound("Invalid inbox id".to_string())),
    }
}