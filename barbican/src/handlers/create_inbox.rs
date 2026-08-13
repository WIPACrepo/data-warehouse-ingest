use axum::{extract::State, http::StatusCode, Json};
use mongodb::results::InsertOneResult;
use std::sync::Arc;

use crate::{db_trait::InboxStore, CreateInbox, InboxDB};

/// Handler to create a new inbox
pub async fn create_inbox(
    State(store): State<Arc<dyn InboxStore>>,
    Json(input): Json<CreateInbox>,
) -> Result<Json<InsertOneResult>, (StatusCode, String)> {
    let inbox = InboxDB {
        id: None,
        name: input.name,
        size: input.size,
        active: true,
    };

    let result = store
        .insert_one(inbox)
        .await
        .map_err(internal_error)?;

    Ok(Json(result))
}

fn internal_error<E>(err: E) -> (StatusCode, String)
where
    E: std::error::Error,
{
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}