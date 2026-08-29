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
use crate::db::{callback_nonces_collection, inboxes_collection, jobs_collection};
use crate::error::{AppError, Result};
use crate::models::{CallbackNonceDB, JobStatus, TransferResult};
use crate::jobs::types::JobType;

/// Nonce TTL window (seconds). Expired-but-present nonces are reusable; a
/// TTL index on `callback_nonces.expires_at` garbage-collects them.
pub const NONCE_TTL_SECS: i64 = 300;
/// Clock-skew tolerance for callback timestamps (seconds).
pub const TIMESTAMP_SKEW_SECS: i64 = 60;

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
    age.num_seconds() < NONCE_TTL_SECS && age.num_seconds() > -TIMESTAMP_SKEW_SECS
}

/// Decide whether a stored nonce is still valid (not expired).
fn nonce_is_live(existing: &CallbackNonceDB, now: DateTime<Utc>) -> bool {
    existing.expires_at > now
}

/// Check and store nonce to prevent replay attacks.
///
/// A nonce that is *present but expired* is treated as reusable: it is
/// replaced (upsert) rather than rejected, so the replay-protection window
/// stays semantically "5 minutes" while the TTL index garbage-collects old
/// rows.
async fn validate_and_store_nonce(
    app_state: &AppState,
    nonce: &str,
) -> Result<()> {
    let nonces_coll = callback_nonces_collection(&app_state.db_client, &app_state.config);

    // Check if nonce already exists
    let existing = nonces_coll
        .find_one(doc! { "nonce": nonce })
        .await
        .map_err(|e| AppError::DatabaseError(format!("Failed to check nonce: {}", e)))?;

    match existing {
        Some(nonce_doc) if nonce_is_live(&nonce_doc, Utc::now()) => {
            return Err(AppError::CallbackNonceReused);
        }
        Some(nonce_doc) => {
            // Expired: replace so the nonce value is recycled under a fresh
            // 5-minute window.
            warn!(
                "Nonce {} present but expired at {}; reusing with a fresh window",
                nonce, nonce_doc.expires_at
            );
            let fresh = CallbackNonceDB::new(nonce.to_string());
            nonces_coll
                .replace_one(doc! { "nonce": nonce }, fresh)
                .await
                .map_err(|e| AppError::DatabaseError(format!("Failed to replace nonce: {}", e)))?;
        }
        None => {
            // Store new nonce with TTL
            let nonce_doc = CallbackNonceDB::new(nonce.to_string());
            nonces_coll
                .insert_one(nonce_doc)
                .await
                .map_err(|e| AppError::DatabaseError(format!("Failed to store nonce: {}", e)))?;
        }
    }

    Ok(())
}

/// Authenticate callback request
async fn authenticate_callback(
    app_state: &AppState,
    headers: &axum::http::HeaderMap,
    request: &JobCallbackRequest,
) -> Result<()> {
    validate_callback_credentials(app_state, headers, request.timestamp, &request.nonce).await
}

/// Validate the shared callback credentials (timestamp + secret + nonce).
///
/// Reused by both the job callback and the job-progress endpoint so
/// `citadel-job` has exactly one trust edge (D9).
pub(crate) async fn validate_callback_credentials(
    app_state: &AppState,
    headers: &axum::http::HeaderMap,
    timestamp: DateTime<Utc>,
    nonce: &str,
) -> Result<()> {
    // Check timestamp
    if !validate_timestamp(timestamp) {
        return Err(AppError::CallbackTimestampExpired);
    }

    // Validate callback secret (X-Callback-Secret header, constant-time compare)
    validate_callback_secret(app_state, headers).await?;

    // Check nonce
    validate_and_store_nonce(app_state, nonce).await?;

    Ok(())
}

/// Constant-time byte comparison to prevent timing side channels when
/// validating the shared callback secret.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b)
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Validate the callback secret from the X-Callback-Secret header.
///
/// - Secret configured + header present: constant-time comparison; mismatch
///   is rejected with [`AppError::InvalidCallbackSecret`].
/// - Secret configured + header missing: rejected.
/// - No secret configured: dev-mode passthrough (warning is logged).
async fn validate_callback_secret(app_state: &AppState, headers: &axum::http::HeaderMap) -> Result<()> {
    let configured_secret = app_state
        .callback_secret
        .read()
        .await
        .clone();

    match configured_secret {
        Some(configured) => {
            let provided = headers
                .get("X-Callback-Secret")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.trim().to_string());

            match provided {
                Some(value) if ct_eq(value.as_bytes(), configured.as_bytes()) => Ok(()),
                _ => {
                    warn!("Callback rejected: missing or invalid X-Callback-Secret");
                    app_state.metrics.callback_errors_total.inc();
                    Err(AppError::InvalidCallbackSecret)
                }
            }
        }
        None => {
            // No secret configured — log warning in production
            if !matches!(app_state.config.job_executor, crate::config::JobExecutorType::Mock) {
                warn!("Callback secret not configured; relying on nonce+timestamp only");
            }
            Ok(())
        }
    }
}

/// Push an error onto a job's `errors` array (used for notification failures).
async fn push_job_error(
    app_state: &AppState,
    job_id: &str,
    code: &str,
    message: &str,
) {
    let jobs_coll = jobs_collection(&app_state.db_client, &app_state.config);
    let now = Utc::now();
    if let Err(e) = jobs_coll
        .update_one(
            doc! { "job_id": job_id },
            doc! {
                "$push": {
                    "errors": {
                        "code": code,
                        "message": message,
                        "timestamp": now.to_rfc3339(),
                    }
                }
            },
        )
        .await
    {
        // Only log; notification errors must never fail the callback itself.
        warn!("Failed to record notification error on job {}: {}", job_id, e);
    }
}

/// Notify catalog then LTA for a completed publish, recording (rather than
/// swallowing) failures. The `metrics.notify_errors_total{target}` counter is
/// incremented per failed target, and the failure is pushed onto the JobDB.
async fn notify_catalog_and_lta(
    app_state: &AppState,
    job_id: &str,
    inbox_doc: &common::InboxDB,
    transfer_result: &TransferResult,
    notify_catalog: bool,
    notify_lta: bool,
) {
    // Track catalog *success* (not just "was attempted") so LTA is skipped
    // when the catalog notification failed.
    let mut catalog_ok = true;

    if notify_catalog {
        match app_state.catalog_notifier.notify(inbox_doc, transfer_result).await {
            Ok(()) => {
                info!("Catalog notification succeeded for job {}", job_id);
            }
            Err(e) => {
                catalog_ok = false;
                error!("Catalog notification failed for job {}: {}", job_id, e);
                app_state.metrics.notify_errors_total.with_label_values(&["catalog"]).inc();
                push_job_error(app_state, job_id, "catalog_notify_failed", &format!("{}", e)).await;
            }
        }
    }

    // LTA only after catalog succeeds.
    if notify_lta && catalog_ok {
        match app_state.lta_notifier.notify(inbox_doc, transfer_result).await {
            Ok(()) => {
                info!("LTA notification succeeded for job {}", job_id);
            }
            Err(e) => {
                error!("LTA notification failed for job {}: {}", job_id, e);
                app_state.metrics.notify_errors_total.with_label_values(&["lta"]).inc();
                push_job_error(app_state, job_id, "lta_notify_failed", &format!("{}", e)).await;
            }
        }
    }
}

/// Persist the terminal-state JobDB update with an optimistic-lock version
/// filter. Returns `Ok(true)` when a row was actually matched/updated,
/// `Ok(false)` when the job was concurrently modified (idempotent duplicate
/// callback), and `Err` on database failure.
async fn persist_terminal_job_update(
    app_state: &AppState,
    job_id: &str,
    version: u32,
    status: JobStatus,
    result: Option<crate::models::JobResult>,
    extra_errors: Vec<crate::models::JobError>,
) -> Result<bool> {
    let jobs_coll = jobs_collection(&app_state.db_client, &app_state.config);
    let now = Utc::now();

    let mut set = doc! {
        "status": status_str(&status),
        "updated_at": now.to_rfc3339(),
    };
    if let Some(result) = result {
        set.insert("result", mongodb::bson::to_document(&result).map_err(|e| AppError::DatabaseError(e.to_string()))?);
    }

    let mut update = doc! { "$set": set, "$inc": { "version": 1 } };
    if !extra_errors.is_empty() {
        let errors_bson: Vec<mongodb::bson::Document> = extra_errors
            .into_iter()
            .filter_map(|e| mongodb::bson::to_document(&e).ok())
            .collect();
        if !errors_bson.is_empty() {
            update.insert(
                "$push",
                doc! { "errors": { "$each": errors_bson } },
            );
        }
    }

    let outcome = jobs_coll
        .update_one(
            doc! { "job_id": job_id, "version": version },
            update,
        )
        .await
        .map_err(|e| AppError::DatabaseError(format!("Failed to update job: {}", e)))?;

    Ok(outcome.matched_count > 0)
}

/// Derive the JSON status string from a [`JobStatus`] (mirrors serde's
/// `rename_all = "snake_case"` for the published field).
fn status_str(status: &JobStatus) -> &'static str {
    match status {
        JobStatus::Queued => "queued",
        JobStatus::Running => "running",
        JobStatus::Completed => "completed",
        JobStatus::Failed => "failed",
        JobStatus::Cancelling => "cancelling",
        JobStatus::Cancelled => "cancelled",
        JobStatus::TimedOut => "timed_out",
    }
}

/// Forward a terminal job result to Barbican so the inbox reaches its
/// terminal state. Errors are logged + metered but never fail the callback.
async fn forward_terminal_transition(
    app_state: &AppState,
    job_id: &str,
    job_type: &JobType,
    success: bool,
    files_transferred: usize,
    total_bytes: u64,
) {
    match job_type {
        JobType::PublishInbox { inbox_id, .. } => {
            let inbox_id = *inbox_id;
            let result = if success {
                // Reuse the inbox doc fetch for notifications to derive the
                // protected path (Barbican ignores the field per D11).
                let protected_path = inboxes_collection(&app_state.db_client, &app_state.config)
                    .find_one(doc! { "_id": inbox_id })
                    .await
                    .ok()
                    .flatten()
                    .map(|i| i.warehouse_path)
                    .unwrap_or_default();

                app_state.barbican_client
                    .publish_complete(&inbox_id, job_id, files_transferred as u64, total_bytes, &protected_path)
                    .await
            } else {
                app_state.barbican_client
                    .publish_failure(&inbox_id, job_id, "publish_failed", "Publish job reported failure")
                    .await
            };
            match result {
                Ok(_) => {
                    info!("Forwarded terminal publish state for job {} to Barbican", job_id);
                }
                Err(e) => {
                    error!("Failed to forward terminal publish state for job {} to Barbican: {}", job_id, e);
                    app_state.metrics.barbican_api_errors_total.inc();
                }
            }
        }
        JobType::DeleteInbox { inbox_id } => {
            let inbox_id = *inbox_id;
            let result = if success {
                app_state.barbican_client.delete_complete(&inbox_id, job_id).await
            } else {
                app_state.barbican_client
                    .delete_failure(&inbox_id, job_id, "delete_failed", "Delete job reported failure")
                    .await
            };
            match result {
                Ok(_) => {
                    info!("Forwarded terminal delete state for job {} to Barbican", job_id);
                }
                Err(e) => {
                    error!("Failed to forward terminal delete state for job {} to Barbican: {}", job_id, e);
                    app_state.metrics.barbican_api_errors_total.inc();
                }
            }
        }
    }
}

pub async fn job_callback(
    State(app_state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(request): Json<JobCallbackRequest>,
) -> Result<(StatusCode, Json<JobCallbackResponse>)> {
    // Authenticate the callback
    authenticate_callback(&app_state, &headers, &request).await?;

    let jobs_coll = jobs_collection(&app_state.db_client, &app_state.config);

    // Find the job
    let job = jobs_coll
        .find_one(doc! { "job_id": &request.job_id })
        .await
        .map_err(|e| AppError::DatabaseError(format!("Failed to find job: {}", e)))?
        .ok_or_else(|| AppError::JobNotFound(request.job_id.clone()))?;

    // Check if job is already completed
    if matches!(job.status, JobStatus::Completed | JobStatus::Cancelled | JobStatus::Failed | JobStatus::TimedOut) {
        warn!("Callback received for already completed job {}", request.job_id);
        return Ok((
            StatusCode::OK,
            Json(JobCallbackResponse { acknowledged: true }),
        ));
    }

    // Note: `job` borrows nothing across await points except via owned clones
    // after this point. We read fields we need up-front.
    let read_version = job.version;
    let job_type = job.job_type.clone();
    let job_id = job.job_id.clone();
    let job_created_at = job.created_at;
    // Capture before `request.result` is consumed by the match below.
    let reported_total_bytes = request.result.as_ref().map(|r| r.total_bytes).unwrap_or(0);

    // Determine the terminal state + result from the callback payload.
    let (terminal_status, terminal_result, extra_errors) = match request.status.as_str() {
        "completed" => {
            if let Some(result) = request.result {
                if result.success {
                    (
                        JobStatus::Completed,
                        Some(crate::models::JobResult {
                            success: true,
                            files_transferred: result.files_transferred,
                            total_bytes: result.total_bytes,
                            completed_at: Utc::now(),
                        }),
                        Vec::new(),
                    )
                } else {
                    (
                        JobStatus::Failed,
                        None,
                        vec![crate::models::JobError {
                            code: "job_execution_failed".to_string(),
                            message: format!("Job failed with {} bytes transferred", result.total_bytes),
                            timestamp: request.timestamp,
                        }],
                    )
                }
            } else {
                (
                    JobStatus::Failed,
                    None,
                    vec![crate::models::JobError {
                        code: "missing_result".to_string(),
                        message: "Callback completed without a result payload".to_string(),
                        timestamp: request.timestamp,
                    }],
                )
            }
        }
        "failed" => {
            let bytes = request.result.as_ref().map(|r| r.total_bytes).unwrap_or(0);
            (
                JobStatus::Failed,
                None,
                vec![crate::models::JobError {
                    code: "job_execution_failed".to_string(),
                    message: format!("Job failed with {} bytes transferred", bytes),
                    timestamp: request.timestamp,
                }],
            )
        }
        "timed_out" => (JobStatus::TimedOut, None, Vec::new()),
        other => {
            return Err(AppError::InternalError(format!(
                "Unknown callback status: {}",
                other
            )));
        }
    };

    // Capture the values needed for metrics / Barbican forwarding.
    let files_transferred = terminal_result
        .as_ref()
        .map(|r| r.files_transferred)
        .unwrap_or(0);
    let terminal_success = matches!(terminal_status, JobStatus::Completed);    // ------------------------------------------------------------------
    // (a) Persist the terminal JobDB write FIRST (optimistic-locked so a
    //     delayed/stale callback cannot overwrite a newer version — P2.3).
    // ------------------------------------------------------------------
    let matched = persist_terminal_job_update(
        &app_state,
        &job_id,
        read_version,
        terminal_status.clone(),
        terminal_result,
        extra_errors,
    )
    .await?;

    if !matched {
        // Concurrent modification: a newer callback/poller already finalized
        // this job. Treat as idempotent ack (do NOT forward to Barbican
        // again — would self-inflict 409s per P1.1).
        warn!(
            "Callback for job {} did not match version {}; job was updated concurrently",
            job_id, read_version
        );
        return Ok((
            StatusCode::OK,
            Json(JobCallbackResponse { acknowledged: true }),
        ));
    }

    // ------------------------------------------------------------------
    // (b) Metrics: terminal job counter + duration (P4.4).
    // ------------------------------------------------------------------
    app_state
        .metrics
        .jobs_total
        .with_label_values(&[status_str(&terminal_status)])
        .inc();
    let age_secs = (Utc::now() - job_created_at).num_seconds().max(0) as f64;
    app_state.metrics.jobs_duration.observe(age_secs);

    // (c) Notify catalog/LTA for successful publishes.
    if terminal_success {
        if let JobType::PublishInbox { notify_catalog, notify_lta, inbox_id } = &job_type {
            let inbox = inboxes_collection(&app_state.db_client, &app_state.config)
                .find_one(doc! { "_id": inbox_id })
                .await
                .map_err(|e| {
                    error!("Failed to fetch inbox for notification: {}", e);
                    AppError::DatabaseError(format!("Failed to fetch inbox: {}", e))
                })?;

            if let Some(ref inbox_doc) = inbox {
                let transfer_result = TransferResult {
                    success: true,
                    files_transferred: vec![],
                    files_failed: vec![],
                    total_bytes: reported_total_bytes,
                    partial_cleanup_completed: true,
                };
                notify_catalog_and_lta(
                    &app_state,
                    &job_id,
                    inbox_doc,
                    &transfer_result,
                    *notify_catalog,
                    *notify_lta,
                )
                .await;
            } else {
                warn!("Inbox {} not found for notifications", inbox_id);
            }
        }
    }

    // (d) Forward the terminal state to Barbican (P1.2, D1). Non-fatal.
    forward_terminal_transition(
        &app_state,
        &job_id,
        &job_type,
        terminal_success,
        files_transferred,
        reported_total_bytes,
    )
    .await;

    info!(
        "Processed callback for job {}: {}",
        request.job_id, request.status
    );

    Ok((
        StatusCode::OK,
        Json(JobCallbackResponse { acknowledged: true }),
    ))
}
