//! Citadel-specific data models

use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

// Re-export job types from jobs::types to avoid duplication
pub use crate::jobs::types::{JobType, JobStatus, JobProgress, JobExecutorInfo};

/// Job error information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobError {
    pub code: String,
    pub message: String,
    pub timestamp: DateTime<Utc>,
}

/// Job result information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobResult {
    pub success: bool,
    pub files_transferred: usize,
    pub total_bytes: u64,
    pub completed_at: DateTime<Utc>,
}

/// Job record stored in MongoDB
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobDB {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    pub id: Option<ObjectId>,
    pub job_id: String, // Unique job identifier
    pub inbox_id: ObjectId,
    pub job_type: JobType,
    pub status: JobStatus,
    pub version: u32, // Optimistic locking version
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub executor: JobExecutorInfo,
    pub progress: Option<JobProgress>,
    pub errors: Vec<JobError>,
    pub result: Option<JobResult>,
    pub timeout_at: Option<DateTime<Utc>>,
}

impl JobDB {
    /// Create a new queued job
    pub fn new(job_id: String, inbox_id: ObjectId, job_type: JobType, executor_type: &str) -> Self {
        let now = Utc::now();
        Self {
            id: None,
            job_id,
            inbox_id,
            job_type,
            status: JobStatus::Queued,
            version: 0,
            created_at: now,
            updated_at: now,
            executor: JobExecutorInfo {
                executor_type: executor_type.to_string(),
                executor_id: None,
            },
            progress: None,
            errors: Vec::new(),
            result: None,
            timeout_at: None,
        }
    }
}

/// Callback nonce for replay attack prevention
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallbackNonceDB {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    pub id: Option<ObjectId>,
    pub nonce: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>, // 5 minutes from created_at
}

impl CallbackNonceDB {
    /// Create a new nonce that expires in 5 minutes
    pub fn new(nonce: String) -> Self {
        let now = Utc::now();
        let expires_at = now + chrono::Duration::minutes(5);
        Self {
            id: None,
            nonce,
            created_at: now,
            expires_at,
        }
    }
}

/// File information for transfer results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    pub path: String,
    pub size: u64,
    pub checksum: String,
}

/// Transfer result from storage operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferResult {
    pub success: bool,
    pub files_transferred: Vec<FileInfo>,
    pub files_failed: Vec<FileInfo>,
    pub total_bytes: u64,
    pub partial_cleanup_completed: bool,
}