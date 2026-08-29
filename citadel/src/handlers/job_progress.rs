//! Job progress reporting handler
//!
//! `citadel-job` reports transfer progress to Citadel (the single trust edge
//! for coordination, D9) via `POST /api/v1/job/progress`. This endpoint is
//! authenticated like the job callback (X-Callback-Secret + timestamp +
//! nonce). Citadel then forwards the counts to Barbican's
//! `publish/progress` / `delete/progress` action endpoints so the inbox's
//! `progress` field stays live during long transfers.
//!
//! Progress reporting is **best-effort**: failures are logged (and, for
//! Barbican, metered by the client) but never fail the callback or the
//! transfer.

use axum::{
    extract::State,
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::AppState;
use crate::error::{AppError, Result};
use crate::jobs::types::JobType;

/// Progress report from a job executor.
#[derive(Debug, Deserialize)]
pub struct JobProgressRequest {
    pub job_id: String,
    pub timestamp: DateTime<Utc>,
    pub nonce: String,
    /// Total files discovered by the transfer (0 when unknown).
    #[serde(default)]
    pub total_files: u64,
    /// Files completely copied so far.
    #[serde(default)]
    pub processed_files: u64,
    /// Total bytes to transfer (0 when unknown).
    #[serde(default)]
    pub total_bytes: u64,
    /// Bytes copied so far.
    #[serde(default)]
    pub transferred_bytes: u64,
    /// Phase string reported by the worker (e.g. "transferring").
    #[serde(default)]
    pub phase: String,
}

#[derive(Debug, Serialize)]
pub struct JobProgressResponse {
    pub acknowledged: bool,
}

/// Handle a progress report, forwarding to Barbican for the matching job type.
pub async fn job_progress(
    State(app_state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(request): Json<JobProgressRequest>,
) -> Result<(StatusCode, Json<JobProgressResponse>)> {
    // Authenticate like the job callback: X-Callback-Secret + timestamp +
    // nonce (TTL + replay protection). Non-200s surface as JSON error bodies.
    crate::handlers::job_callback::validate_callback_credentials(
        &app_state,
        &headers,
        request.timestamp,
        &request.nonce,
    )
    .await?;

    let jobs_coll = crate::db::jobs_collection(&app_state.db_client, &app_state.config);
    let job = jobs_coll
        .find_one(mongodb::bson::doc! { "job_id": &request.job_id })
        .await
        .map_err(|e| AppError::DatabaseError(format!("Failed to find job: {}", e)))?
        .ok_or_else(|| AppError::JobNotFound(request.job_id.clone()))?;

    let forward_result = match &job.job_type {
        JobType::PublishInbox { inbox_id, .. } => {
            forward_publish(&app_state, inbox_id, &request).await
        }
        JobType::DeleteInbox { inbox_id } => {
            forward_delete(&app_state, inbox_id, &request).await
        }
    };

    match forward_result {
        Ok(_) => {
            info!(
                "Forwarded progress for job {} (files {}/{}, bytes {}/{})",
                request.job_id,
                request.processed_files,
                request.total_files,
                request.transferred_bytes,
                request.total_bytes
            );
        }
        Err(e) => {
            // Best-effort: never fail the progress report (and thus the
            // transfer) because Barbican is unreachable.
            tracing::warn!(
                "Failed to forward progress for job {} to Barbican: {}",
                request.job_id,
                e
            );
        }
    }

    Ok((
        StatusCode::OK,
        Json(JobProgressResponse { acknowledged: true }),
    ))
}

/// Forward publish-style progress to Barbican's `publish/progress`.
async fn forward_publish(
    app_state: &AppState,
    inbox_id: &ObjectId,
    request: &JobProgressRequest,
) -> Result<()> {
    app_state
        .barbican_client
        .publish_progress(
            inbox_id,
            &request.job_id,
            phase_or(&request.phase, "in_progress"),
            request.total_files,
            request.processed_files,
            request.total_bytes,
            request.transferred_bytes,
        )
        .await
        .map(|_| ())
}

/// Forward delete-style progress to Barbican's `delete/progress`.
async fn forward_delete(
    app_state: &AppState,
    inbox_id: &ObjectId,
    request: &JobProgressRequest,
) -> Result<()> {
    app_state
        .barbican_client
        .delete_progress(
            inbox_id,
            &request.job_id,
            phase_or(&request.phase, "in_progress"),
            request.total_bytes,
            request.transferred_bytes,
        )
        .await
        .map(|_| ())
}

fn phase_or<'a>(phase: &'a str, default: &'a str) -> &'a str {
    if phase.is_empty() {
        default
    } else {
        phase
    }
}
