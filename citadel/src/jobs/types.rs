//! Job types and status definitions

use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

/// Job types that Citadel can execute
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobType {
    PublishInbox {
        inbox_id: ObjectId,
        notify_catalog: bool,
        notify_lta: bool,
    },
    DeleteInbox {
        inbox_id: ObjectId,
    },
}

/// Job status in the lifecycle
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelling,
    Cancelled,
    TimedOut,
}

/// Job progress information
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JobProgress {
    pub total_files: usize,
    pub processed_files: usize,
    pub total_bytes: u64,
    pub processed_bytes: u64,
    pub phase: String,
    pub last_updated: DateTime<Utc>,
}

/// Job executor information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobExecutorInfo {
    #[serde(rename = "type")]
    pub executor_type: String,
    pub executor_id: Option<String>,
}

/// Job status detail for API responses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobStatusDetail {
    pub job_id: String,
    pub status: JobStatus,
    pub progress: Option<JobProgress>,
    pub executor: JobExecutorInfo,
}

/// Job record for queue management
#[derive(Debug, Clone)]
pub struct Job {
    pub job_id: String,
    pub inbox_id: ObjectId,
    pub job_type: JobType,
    pub created_at: DateTime<Utc>,
}