//! Cancel job handler

use axum::{
    Json,
    extract::{Extension, Path, State},
    http::StatusCode,
};
use serde::Serialize;

use crate::AppState;
use common::Claims;

use crate::db::jobs_collection;
use crate::error::{AppError, Result};
use crate::jobs::types::JobStatus;

#[derive(Debug, Serialize)]
pub struct CancelJobResponse {
    pub job_id: String,
    pub status: String,
}

pub async fn cancel_job(
    State(app_state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(job_id): Path<String>,
) -> Result<(StatusCode, Json<CancelJobResponse>)> {
    // Check authorization - admin role required for cancellation
    if !claims.is_admin("citadel") {
        return Err(AppError::Forbidden(
            "Insufficient permissions to cancel jobs".to_string(),
        ));
    }

    let jobs_coll = jobs_collection(&app_state.db_client, &app_state.config);

    // Find the job
    let job = jobs_coll
        .find_one(mongodb::bson::doc! { "job_id": &job_id })
        .await
        .map_err(|e| AppError::DatabaseError(format!("Failed to query job: {}", e)))?
        .ok_or_else(|| AppError::JobNotFound(job_id.clone()))?;

    // Check if job can be cancelled
    match job.status {
        JobStatus::Completed | JobStatus::Cancelled | JobStatus::Failed | JobStatus::TimedOut => {
            return Err(AppError::Conflict(format!(
                "Job {} cannot be cancelled in status {:?}",
                job_id, job.status
            )));
        }
        JobStatus::Cancelling => {
            return Err(AppError::Conflict(format!(
                "Job {} is already being cancelled",
                job_id
            )));
        }
        _ => {}
    }

    // Note: we use version-conditional `update_one` (not full-doc replace_one)
    // so concurrent writers cannot clobber each other's changes silently. The
    // `version` read when the job was fetched is part of the filter and the
    // update bumps it (P2.3). A 0-matched update means the job changed
    // concurrently.
    let read_version = job.version;
    let job_id_for_update = job_id.clone();
    let now = chrono::Utc::now().to_rfc3339();

    // Update job status to cancelling
    let cancelling = jobs_coll
        .update_one(
            mongodb::bson::doc! { "job_id": &job_id_for_update, "version": read_version },
            mongodb::bson::doc! {
                "$set": {
                    "status": "cancelling",
                    "updated_at": &now,
                    "version": read_version + 1,
                }
            },
        )
        .await
        .map_err(|e| AppError::DatabaseError(format!("Failed to update job status: {}", e)))?;

    if cancelling.matched_count == 0 {
        return Err(AppError::Conflict(format!(
            "Job {} was modified concurrently while being cancelled",
            job_id_for_update
        )));
    }

    // Cancel the job in the executor
    app_state.executor.cancel(&job_id_for_update).await?;

    // Update final status
    let cancelled = jobs_coll
        .update_one(
            mongodb::bson::doc! { "job_id": &job_id_for_update, "version": read_version + 1 },
            mongodb::bson::doc! {
                "$set": {
                    "status": "cancelled",
                    "updated_at": &now,
                    "version": read_version + 2,
                }
            },
        )
        .await
        .map_err(|e| AppError::DatabaseError(format!("Failed to update job status: {}", e)))?;

    if cancelled.matched_count == 0 {
        return Err(AppError::Conflict(format!(
            "Job {} was modified concurrently while being cancelled",
            job_id_for_update
        )));
    }

    Ok((
        StatusCode::OK,
        Json(CancelJobResponse {
            job_id: job_id_for_update,
            status: "cancelled".to_string(),
        }),
    ))
}
