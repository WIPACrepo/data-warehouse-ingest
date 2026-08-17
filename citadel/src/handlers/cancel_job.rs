//! Cancel job handler

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    Json,
};
use serde::Serialize;

use crate::AppState;
use crate::auth::AuthState;

use crate::db::jobs_collection;
use crate::error::{CitadelError, Result};
use crate::jobs::types::JobStatus;

#[derive(Debug, Serialize)]
pub struct CancelJobResponse {
    pub job_id: String,
    pub status: String,
}

pub async fn cancel_job(
    State(app_state): State<AppState>,
    Extension(auth_state): Extension<AuthState>,
    Path(job_id): Path<String>,
) -> Result<(StatusCode, Json<CancelJobResponse>)> {
    // Check authorization - admin role required for cancellation
    if !auth_state.claims.is_admin() {
        return Err(CitadelError::Forbidden(
            "Insufficient permissions to cancel jobs".to_string(),
        ));
    }

    let jobs_coll = jobs_collection(&app_state.db_client, &app_state.config);
    
    // Find the job
    let mut job = jobs_coll
        .find_one(mongodb::bson::doc! { "job_id": &job_id })
        .await
        .map_err(|e| {
            CitadelError::DatabaseError(format!("Failed to query job: {}", e))
        })?
        .ok_or_else(|| CitadelError::JobNotFound(job_id.clone()))?;

    // Check if job can be cancelled
    match job.status {
        JobStatus::Completed | JobStatus::Cancelled | JobStatus::Failed | JobStatus::TimedOut => {
            return Err(CitadelError::JobCompleted(
                format!("Job {} cannot be cancelled in status {:?}", job_id, job.status)
            ));
        }
        JobStatus::Cancelling => {
            return Err(CitadelError::JobCompleted(
                format!("Job {} is already being cancelled", job_id)
            ));
        }
        _ => {}
    }

    // Update job status to cancelling
    job.status = JobStatus::Cancelling;
    jobs_coll
        .replace_one(
            mongodb::bson::doc! { "job_id": &job_id },
            &job,
        )
        .await
        .map_err(|e| {
            CitadelError::DatabaseError(format!("Failed to update job status: {}", e))
        })?;

    // Cancel the job in the executor
    app_state.executor.cancel(&job_id).await?;

    // Update final status
    job.status = JobStatus::Cancelled;
    jobs_coll
        .replace_one(
            mongodb::bson::doc! { "job_id": &job_id },
            &job,
        )
        .await
        .map_err(|e| {
            CitadelError::DatabaseError(format!("Failed to update job status: {}", e))
        })?;

    Ok((
        StatusCode::OK,
        Json(CancelJobResponse {
            job_id,
            status: "cancelled".to_string(),
        }),
    ))
}