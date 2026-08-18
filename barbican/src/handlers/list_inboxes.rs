use axum::extract::{Query, State};
use mongodb::bson::doc;
use std::sync::Arc;

use crate::AppState;
use common::{AppError, AppJson, InboxDB};
use serde::Deserialize;

/// Query parameters for listing inboxes
#[derive(Debug, Deserialize)]
pub struct ListInboxesQuery {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub skip: Option<u64>,
}

/// Handler to list all inboxes with optional filtering
pub async fn list_inboxes(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ListInboxesQuery>,
) -> Result<AppJson<Vec<InboxDB>>, AppError> {
    let mut filter = doc! {};
    
    // Filter by status if provided
    if let Some(status) = params.status {
        filter.insert("status", status);
    }

    let mut find_options = mongodb::options::FindOptions::default();
    
    // Apply limit if provided
    if let Some(limit) = params.limit {
        find_options.limit = Some(limit as i64);
    }
    
    // Apply skip if provided
    if let Some(skip) = params.skip {
        find_options.skip = Some(skip);
    }

    let results = state.store
        .find_many(filter, find_options)
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

    Ok(AppJson::new(results))
}