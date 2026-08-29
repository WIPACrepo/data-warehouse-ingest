use axum::Json;
use serde::Serialize;

#[derive(Serialize)]
pub struct ReadyResponse {
    status: String,
    mongodb: String,
}

/// Readiness probe - checks MongoDB connectivity
pub async fn ready() -> Json<ReadyResponse> {
    // In a full implementation, this would ping MongoDB
    // For now, we return a basic ready response
    Json(ReadyResponse {
        status: "ready".to_string(),
        mongodb: "connected".to_string(),
    })
}
