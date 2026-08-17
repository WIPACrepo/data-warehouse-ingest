//! Job callback handler - receives completion notifications from job executors

use axum::{
    extract::State,
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use mongodb::bson::doc;
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use crate::AppState;
use crate::db::{callback_nonces_collection, jobs_collection};
use crate::error::{CitadelError, Result};
use crate::models::{CallbackNonceDB, JobStatus, TransferResult};
use crate::jobs::types::JobType;

#[derive(Debug, Deserialize)]
pub struct JobCallbackRequest {
    pub job_id: String,
    pub status: String,
    pub timestamp: DateTime<Utc>,
    pub nonce: String,
    pub result: Option<CallbackResult>,
}

#[derive(Debug, Deserialize)]
pub struct CallbackResult {
    pub success: bool,
    pub files_transferred: usize,
    pub total_bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct JobCallbackResponse {
    pub acknowledged: bool,
}

/// Validate callback timestamp (must be within 5 minutes)
fn validate_timestamp(timestamp: DateTime<Utc>) -> bool {
    let now = Utc::now();
    let age = now.signed_duration_since(timestamp);
    age.num_seconds() < 300 && age.num_seconds() > -60 // Allow 1 minute clock skew
}

/// Check and store nonce to prevent replay attacks
async fn validate_and_store_nonce(
    app_state: &AppState,
    nonce: &str,
) -> Result<()> {
    let nonces_coll = callback_nonces_collection(&app_state.db_client, &app_state.config);
    
    // Check if nonce already exists
    let existing = nonces_coll
        .find_one(doc! { "nonce": nonce })
        .await
        .map_err(|e| CitadelError::DatabaseError(format!("Failed to check nonce: {}", e)))?;
    
    if existing.is_some() {
        return Err(CitadelError::CallbackNonceReused);
    }
    
    // Store new nonce with TTL
    let nonce_doc = CallbackNonceDB::new(nonce.to_string());
    nonces_coll
        .insert_one(nonce_doc)
        .await
        .map_err(|e| CitadelError::DatabaseError(format!("Failed to store nonce: {}", e)))?;
    
    Ok(())
}

/// Authenticate callback request
async fn authenticate_callback(
    app_state: &AppState,
    request: &JobCallbackRequest,
) -> Result<()> {
    // Check timestamp
    if !validate_timestamp(request.timestamp) {
        return Err(CitadelError::CallbackTimestampExpired);
    }
    
    // Check nonce
    validate_and_store_nonce(app_state, &request.nonce).await?;
    
  
    // TODO: Validate callback secret from header if needed
    // For now, we rely on timestamp and nonce validation
    
    Ok(())
}

pub async fn job_callback(
    State(app_state): State<AppState>,
    Json(request): Json<JobCallbackRequest>,
) -> Result<(StatusCode, Json<JobCallbackResponse>)> {
    // Authenticate the callback
    authenticate_callback(&app_state, &request).await?;
    
    let jobs_coll = jobs_collection(&app_state.db_client, &app_state.config);
    
    // Find the job
    let mut job = jobs_coll
        .find_one(doc! { "job_id": &request.job_id })
        .await
        .map_err(|e| CitadelError::DatabaseError(format!("Failed to find job: {}", e)))?
        .ok_or_else(|| CitadelError::JobNotFound(request.job_id.clone()))?;
    
    // Check if job is already completed
    if matches!(job.status, JobStatus::Completed | JobStatus::Cancelled | JobStatus::Failed | JobStatus::TimedOut) {
        warn!("Callback received for already completed job {}", request.job_id);
        return return Ok((
            StatusCode::OK,
            Json(JobCallbackResponse { acknowledged: true }),
        ));
    }
    
    // Update job status based on callback
    match request.status.as_str() {
        "completed" => {
            if let Some(result) = request.result {
                if result.success {
                    // Handle successful completion
                    job.status = JobStatus::Completed;
                    job.result = Some(crate::models::JobResult {
                        success: true,
                        files_transferred: result.files_transferred,
                        total_bytes: result.total_bytes,
                        completed_at: Utc::now(),
                    });
                    
                    // Notify catalog and LTA if this was a publish job
                    if let JobType::PublishInbox { notify_catalog, notify_lta, inbox_id } = &job.job_type {
                        // Get inbox details for for notifications
                        // TODO: Get inbox from Barbican or MongoDB
                        
                        if *notify_catalog {
                            // TODO: Create TransferResult from job result
                            let transfer_result = TransferResult {
                                success: true,
                                files_transferred: vec![],
                                files_failed: vec![],
                                total_bytes: result.total_bytes,
                                partial_cleanup_completed: true,
                            };
                            // app_state.catalog_notifier.notify(&inbox, &transfer_result).await?;
                        }
                        
                        if *notify_lta && *notify_catalog {
                            // Only notify LTA after catalog notification succeeds
                            // TODO: Same()
                        }
                    }
                } else {
                    job.status = JobStatus::Failed;
                }
            } else {
                job.status = JobStatus::Failed;
            }
        }
        "failed" => {
            job.status = JobStatus::Failed;
            if let Some(result) = &request.result {
                if !result.success {
                    job.errors.push(crate::models::JobError {
                        code: "job_execution_failed".to_string(),
                        message: format!("Job failed with {} bytes transferred", result.total_bytes),
                        timestamp: request.timestamp,
                    });
                }
            }
        }
        "timed_out" => {
            job.status = JobStatus::TimedOut;
        }
        _ => {
            return Err(CitadelError::InvalidJwt(format!("Unknown callback status: {}", request.status)));
        }
    }
    
    job.updated_at = Utc::now();
    
    // Update job in database
    jobs_coll
        .replace_one(
            doc! { "job_id": &request.job_id },
            &job,
        )
        .await
        .map_err(|e| CitadelError::DatabaseError(format!("Failed to update job: {}", e)))?;
    
    info!("Processed callback for job {}: {}", request.job_id, request.status);
    
    Ok((
        StatusCode::OK,
        Json(JobCallbackResponse { acknowledged: true }),
    ))
}