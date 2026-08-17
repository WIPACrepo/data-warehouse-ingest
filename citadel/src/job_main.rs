//! citadel-job: Job executor for file transfers
//! 
//! This binary runs as a Kubernetes Job or local process to execute
//! file transfer operations managed by the Citadel service.

use std::env;
use std::path::{Path, PathBuf};
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
    
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, JobError>;

/// Transfer result for callback
#[derive(Debug, Serialize)]
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
        
        let inbox_path = env::var("INBOX_PATH")
            .map(PathBuf::from)
            .map_err(|_| JobError::ConfigError("Missing INBOX_PATH".to_string()))?;
            
        let warehouse_path = env::var("WAREHOUSE_PATH")
            .map(PathBuf::from)
            .map_err(|_| JobError::ConfigError("Missing WAREHOUSE_PATH".to_string()))?;
        
        Ok(Self {
            job_id: env::var("JOB_ID")
                .map_err(|_| JobError::ConfigError("Missing JOB_ID".to_string()))?,
            job_type: env::var("JOB_TYPE")
                .map_err(|_| JobError::ConfigError("Missing JOB_TYPE".to_string()))?,
            inbox_id: env::var("INBOX_ID")
                .map_err(|_| JobError::ConfigError("Missing INBOX_ID".to_string()))?,
            inbox_path,
            warehouse_path,
            callback_url: env::var("CITADEL_CALLBACK_URL")
                .unwrap_or_else(|_| "http://localhost:8081/api/v1/job/callback".to_string()),
            callback_secret_file: env::var("CITADEL_CALLBACK_SECRET_FILE")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/etc/citadel/secrets/callback-secret")),
            callback_timestamp: Utc::now(),
            callback_nonce: uuid::Uuid::new_v4().to_string(),
            transfer_parallelism,
            transfer_chunk_size_mb,
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

// Include the storage transfer module
mod storage {
    pub mod transfer;
}

use storage::transfer::{transfer_files, TransferResult as StorageTransferResult};

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
    
    info!("Starting job {} of type {}", config.job_id, config.job_type);
    info!("Inbox path: {:?}", config.inbox_path);
    info!("Warehouse path: {:?}", config.warehouse_path);
    
    // Execute the file transfer
    let transfer_result = match transfer_files(
        &config.inbox_path,
        &config.warehouse_path,
        config.transfer_parallelism,
        config.transfer_chunk_size_mb,
    ).await {
        Ok(r) => {
            info!("Transfer completed: success={}", r.success);
            r
        }
        Err(e) => {
            error!("Transfer failed: {}", e);
            StorageTransferResult {
                success: false,
                files_transferred: vec![],
                files_failed: vec![],
                total_bytes: 0,
                partial_cleanup_completed: false,
            }
        }
    };
    
    // Convert to callback result
    let callback_result = TransferResult {
        success: transfer_result.success,
        files_transferred: transfer_result.files_transferred.len(),
        total_bytes: transfer_result.total_bytes,
    };
    
    // Send callback to Citadel
    if let Err(e) = send_callback(&config, &callback_result).await {
        error!("Failed to send callback: {}", e);
    }
    
    info!("Job {} completed", config.job_id);
    
    if callback_result.success {
        std::process::exit(0);
    } else {
        std::process::exit(1);
    }
}

/// Send callback to Citadel API with job result
async fn send_callback(config: &JobConfig, result: &TransferResult) -> Result<()> {
    let callback_secret = tokio::fs::read_to_string(&config.callback_secret_file)
        .await
        .map_err(|e| JobError::CallbackFailed(format!("Failed to read callback secret: {}", e)))?;
    
    let callback_request = CallbackRequest {
        job_id: config.job_id.clone(),
        status: if result.success { "completed".to_string() } else { "failed".to_string() },
        timestamp: config.callback_timestamp,
        nonce: config.callback_nonce.clone(),
        result: Some(TransferResult {
            success: result.success,
            files_transferred: result.files_transferred,
            total_bytes: result.total_bytes,
        }),
    };
    
    let client = reqwest::Client::new();
    let response = client
        .post(&config.callback_url)
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