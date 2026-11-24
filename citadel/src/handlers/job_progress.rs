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

use axum::{Json, extract::State, http::StatusCode};
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
    /// Bytes processed so far by the job's operation: bytes **copied** for a
    /// publish job, bytes **removed** for a delete job. Forwarded to Barbican
    /// as `transferred_bytes` (publish) or `deleted_bytes` (delete). The wire
    /// name is kept for compatibility with existing workers.
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
        JobType::DeleteInbox { inbox_id } => forward_delete(&app_state, inbox_id, &request).await,
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

/// Forward delete-job progress to Barbican's `delete/progress`.
///
/// The worker's generic "bytes processed" counter (`transferred_bytes`) is
/// Barbican's `deleted_bytes` for a delete job. `citadel-job`'s
/// `execute_delete` does not currently report progress, so this path is
/// exercised only by tests until it does.
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
    if phase.is_empty() { default } else { phase }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_phase_or_empty_uses_default() {
        assert_eq!(phase_or("", "in_progress"), "in_progress");
        assert_eq!(phase_or("", "in_progress"), "in_progress");
    }

    #[test]
    fn test_phase_or_uses_provided_phase() {
        assert_eq!(phase_or("transferring", "in_progress"), "transferring");
        assert_eq!(phase_or("committed", "in_progress"), "committed");
    }
}

#[cfg(test)]
mod forwarding_tests {
    use super::*;
    use crate::config::CitadelConfig;
    use crate::integrations::barbican::MockInboxTransitioner;
    use crate::metrics::CitadelMetrics;
    use crate::storage::manager::StorageManager;
    use std::sync::Arc;

    /// Build an AppState whose barbican_client is a mock; the db_client and
    /// the rest are inert (forward_publish/forward_delete never touch them).
    async fn test_state(mock: MockInboxTransitioner) -> AppState {
        let config = CitadelConfig {
            barbican_api_url: "http://barbican:8080".to_string(),
            ..CitadelConfig::default()
        };
        let metrics = Arc::new(CitadelMetrics::new().expect("metrics init"));
        let client = mongodb::Client::with_uri_str("mongodb://localhost:27017")
            .await
            .expect("lazy client construction must not connect");
        AppState {
            config: Arc::new(config.clone()),
            auth_state: Arc::new(common::AuthMiddlewareState {
                validator: Arc::new(common::JwtValidator::new(
                    Arc::new(common::JwksCache::new(
                        "http://localhost:8080/certs".to_string(),
                        60,
                    )),
                    common::AuthConfig {
                        keycloak_url: "http://localhost:8080".to_string(),
                        keycloak_realm: "test".to_string(),
                        keycloak_client_id: "test".to_string(),
                        required_roles: vec!["read".to_string()],
                        allowed_algorithms: vec!["RS256".to_string()].into_iter().collect(),
                        jwks_refresh_secs: 60,
                        leeway_ms: 5000,
                    },
                )),
                required_roles: vec!["read".to_string()],
                service_name: "citadel".to_string(),
            }),
            storage_manager: Arc::new(StorageManager::new(&config)),
            barbican_client: Arc::new(mock),
            executor: Arc::new(crate::jobs::executor::MockExecutor::new(config.clone())),
            catalog_notifier: Arc::new(crate::integrations::catalog::NoOpCatalogNotifier),
            lta_notifier: Arc::new(crate::integrations::lta::NoOpLtaNotifier),
            metrics,
            db_client: Arc::new(client),
            callback_secret: Arc::new(tokio::sync::RwLock::new(Some("secret".to_string()))),
        }
    }

    fn request() -> JobProgressRequest {
        JobProgressRequest {
            job_id: "job-1".to_string(),
            timestamp: Utc::now(),
            nonce: "nonce-1".to_string(),
            total_files: 10,
            processed_files: 4,
            total_bytes: 1000,
            transferred_bytes: 400,
            phase: "transferring".to_string(),
        }
    }

    #[tokio::test]
    async fn test_forward_publish_calls_barbican_with_counts() {
        let inbox_id = ObjectId::new();
        let mut mock = MockInboxTransitioner::new();
        mock.expect_publish_progress()
            .withf(
                move |id,
                      job_id,
                      phase,
                      total_files,
                      processed_files,
                      total_bytes,
                      transferred_bytes| {
                    id == &inbox_id
                        && job_id == "job-1"
                        && phase == "transferring"
                        && *total_files == 10
                        && *processed_files == 4
                        && *total_bytes == 1000
                        && *transferred_bytes == 400
                },
            )
            .times(1)
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Err(AppError::ServiceUnavailable("down".into())) })
            });

        let state = test_state(mock).await;
        let req = request();
        // Barbican error is non-fatal at the forward layer too: the function
        // returns Err, and the caller (job_progress) treats it best-effort.
        let result = forward_publish(&state, &inbox_id, &req).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_forward_delete_calls_barbican_with_bytes() {
        let inbox_id = ObjectId::new();
        let mut mock = MockInboxTransitioner::new();
        mock.expect_delete_progress()
            .withf(move |id, job_id, phase, total_bytes, deleted_bytes| {
                id == &inbox_id
                    && job_id == "job-1"
                    && phase == "in_progress"
                    && *total_bytes == 1000
                    && *deleted_bytes == 400
            })
            .times(1)
            .returning(|_, _, _, _, _| {
                Box::pin(async { Err(AppError::ServiceUnavailable("down".into())) })
            });

        let state = test_state(mock).await;
        let mut req = request();
        req.phase = "".to_string(); // empty phase → default "in_progress"
        let result = forward_delete(&state, &inbox_id, &req).await;
        assert!(result.is_err());
    }
}
