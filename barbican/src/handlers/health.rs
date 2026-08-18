use axum::Json;
use serde::Serialize;

#[derive(Serialize)]
pub struct HealthResponse {
    status: String,
}

/// Health check endpoint - returns OK if the service is running
pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "healthy".to_string(),
    })
}