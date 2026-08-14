use axum::{extract::{Path, State}, http::StatusCode, Json};
use mongodb::{bson::oid::ObjectId, bson::doc};
use std::sync::Arc;

use common::{InboxStore, InboxDB};

/// Handler to get an inbox by ID
pub async fn get_inbox(
    State(store): State<Arc<dyn InboxStore>>,
    Path(id): Path<ObjectId>,
) -> Result<Json<InboxDB>, (StatusCode, String)> {
    let result = store
        .find_one(doc! { "_id": id })
        .await
        .map_err(internal_error)?;

    match result {
        Some(inbox) => Ok(Json(inbox)),
        None => Err((StatusCode::NOT_FOUND, "Invalid inbox id".to_string())),
    }
}

fn internal_error<E>(err: E) -> (StatusCode, String)
where
    E: std::error::Error,
{
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}