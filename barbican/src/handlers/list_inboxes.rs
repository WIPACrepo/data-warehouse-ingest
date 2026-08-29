use axum::extract::{Query, State};
use mongodb::bson::doc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use common::{AppError, AppJson, InboxDB};

/// Query parameters for listing inboxes
#[derive(Debug, Deserialize)]
pub struct ListInboxesQuery {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: u64,
    #[serde(default)]
    pub offset: u64,
}

fn default_limit() -> u64 {
    100
}

/// Paginated inbox list response envelope
#[derive(Debug, Serialize)]
pub struct ListInboxesResponse {
    pub inboxes: Vec<InboxDB>,
    pub total: u64,
    pub limit: u64,
    pub offset: u64,
    pub has_more: bool,
}

/// Handler to list all inboxes with optional filtering and pagination
pub async fn list_inboxes(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ListInboxesQuery>,
) -> Result<AppJson<ListInboxesResponse>, AppError> {
    let mut filter = doc! {};

    // Filter by status if provided
    if let Some(status) = params.status {
        filter.insert("status", status);
    }

    let limit = params.limit.min(1000);

    let mut find_options = mongodb::options::FindOptions::default();
    find_options.limit = Some(limit as i64);
    find_options.skip = Some(params.offset);

    let results = state
        .store
        .find_many(filter.clone(), find_options)
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

    let total = state
        .store
        .count(filter)
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

    let has_more = params.offset + (results.len() as u64) < total;

    Ok(AppJson::new(ListInboxesResponse {
        inboxes: results,
        total,
        limit,
        offset: params.offset,
        has_more,
    }))
}
