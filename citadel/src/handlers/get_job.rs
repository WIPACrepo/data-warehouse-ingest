//! Get job status handler

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    Json,
};
use serde::Serialize;

use crate::AppState;
use crate::auth::AuthState;

use crate::db::jobs_collection;
use crate::error::{AppError, Result};
use crate::jobs::types::JobType;

#[derive(Debug, Serialize)]
pub struct GetJobResponse {
    pub job_id: String,
    pub inbox_id: String,
    pub job_type: String,
    pub status: String,
    pub progress: Option<crate::jobs::types::JobProgress>,
    pub errors: Vec<crate::models::JobError>,
    pub result: Option<crate::models::JobResult>,
}

pub async fn get_job(
    State(app_state): State<AppState>,
    Extension(auth_state): Extension<AuthState>,
    Path(job_id): Path<String>,
) -> Result<(StatusCode, Json<GetJobResponse>)> {
    // Check authorization - read role is sufficient
    if !auth_state.claims.can_read() {
        return Err(AppError::Forbidden(
            "Insufficient permissions to view job status".to_string(),
        ));
    }

    let jobs_coll = jobs_collection(&app_state.db_client, &app_state.config);
    
    let job = jobs_coll
        .find_one(mongodb::bson::doc! { "job_id": &job_id })
        .await
        .map_err(|e| {
            AppError::DatabaseError(format!("Failed to query job: {}", e))
        })?
        .ok_or_else(|| AppError::JobNotFound(job_id.clone()))?;

    let job_type_str = match &job.job_type {
        JobType::PublishInbox { .. } => "publish",
        JobType::DeleteInbox { .. } => "delete",
    };

    Ok((
        StatusCode::OK,
        Json(GetJobResponse {
            job_id: job.job_id,
            inbox_id: job.inbox_id.to_string(),
            job_type: job_type_str.to_string(),
            status: format!("{:?}", job.status).to_lowercase(),
            progress: job.progress,
            errors: job.errors,
            result: job.result,
        }),
    ))
}