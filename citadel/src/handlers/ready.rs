//! Readiness probe endpoint

use axum::{Json, extract::State, http::StatusCode};
use serde::Serialize;

use crate::AppState;
use crate::error::Result;

#[derive(Serialize)]
pub struct ReadyResponse {
    pub status: String,
    pub mongodb: String,
}

pub async fn ready_with_client(
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<ReadyResponse>)> {
    match state
        .db_client
        .database("admin")
        .run_command(mongodb::bson::doc! { "ping": 1 })
        .await
    {
        Ok(_) => Ok((
            StatusCode::OK,
            Json(ReadyResponse {
                status: "ready".to_string(),
                mongodb: "connected".to_string(),
            }),
        )),
        Err(_) => Ok((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ReadyResponse {
                status: "not_ready".to_string(),
                mongodb: "disconnected".to_string(),
            }),
        )),
    }
}
