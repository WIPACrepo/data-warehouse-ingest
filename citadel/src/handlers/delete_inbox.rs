//! Delete inbox handler - starts inbox deletion job
//!
//! Submits a `DeleteInbox` job to the executor after enforcing the delete
//! queue rate limit (D8), mirroring `publish_inbox.rs`: the in-flight delete
//! job count is checked against MongoDB *before* the Barbican `delete_start`
//! transition or executor submission; a saturated queue rolls back the
//! just-inserted `JobDB` row and rejects with `429 TooManyRequests`.

use axum::{
    Json,
    extract::{Extension, Path, State},
    http::StatusCode,
};
use mongodb::bson::{doc, oid::ObjectId};
use serde::Serialize;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::AppState;
use crate::db::inboxes_collection;
use crate::handlers::compensate::{Operation, codes, compensate};
use common::Claims;

use crate::error::{AppError, Result};
use crate::jobs::types::{JobStatus, JobType};
use crate::models::JobDB;

use super::publish_inbox::{count_in_flight_jobs, quota_exceeded, rollback_job};

#[derive(Debug, Serialize)]
pub struct DeleteInboxResponse {
    pub job_id: String,
    pub inbox_id: String,
    pub status: String,
    pub job_type: String,
}

pub async fn delete_inbox(
    State(app_state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(inbox_id): Path<String>,
) -> Result<(StatusCode, Json<DeleteInboxResponse>)> {
    // Check authorization
    if !claims.can_write("citadel") {
        return Err(AppError::Forbidden(
            "Insufficient permissions to delete inbox".to_string(),
        ));
    }

    let object_id =
        ObjectId::parse_str(&inbox_id).map_err(|_| AppError::InboxNotFound(inbox_id.clone()))?;

    // Get inbox metadata directly from MongoDB (shared collection, read-only for Citadel)
    let inboxes_coll = inboxes_collection(&app_state.db_client, &app_state.config);
    let inbox = inboxes_coll
        .find_one(doc! { "_id": object_id })
        .await
        .map_err(|e| {
            error!("Failed to get inbox from MongoDB: {}", e);
            AppError::DatabaseError(format!("Failed to query inbox: {}", e))
        })?
        .ok_or_else(|| AppError::InboxNotFound(inbox_id.clone()))?;

    // Validate inbox must exist and have a path to be deleted
    if inbox.inbox_path.is_none() {
        return Err(AppError::InvalidStateTransition(
            "Inbox must be created before deletion".to_string(),
        ));
    }

    // Create job record
    let job_id = Uuid::new_v4().to_string();
    let job_type = JobType::DeleteInbox {
        inbox_id: object_id,
    };

    let executor_type = app_state.config.job_executor.to_string();

    let mut job = JobDB::new(job_id.clone(), object_id, job_type.clone(), &executor_type);

    // Insert job into MongoDB
    let jobs_coll = crate::db::jobs_collection(&app_state.db_client, &app_state.config);
    jobs_coll.insert_one(&job).await.map_err(|e| {
        error!("Failed to insert job into MongoDB: {}", e);
        AppError::DatabaseError(format!("Failed to create job: {}", e))
    })?;

    // Enforce the delete queue rate limit BEFORE transitioning the inbox
    // state or submitting to the executor (D8).
    let in_flight = count_in_flight_jobs(&app_state, "delete_inbox").await?;
    if quota_exceeded(in_flight, app_state.config.queue_delete_rate_limit) {
        rollback_job(&app_state, &job_id).await;
        warn!(
            "Delete queue saturated ({} in-flight >= limit {}); rejecting job {}",
            in_flight, app_state.config.queue_delete_rate_limit, job_id
        );
        return Err(AppError::TooManyRequests(format!(
            "Delete queue is saturated ({} in-flight jobs)",
            in_flight
        )));
    }

    // Transition inbox state via Barbican delete_start action
    let start_result = app_state
        .barbican_client
        .delete_start(&object_id, &job_id)
        .await;
    // If Barbican refused or did not answer, the inbox never left its state,
    // so there is nothing to compensate there — but the just-inserted job row
    // is `queued` and would count toward the in-flight quota forever. Remove
    // it, exactly as the quota-rejection path does.
    if let Err(e) = start_result {
        error!("Failed to signal delete start to Barbican: {}", e);
        rollback_job(&app_state, &job_id).await;
        return Err(e);
    }

    // Submit job to executor (Kubernetes or Mock). The job is not running
    // if this fails, so it is safe to compensate: inbox -> failed via
    // Barbican, job -> failed (leaves the in-flight quota).
    let executor_id = match app_state.executor.submit(&job).await {
        Ok(id) => id,
        Err(e) => {
            compensate(
                &app_state,
                Operation::Delete,
                &object_id,
                &job_id,
                codes::EXECUTOR_ERROR,
                &e,
            )
            .await;
            return Err(e);
        }
    };

    // Update job with executor info
    job.executor.executor_id = Some(executor_id);
    job.status = JobStatus::Running;

    // The job is already running, so a failure here must NOT compensate
    // (that would mark a live transfer failed). Log it and still report the
    // job as accepted; its callback updates the job by `job_id` and the
    // status poller reconciles the rest.
    if let Err(e) = jobs_coll
        .replace_one(mongodb::bson::doc! { "job_id": &job_id }, &job)
        .await
    {
        error!(
            "Job {} was submitted but recording its executor info failed: {}",
            job_id, e
        );
    }

    info!("Created delete job {} for inbox {}", job_id, object_id);

    Ok((
        StatusCode::ACCEPTED,
        Json(DeleteInboxResponse {
            job_id,
            inbox_id: object_id.to_string(),
            status: "running".to_string(),
            job_type: "delete".to_string(),
        }),
    ))
}
