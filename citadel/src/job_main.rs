//! citadel-job: Job executor for file transfers
//!
//! This binary runs as a Kubernetes Job or local process to execute
//! file transfer operations managed by the Citadel service.
//!
//! # Environment Variables
//!
//! Required:
//! - `JOB_ID` - Unique job identifier
//! - `JOB_TYPE` - Job type (publish_inbox, delete_inbox)
//! - `INBOX_ID` - MongoDB ObjectId of the inbox
//! - `INBOX_PATH` - Filesystem path to the inbox directory
//! - `WAREHOUSE_PATH` - Filesystem path for protected storage target
//!
//! Optional (with defaults):
//! - `CITADEL_CALLBACK_URL` - http://localhost:8081/api/v1/job/callback
//! - `CITADEL_CALLBACK_SECRET_FILE` - /etc/citadel/secrets/callback-secret
//! - `CITADEL_TRANSFER_PARALLELISM` - 4
//! - `CITADEL_TRANSFER_CHUNK_SIZE_MB` - 16
//! - `CITADEL_JOB_METADATA_POLL_INTERVAL_SECS` - 15
//! - `CITADEL_PROGRESS_UPDATE_INTERVAL_SECS` - 30
//! - `CITADEL_PROTECTED_BASE_PATH` - warehouse validation base (from CitadelConfig)
//! - `CITADEL_MONGODB_URI` / `CITADEL_MONGODB_DATABASE` - shared Mongo (for the
//!   metadata bail-out poll)
//! - `CITADEL_INBOX_*` - inbox security settings (see citadel/src/docs/configuration.md)
//!
//! # Progress / metadata polling (P4.1)
//!
//! While a publish transfer runs, `citadel-job` polls the shared MongoDB
//! `inboxes` collection every `CITADEL_JOB_METADATA_POLL_INTERVAL_SECS`:
//! - **Bail-out**: if the inbox is no longer `publishing`, the transfer is
//!   cancelled via a `CancellationToken` (surfaced as `MetadataChanged`).
//! - **Progress**: every `CITADEL_PROGRESS_UPDATE_INTERVAL_SECS` the current
//!   `ProgressTracker` snapshot is POSTed to Citadel's
//!   `POST /api/v1/job/progress` (X-Callback-Secret auth), which forwards it
//!   to Barbican. Progress-report failures are logged + retried with bounded
//!   backoff and never fail the transfer.

use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

#[derive(Error, Debug)]
pub enum JobError {
    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Storage error: {0}")]
    StorageError(String),

    #[error("File transfer failed: {0}")]
    FileTransferFailed(String),

    #[error("Callback failed: {0}")]
    CallbackFailed(String),

    #[error("Metadata poll detected state change: {0}")]
    MetadataChanged(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, JobError>;

/// Transfer result for callback
#[derive(Debug, Serialize, Clone)]
pub struct TransferResult {
    pub success: bool,
    pub files_transferred: usize,
    pub total_bytes: u64,
}

/// Job configuration from environment
#[derive(Debug, Clone)]
pub struct JobConfig {
    pub job_id: String,
    pub job_type: String,
    pub inbox_id: String,
    pub inbox_path: PathBuf,
    pub warehouse_path: PathBuf,
    pub callback_url: String,
    pub callback_secret_file: PathBuf,
    pub transfer_parallelism: usize,
    pub transfer_chunk_size_mb: usize,
    pub metadata_poll_interval_secs: u64,
    pub progress_update_interval_secs: u64,
    pub mongodb_uri: String,
    pub mongodb_database: String,
}

impl JobConfig {
    pub fn from_env() -> Result<Self> {
        let transfer_parallelism = env::var("CITADEL_TRANSFER_PARALLELISM")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(4);

        let transfer_chunk_size_mb = env::var("CITADEL_TRANSFER_CHUNK_SIZE_MB")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(16);

        let metadata_poll_interval_secs = env::var("CITADEL_JOB_METADATA_POLL_INTERVAL_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(15);

        let progress_update_interval_secs = env::var("CITADEL_PROGRESS_UPDATE_INTERVAL_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30);

        let mongodb_uri = env::var("CITADEL_MONGODB_URI")
            .unwrap_or_else(|_| "mongodb://localhost:27017".to_string());
        let mongodb_database =
            env::var("CITADEL_MONGODB_DATABASE").unwrap_or_else(|_| "data_warehouse".to_string());

        Ok(Self {
            job_id: env::var("JOB_ID")
                .map_err(|_| JobError::ConfigError("Missing JOB_ID".to_string()))?,
            job_type: env::var("JOB_TYPE")
                .map_err(|_| JobError::ConfigError("Missing JOB_TYPE".to_string()))?,
            inbox_id: env::var("INBOX_ID")
                .map_err(|_| JobError::ConfigError("Missing INBOX_ID".to_string()))?,
            inbox_path: env::var("INBOX_PATH")
                .map(PathBuf::from)
                .map_err(|_| JobError::ConfigError("Missing INBOX_PATH".to_string()))?,
            warehouse_path: env::var("WAREHOUSE_PATH")
                .map(PathBuf::from)
                .map_err(|_| JobError::ConfigError("Missing WAREHOUSE_PATH".to_string()))?,
            callback_url: env::var("CITADEL_CALLBACK_URL")
                .unwrap_or_else(|_| "http://localhost:8081/api/v1/job/callback".to_string()),
            callback_secret_file: env::var("CITADEL_CALLBACK_SECRET_FILE")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/etc/citadel/secrets/callback-secret")),
            transfer_parallelism,
            transfer_chunk_size_mb,
            metadata_poll_interval_secs,
            progress_update_interval_secs,
            mongodb_uri,
            mongodb_database,
        })
    }

    /// Base URL of the Citadel service (`CITADEL_CALLBACK_URL` minus the
    /// `/api/v1/job/callback` suffix), used to reach other endpoints such as
    /// `POST /api/v1/job/progress`.
    fn citadel_base_url(&self) -> String {
        let cb = self.callback_url.trim_end_matches('/');
        cb.trim_end_matches("/api/v1/job/callback").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_callback_url(url: &str) -> JobConfig {
        // from_env needs JOB_ID etc.; build a minimal config directly for the
        // pure helper under test.
        JobConfig {
            job_id: "test-job".to_string(),
            job_type: "publish_inbox".to_string(),
            inbox_id: "000000000000000000000000".to_string(),
            inbox_path: PathBuf::from("/tmp/inbox"),
            warehouse_path: PathBuf::from("/tmp/warehouse"),
            callback_url: url.to_string(),
            callback_secret_file: PathBuf::from("/tmp/secret"),
            transfer_parallelism: 4,
            transfer_chunk_size_mb: 16,
            metadata_poll_interval_secs: 15,
            progress_update_interval_secs: 30,
            mongodb_uri: "mongodb://localhost:27017".to_string(),
            mongodb_database: "data_warehouse".to_string(),
        }
    }

    fn sample_result() -> TransferResult {
        TransferResult {
            success: true,
            files_transferred: 1,
            total_bytes: 10,
        }
    }

    /// Regression (REVIEW #4): the callback timestamp must be the send time,
    /// not the job start time, or callbacks of jobs > 5 min are rejected.
    #[test]
    fn test_callback_request_uses_send_time_and_given_nonce() {
        let sent_at = Utc::now() + chrono::Duration::hours(2);
        let req = build_callback_request(
            "job-1",
            "completed",
            &sample_result(),
            sent_at,
            "n-1".into(),
        );
        assert_eq!(req.timestamp, sent_at);
        assert_eq!(req.nonce, "n-1");
        assert_eq!(req.job_id, "job-1");
        assert_eq!(req.status, "completed");
    }

    #[test]
    fn test_progress_request_uses_send_time_and_given_nonce() {
        let progress = citadel::jobs::types::JobProgress::default();
        let sent_at = Utc::now();
        let req = build_progress_request("job-1", &progress, sent_at, "n-2".into());
        assert_eq!(req.timestamp, sent_at);
        assert_eq!(req.nonce, "n-2");
    }

    #[test]
    fn test_citadel_base_url_strips_callback_suffix() {
        let config = config_with_callback_url("http://localhost:8081/api/v1/job/callback");
        assert_eq!(config.citadel_base_url(), "http://localhost:8081");
    }

    #[test]
    fn test_citadel_base_url_trailing_slash() {
        let config = config_with_callback_url("http://citadel:8081/api/v1/job/callback/");
        assert_eq!(config.citadel_base_url(), "http://citadel:8081");
    }

    #[test]
    fn test_citadel_base_url_unknown_suffix_unchanged() {
        let config = config_with_callback_url("http://citadel:8081/something/else");
        assert_eq!(
            config.citadel_base_url(),
            "http://citadel:8081/something/else"
        );
    }
}

/// Callback request to Citadel API
#[derive(Debug, Serialize)]
pub struct CallbackRequest {
    pub job_id: String,
    pub status: String,
    pub timestamp: DateTime<Utc>,
    pub nonce: String,
    pub result: Option<TransferResult>,
}

/// Build a callback request stamped with `now` and `nonce`.
///
/// The worker owns replay-protection values (decision recorded in
/// `citadel/src/docs/security-considerations.md`): every send attempt gets a
/// fresh timestamp and nonce. Citadel rejects timestamps older than
/// `NONCE_TTL_SECS` (300 s), so a timestamp captured at job start would make
/// the completion callback of any job longer than 5 minutes fail, and a
/// reused nonce would make a retry look like a replay.
fn build_callback_request(
    job_id: &str,
    status: &str,
    result: &TransferResult,
    now: DateTime<Utc>,
    nonce: String,
) -> CallbackRequest {
    CallbackRequest {
        job_id: job_id.to_string(),
        status: status.to_string(),
        timestamp: now,
        nonce,
        result: Some(result.clone()),
    }
}

/// Send callback to Citadel API with job result (one attempt; fresh
/// timestamp + nonce per call).
async fn send_callback(config: &JobConfig, status: &str, result: &TransferResult) -> Result<()> {
    let callback_request = build_callback_request(
        &config.job_id,
        status,
        result,
        Utc::now(),
        uuid::Uuid::new_v4().to_string(),
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| JobError::CallbackFailed(format!("Failed to build HTTP client: {}", e)))?;

    let response = client
        .post(&config.callback_url)
        .header("X-Callback-Secret", get_callback_secret(config)?)
        .json(&callback_request)
        .send()
        .await
        .map_err(|e| JobError::CallbackFailed(format!("Failed to send callback: {}", e)))?;

    if !response.status().is_success() {
        return Err(JobError::CallbackFailed(format!(
            "Callback returned status: {}",
            response.status()
        )));
    }

    info!("Successfully sent callback to Citadel API");
    Ok(())
}

/// Read callback secret from file
fn get_callback_secret(config: &JobConfig) -> Result<String> {
    let secret = env::var("CITADEL_CALLBACK_SECRET")
        .or_else(|_| {
            std::fs::read_to_string(&config.callback_secret_file)
                .map_err(|_| env::VarError::NotPresent)
        })
        .map_err(|_| JobError::CallbackFailed("No callback secret configured".to_string()))?;
    Ok(secret.trim().to_string())
}

// ---------------------------------------------------------------------------
// Progress reporting to Citadel (P4.1 / D9)
// ---------------------------------------------------------------------------

/// Progress report POSTed to Citadel's `POST /api/v1/job/progress`.
#[derive(Debug, Serialize)]
struct ProgressRequest {
    job_id: String,
    timestamp: DateTime<Utc>,
    nonce: String,
    total_files: u64,
    processed_files: u64,
    total_bytes: u64,
    transferred_bytes: u64,
    phase: String,
}

/// Build a progress report stamped with `now` and `nonce` (fresh per send
/// attempt, like [`build_callback_request`]).
fn build_progress_request(
    job_id: &str,
    progress: &citadel::jobs::types::JobProgress,
    now: DateTime<Utc>,
    nonce: String,
) -> ProgressRequest {
    ProgressRequest {
        job_id: job_id.to_string(),
        timestamp: now,
        nonce,
        total_files: progress.total_files as u64,
        processed_files: progress.processed_files as u64,
        total_bytes: progress.total_bytes,
        transferred_bytes: progress.processed_bytes,
        phase: progress.phase.clone(),
    }
}

/// POST a progress snapshot to Citadel with bounded retry + backoff.
/// Never fails the transfer: after the final attempt the error is logged and
/// the function returns. Each attempt carries a fresh timestamp and nonce.
async fn send_progress(config: &JobConfig, progress: &citadel::jobs::types::JobProgress) {
    let url = format!("{}/api/v1/job/progress", config.citadel_base_url());

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to build progress client: {}", e);
            return;
        }
    };

    let secret = get_callback_secret(config).unwrap_or_default();

    // Bounded retry with exponential backoff (1s, 2s).
    let mut attempt = 0;
    loop {
        let body = build_progress_request(
            &config.job_id,
            progress,
            Utc::now(),
            uuid::Uuid::new_v4().to_string(),
        );
        let result = client
            .post(&url)
            .header("X-Callback-Secret", &secret)
            .json(&body)
            .send()
            .await;

        match result {
            Ok(resp) if resp.status().is_success() => {
                info!(
                    "Reported progress for job {} (files {}/{}, bytes {}/{})",
                    config.job_id,
                    body.processed_files,
                    body.total_files,
                    body.transferred_bytes,
                    body.total_bytes
                );
                return;
            }
            Ok(resp) => {
                warn!(
                    "Progress report for job {} returned status {}",
                    config.job_id,
                    resp.status()
                );
            }
            Err(e) => {
                warn!("Progress report for job {} failed: {}", config.job_id, e);
            }
        }

        attempt += 1;
        if attempt >= 3 {
            warn!(
                "Giving up progress report for job {} after {} attempts",
                config.job_id, attempt
            );
            return;
        }
        tokio::time::sleep(Duration::from_secs(2_u64.pow(attempt))).await;
    }
}

/// Handle returned by [`spawn_progress_monitor`] carrying the tracker and
/// the cancellation token the transfer must observe.
struct ProgressMonitor {
    tracker: Arc<citadel::storage::progress::ProgressTracker>,
    cancel: CancellationToken,
}

/// Spawn the metadata bail-out poll + progress-report loop for a publish job.
///
/// The poll reads the shared `inboxes` collection; if the inbox is no longer
/// `publishing`, `cancel` is triggered so the in-flight transfer unwinds.
/// Progress snapshots are POSTed to Citadel on the progress cadence.
fn spawn_progress_monitor(config: &JobConfig) -> ProgressMonitor {
    let tracker = Arc::new(citadel::storage::progress::ProgressTracker::new(
        0,
        config.progress_update_interval_secs,
    ));
    let cancel = CancellationToken::new();

    let cfg = config.clone();
    let tracker_for_loop = tracker.clone();
    let cancel_for_loop = cancel.clone();
    tokio::spawn(async move {
        // Connect to the shared MongoDB. If it is unavailable, log and skip
        // the bail-out poll (progress reporting still works via the tracker).
        let client: Option<Arc<mongodb::Client>> =
            match common::db_setup(&cfg.mongodb_uri, &cfg.mongodb_database).await {
                Ok(c) => Some(Arc::new(c)),
                Err(e) => {
                    warn!(
                        "Failed to connect to MongoDB for metadata poll: {}; bail-out disabled",
                        e
                    );
                    None
                }
            };

        let mut poll_interval =
            tokio::time::interval(Duration::from_secs(cfg.metadata_poll_interval_secs.max(1)));
        let mut report_interval = tokio::time::interval(Duration::from_secs(
            cfg.progress_update_interval_secs.max(1),
        ));
        // The first `interval` tick fires immediately; consume it so the
        // first report happens after `CITADEL_PROGRESS_UPDATE_INTERVAL_SECS`.
        report_interval.tick().await;

        loop {
            tokio::select! {
                _ = poll_interval.tick() => {
                    let Some(client) = client.as_ref() else { continue };
                    let inbox_coll = common::db::db_collection::<common::InboxDB>(
                        client, &cfg.mongodb_database, "inboxes",
                    );
                    let inbox = inbox_coll
                        .find_one(mongodb::bson::doc! { "_id": &cfg.inbox_id })
                        .await;
                    match inbox {
                        Ok(Some(inbox)) if matches!(inbox.status, common::InboxStatus::Publishing) => {
                            // Still transferring; nothing to do.
                        }
                        Ok(Some(inbox)) => {
                            warn!(
                                "Inbox {} status changed to {:?}; cancelling transfer",
                                cfg.inbox_id, inbox.status
                            );
                            cancel_for_loop.cancel();
                            return;
                        }
                        Ok(None) => {
                            warn!("Inbox {} not found; cancelling transfer", cfg.inbox_id);
                            cancel_for_loop.cancel();
                            return;
                        }
                        Err(e) => {
                            warn!("Metadata poll failed for inbox {}: {}; continuing", cfg.inbox_id, e);
                        }
                    }
                }
                _ = report_interval.tick() => {
                    let p = tracker_for_loop.get_progress().await;
                    send_progress(&cfg, &p).await;
                }
            }
        }
    });

    ProgressMonitor { tracker, cancel }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let config = match JobConfig::from_env() {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to load job configuration: {}", e);
            std::process::exit(1);
        }
    };

    info!(
        "Starting job {} of type {} (parallelism={}, chunk_size={}MB)",
        config.job_id, config.job_type, config.transfer_parallelism, config.transfer_chunk_size_mb
    );
    info!("Inbox path: {:?}", config.inbox_path);
    info!("Warehouse path: {:?}", config.warehouse_path);

    // Execute based on job type
    let callback_result = match config.job_type.as_str() {
        "publish_inbox" => execute_publish(&config).await,
        "delete_inbox" => execute_delete(&config).await,
        _ => {
            error!("Unknown job type: {}", config.job_type);
            TransferResult {
                success: false,
                files_transferred: 0,
                total_bytes: 0,
            }
        }
    };

    // Send callback to Citadel
    let status = if callback_result.success {
        "completed"
    } else {
        "failed"
    };

    if let Err(e) = send_callback(&config, status, &callback_result).await {
        error!("Failed to send callback (retrying 2 more times): {}", e);
        // Retry with exponential backoff
        for attempt in 1..=2 {
            tokio::time::sleep(std::time::Duration::from_secs(2_u64.pow(attempt))).await;
            if let Err(e2) = send_callback(&config, status, &callback_result).await {
                error!("Callback retry {} failed: {}", attempt, e2);
            } else {
                info!("Callback retry {} succeeded", attempt);
                break;
            }
        }
    }

    info!("Job {} completed with status: {}", config.job_id, status);

    if callback_result.success {
        std::process::exit(0);
    } else {
        std::process::exit(1);
    }
}

/// Execute a publish (transfer) job
async fn execute_publish(config: &JobConfig) -> TransferResult {
    // Load the full Citadel configuration (protected base path + inbox
    // security settings). `from_env` only fails on malformed values; the
    // deployment forwards the relevant variables via the job environment.
    let citadel_config = match citadel::config::CitadelConfig::from_env() {
        Ok(c) => c,
        Err(e) => {
            error!(
                "Failed to load Citadel config for transfer: {}; using defaults",
                e
            );
            citadel::config::CitadelConfig::default()
        }
    };

    let audit_context = citadel::storage::transfer::AuditContext::new(
        mongodb::bson::oid::ObjectId::parse_str(&config.inbox_id).ok(),
        Some(config.job_id.clone()),
    );

    // Wire the ProgressTracker + metadata bail-out poll (D5/P4.1).
    let monitor = spawn_progress_monitor(config);

    match citadel::storage::transfer::transfer_files_with_metrics(
        &config.inbox_path,
        &config.warehouse_path,
        &citadel_config,
        config.transfer_parallelism,
        config.transfer_chunk_size_mb,
        audit_context,
        None,
        Some(monitor.tracker),
        Some(monitor.cancel),
    )
    .await
    {
        Ok(r) => {
            info!(
                "Publish completed: success={}, files={}, bytes={}",
                r.success,
                r.files_transferred.len(),
                r.total_bytes
            );
            TransferResult {
                success: r.success,
                files_transferred: r.files_transferred.len(),
                total_bytes: r.total_bytes,
            }
        }
        Err(e) => {
            error!("Publish failed: {}", e);
            // `TransferFailed` wraps every distinct error collected during
            // the run (and the security audit trail); log each one.
            if let citadel::storage::transfer::TransferError::TransferFailed { errors, .. } = &e {
                for err in errors {
                    error!("  - {}", err);
                }
            }
            TransferResult {
                success: false,
                files_transferred: 0,
                total_bytes: 0,
            }
        }
    }
}

/// Execute a delete job
async fn execute_delete(config: &JobConfig) -> TransferResult {
    info!("Executing delete job for inbox {:?}", config.inbox_path);

    match tokio::fs::remove_dir_all(&config.inbox_path).await {
        Ok(()) => {
            info!("Delete completed successfully");
            TransferResult {
                success: true,
                files_transferred: 0,
                total_bytes: 0,
            }
        }
        Err(e) => {
            error!("Delete failed: {}", e);
            TransferResult {
                success: false,
                files_transferred: 0,
                total_bytes: 0,
            }
        }
    }
}
