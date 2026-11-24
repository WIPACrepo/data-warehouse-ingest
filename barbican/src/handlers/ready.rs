//! Readiness probe endpoint

use axum::{Json, extract::State, http::StatusCode};
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;

#[derive(Serialize)]
pub struct ReadyResponse {
    pub status: String,
    pub mongodb: String,
}

/// Readiness probe — pings MongoDB through the inbox store.
///
/// - `200 {"status":"ready","mongodb":"connected"}` when the ping succeeds;
/// - `503 {"status":"not_ready","mongodb":"disconnected"}` otherwise.
///
/// Mirrors Citadel's `/ready` (`citadel/src/handlers/ready.rs`).
pub async fn ready(State(state): State<Arc<AppState>>) -> (StatusCode, Json<ReadyResponse>) {
    match state.store.ping().await {
        Ok(()) => (
            StatusCode::OK,
            Json(ReadyResponse {
                status: "ready".to_string(),
                mongodb: "connected".to_string(),
            }),
        ),
        Err(e) => {
            tracing::warn!("Readiness check failed: MongoDB ping error: {e}");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ReadyResponse {
                    status: "not_ready".to_string(),
                    mongodb: "disconnected".to_string(),
                }),
            )
        }
    }
}
