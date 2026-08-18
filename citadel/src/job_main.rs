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

use std::env;
use std::path::PathBuf;
use chrono::{DateTime, Utc};
use serde::Serialize;
use tracing::{info, error};
use thiserror::Error;

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
    pub callback_timestamp: DateTime<Utc>,
    pub callback_nonce: String,
    pub transfer_parallelism: usize,
    pub transfer_chunk_size_mb: usize,
    pub metadata_poll_interval_secs: u64,
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
            callback_timestamp: Utc::now(),
            callback_nonce: uuid::Uuid::new_v4().to_string(),
            transfer_parallelism,
            transfer_chunk_size_mb,
            metadata_poll_interval_secs,
        })
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

/// Send callback to Citadel API with job result
async fn send_callback(config: &JobConfig, status: &str, result: &TransferResult) -> Result<()> {
    let callback_request = CallbackRequest {
        job_id: config.job_id.clone(),
        status: status.to_string(),
        timestamp: config.callback_timestamp,
        nonce: config.callback_nonce.clone(),
        result: Some(result.clone()),
    };

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
        config.job_id,
        config.job_type,
        config.transfer_parallelism,
        config.transfer_chunk_size_mb
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
    match citadel::storage::transfer::transfer_files(
        &config.inbox_path,
        &config.warehouse_path,
        config.transfer_parallelism,
        config.transfer_chunk_size_mb,
    ).await {
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