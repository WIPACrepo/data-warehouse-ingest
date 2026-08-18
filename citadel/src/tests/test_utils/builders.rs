//! Test builders for Citadel domain objects

use chrono::Utc;
use mongodb::bson::oid::ObjectId;
use crate::models::JobDB;
use crate::jobs::types::{JobType, JobStatus, JobExecutorInfo};

/// Builder for JobDB test fixtures
pub struct JobBuilder {
    job_id: String,
    inbox_id: ObjectId,
    job_type: JobType,
    status: JobStatus,
    version: u32,
    executor_type: String,
    executor_id: Option<String>,
    timeout_at: Option<chrono::DateTime<Utc>>,
}

impl Default for JobBuilder {
    fn default() -> Self {
        Self {
            job_id: "test-job-001".to_string(),
            inbox_id: ObjectId::new(),
            job_type: JobType::PublishInbox {
                inbox_id: ObjectId::new(),
                notify_catalog: true,
                notify_lta: true,
            },
            status: JobStatus::Queued,
            version: 0,
            executor_type: "mock".to_string(),
            executor_id: None,
            timeout_at: None,
        }
    }
}

impl JobBuilder {
    pub fn job_id(mut self, job_id: &str) -> Self {
        self.job_id = job_id.to_string();
        self
    }

    pub fn inbox_id(mut self, inbox_id: ObjectId) -> Self {
        self.inbox_id = inbox_id;
        self
    }

    pub fn job_type(mut self, job_type: JobType) -> Self {
        self.job_type = job_type;
        self
    }

    pub fn status(mut self, status: JobStatus) -> Self {
        self.status = status;
        self
    }

    pub fn executor_type(mut self, executor_type: &str) -> Self {
        self.executor_type = executor_type.to_string();
        self
    }

    pub fn executor_id(mut self, executor_id: Option<String>) -> Self {
        self.executor_id = executor_id;
        self
    }

    pub fn timeout_at(mut self, timeout_at: chrono::DateTime<Utc>) -> Self {
        self.timeout_at = Some(timeout_at);
        self
    }

    pub fn build(self) -> JobDB {
        let now = Utc::now();
        JobDB {
            id: None,
            job_id: self.job_id,
            inbox_id: self.inbox_id,
            job_type: self.job_type,
            status: self.status,
            version: self.version,
            created_at: now,
            updated_at: now,
            executor: JobExecutorInfo {
                executor_type: self.executor_type,
                executor_id: self.executor_id,
            },
            progress: None,
            errors: vec![],
            result: None,
            timeout_at: self.timeout_at,
        }
    }
}

/// Create a default test job
pub fn default_test_job() -> JobDB {
    JobBuilder::default().build()
}

/// Create a queued publish job
pub fn queued_publish_job() -> JobDB {
    JobBuilder::default()
        .status(JobStatus::Queued)
        .job_type(JobType::PublishInbox {
            inbox_id: ObjectId::new(),
            notify_catalog: true,
            notify_lta: true,
        })
        .build()
}

/// Create a running publish job
pub fn running_publish_job() -> JobDB {
    JobBuilder::default()
        .status(JobStatus::Running)
        .executor_id(Some("mock-pid-123".to_string()))
        .job_type(JobType::PublishInbox {
            inbox_id: ObjectId::new(),
            notify_catalog: true,
            notify_lta: true,
        })
        .build()
}

/// Create a completed publish job
pub fn completed_publish_job() -> JobDB {
    JobBuilder::default()
        .status(JobStatus::Completed)
        .job_type(JobType::PublishInbox {
            inbox_id: ObjectId::new(),
            notify_catalog: true,
            notify_lta: true,
        })
        .build()
}

/// Create a queued delete job
pub fn queued_delete_job() -> JobDB {
    JobBuilder::default()
        .status(JobStatus::Queued)
        .job_type(JobType::DeleteInbox {
            inbox_id: ObjectId::new(),
        })
        .build()
}

/// Create a failed job
pub fn failed_job() -> JobDB {
    JobBuilder::default()
        .status(JobStatus::Failed)
        .build()
}