//! Publish inbox handler - starts file transfer job
//!
//! Submits a `PublishInbox` job to the executor after enforcing the publish
//! queue rate limit (D8): the number of in-flight publish jobs (queued,
//! running, cancelling) is counted against MongoDB *before* the Barbican
//! `publish_start` transition or executor submission. If the quota is met or
//! exceeded, the just-inserted `JobDB` row is rolled back and the request is
//! rejected with `429 TooManyRequests`.

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

/// Pure quota check: a submission is rejected when the in-flight count is
/// at/above the configured limit.
pub fn quota_exceeded(in_flight: u64, limit: usize) -> bool {
    in_flight >= limit as u64
}

/// Count of in-flight (`queued`/`running`/`cancelling`) jobs of a type.
///
/// The `JobType` enum is serialized as a tagged object (`{"type": ...}`), so
/// the canonical Mongo filter for a publish job is `{"job_type.type":
/// "publish_inbox"}`.
pub async fn count_in_flight_jobs(
    app_state: &AppState,
    job_type_str: &str,
) -> Result<u64> {
    let jobs_coll = crate::db::jobs_collection(&app_state.db_client, &app_state.config);
    let filter = doc! {
        "job_type.type": job_type_str,
        "status": {
            "$in": [
                "queued",
                "running",
                "cancelling",
            ],
        },
    };
    jobs_coll
        .count_documents(filter)
        .await
        .map_err(|e| AppError::DatabaseError(format!("Failed to count in-flight jobs: {}", e)))
}

/// Roll back a just-inserted JobDB row (quota rejection).
pub async fn rollback_job(app_state: &AppState, job_id: &str) {
    let jobs_coll = crate::db::jobs_collection(&app_state.db_client, &app_state.config);
    if let Err(e) = jobs_coll
        .delete_one(doc! { "job_id": job_id })
        .await
    {
        error!("Failed to roll back rejected job {}: {}", job_id, e);
    }
}

#[derive(Debug, Serialize)]
pub struct PublishInboxResponse {
    pub job_id: String,
    pub inbox_id: String,
    pub status: String,
    pub job_type: String,
}

pub async fn publish_inbox(
    State(app_state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(inbox_id): Path<String>,
) -> Result<(StatusCode, Json<PublishInboxResponse>)> {
    // Check authorization
    if !claims.can_write("citadel") {
        return Err(AppError::Forbidden(
            "Insufficient permissions to publish inbox".to_string(),
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

    // Validate inbox state - must have inbox_path to publish
    if inbox.inbox_path.is_none() {
        return Err(AppError::InvalidStateTransition(
            "Inbox must be created before publishing".to_string(),
        ));
    }

    // Create job record
    let job_id = Uuid::new_v4().to_string();
    let job_type = JobType::PublishInbox {
        inbox_id: object_id,
        notify_catalog: true,
        notify_lta: true,
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

    // Enforce the publish queue rate limit BEFORE transitioning the inbox
    // state or submitting to the executor (D8). A saturated queue rolls back
    // the just-inserted row and rejects with 429.
    let in_flight = count_in_flight_jobs(&app_state, "publish_inbox").await?;
    if quota_exceeded(in_flight, app_state.config.queue_publish_rate_limit) {
        rollback_job(&app_state, &job_id).await;
        warn!(
            "Publish queue saturated ({} in-flight >= limit {}); rejecting job {}",
            in_flight,
            app_state.config.queue_publish_rate_limit,
            job_id
        );
        return Err(AppError::TooManyRequests(format!(
            "Publish queue is saturated ({} in-flight jobs)",
            in_flight
        )));
    }

    // Transition inbox state via Barbican publish_start action
    app_state.barbican_client
        .publish_start(&object_id, &job_id)
        .await
        .map_err(|e| {
            error!("Failed to signal publish start to Barbican: {}", e);
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

    info!("Created publish job {} for inbox {}", job_id, object_id);

    Ok((
        StatusCode::ACCEPTED,
        Json(PublishInboxResponse {
            job_id,
            inbox_id: object_id.to_string(),
            status: "running".to_string(),
            job_type: "publish".to_string(),
        }),
    ))
}
