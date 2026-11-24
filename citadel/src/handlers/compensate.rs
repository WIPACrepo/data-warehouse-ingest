//! Compensating actions when an operation fails *after* its Barbican
//! `*/start` transition succeeded.
//!
//! Without compensation the inbox stays `creating`/`publishing`/`deleting`
//! forever, and (for publish/delete) the job document stays `queued`, which
//! counts toward the in-flight quota (`count_in_flight_jobs`) forever.
//!
//! Compensation is **best-effort**: every step is logged, failures inside
//! compensation are never raised, and the caller always returns the
//! *original* error.
//!
//! It must only run when the job is known **not** to be running. After a
//! successful `executor.submit` the job may already be transferring, so a
//! later failure (e.g. updating the job document) is only logged and left to
//! the job-status poller to reconcile.

use chrono::Utc;
use mongodb::bson::{doc, oid::ObjectId};
use tracing::{error, warn};

use crate::AppState;
use crate::error::AppError;

/// Which operation is being compensated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Create,
    Publish,
    Delete,
}

/// Error codes sent to Barbican's `*/failure` and stored on the job.
pub mod codes {
    /// The inbox directory could not be created.
    pub const STORAGE_ERROR: &str = "storage_error";
    /// Barbican rejected or did not answer the completing transition.
    pub const BARBICAN_ERROR: &str = "barbican_error";
    /// The job could not be submitted to the executor.
    pub const EXECUTOR_ERROR: &str = "executor_error";
}

/// Move the inbox to `failed` via Barbican and, for publish/delete, mark the
/// job document `failed` so it leaves the in-flight quota.
pub async fn compensate(
    state: &AppState,
    op: Operation,
    inbox_id: &ObjectId,
    job_id: &str,
    code: &str,
    original: &AppError,
) {
    let message = original.to_string();
    warn!(
        ?op,
        %inbox_id,
        job_id,
        code,
        "Compensating failed operation: {}",
        message
    );

    let barbican = &state.barbican_client;
    let result = match op {
        Operation::Create => {
            barbican
                .create_failure(inbox_id, job_id, code, &message)
                .await
        }
        Operation::Publish => {
            barbican
                .publish_failure(inbox_id, job_id, code, &message)
                .await
        }
        Operation::Delete => {
            barbican
                .delete_failure(inbox_id, job_id, code, &message)
                .await
        }
    };
    if let Err(e) = result {
        error!(
            ?op,
            %inbox_id,
            job_id,
            "Compensation: Barbican *_failure call failed (inbox may stay in progress): {}",
            e
        );
    }

    if matches!(op, Operation::Publish | Operation::Delete) {
        mark_job_failed(state, job_id, code, &message).await;
    }
}

/// Set the job document to `failed` with an error entry (best-effort).
async fn mark_job_failed(state: &AppState, job_id: &str, code: &str, message: &str) {
    let jobs = crate::db::jobs_collection(&state.db_client, &state.config);
    let now = Utc::now();
    let update = doc! {
        "$set": { "status": "failed", "updated_at": now.to_rfc3339() },
        "$push": { "errors": { "code": code, "message": message, "timestamp": now.to_rfc3339() } },
    };
    if let Err(e) = jobs.update_one(doc! { "job_id": job_id }, update).await {
        error!(
            job_id,
            "Compensation: failed to mark job failed (it may count toward the quota): {}", e
        );
    }
}
