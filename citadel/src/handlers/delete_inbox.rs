//! Delete inbox handler - starts inbox deletion job
//!
//! Submits a `DeleteInbox` job to the executor after enforcing the delete
//! queue rate limit (D8), mirroring `publish_inbox.rs`: the in-flight delete
//! job count is checked against MongoDB *before* the Barbican `delete_start`
//! transition or executor submission; a saturated queue rolls back the
//! just-inserted `JobDB` row and rejects with `429 TooManyRequests`.

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    Json,
};
use mongodb::bson::{doc, oid::ObjectId};
use serde::Serialize;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::AppState;
use common::Claims;
use crate::db::inboxes_collection;

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

    let object_id = ObjectId::parse_str(&inbox_id)
        .map_err(|_| AppError::InboxNotFound(inbox_id.clone()))?;

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

    let mut job = JobDB::new(
        job_id.clone(),
        object_id,
        job_type.clone(),
        &executor_type,
    );

    // Insert job into MongoDB
    let jobs_coll = crate::db::jobs_collection(&app_state.db_client, &app_state.config);
    jobs_coll
        .insert_one(&job)
        .await
        .map_err(|e| {
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
            in_flight,
            app_state.config.queue_delete_rate_limit,
            job_id
        );
        return Err(AppError::TooManyRequests(format!(
            "Delete queue is saturated ({} in-flight jobs)",
            in_flight
        )));
    }

    // Transition inbox state via Barbican delete_start action
    app_state.barbican_client
        .delete_start(&object_id, &job_id)
        .await
        .map_err(|e| {
            error!("Failed to signal delete start to Barbican: {}", e);
            e
        })?;

    // Submit job to executor (Kubernetes or Mock)
    let executor_id = app_state.executor.submit(&job).await?;

    // Update job with executor info
    job.executor.executor_id = Some(executor_id);
    job.status = JobStatus::Running;

    jobs_coll
        .replace_one(
            mongodb::bson::doc! { "job_id": &job_id },
            &job,
        )
        .await
        .map_err(|e| {
            error!("Failed to update job in MongoDB: {}", e);
            AppError::DatabaseError(format!("Failed to update job: {}", e))
        })?;

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
