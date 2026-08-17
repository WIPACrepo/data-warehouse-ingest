//! Mock job executor for testing

use crate::config::CitadelConfig;
use crate::error::Result;
use crate::jobs::types::{JobStatusDetail, JobStatus};
use crate::jobs::executor::JobExecutor;
use crate::models::JobDB;
use tracing::info;

pub struct MockExecutor {
    _config: CitadelConfig,
}

impl MockExecutor {
    pub fn new(config: CitadelConfig) -> Self {
        Self { _config: config }
    }
}

#[async_trait::async_trait]
impl JobExecutor for MockExecutor {
    async fn submit(&self, job: &JobDB) -> Result<String> {
        info!("Mock executor submitting job {}", job.job_id);
        // In mock mode, we would spawn a local process
        Ok(job.job_id.clone())
    }
    
    async fn cancel(&self, job_id: &str) -> Result<()> {
        info!("Mock executor cancelling job {}", job_id);
        Ok(())
    }
    
    async fn status(&self, job_id: &str) -> Result<JobStatusDetail> {
        Ok(JobStatusDetail {
            job_id: job_id.to_string(),
            status: JobStatus::Completed,
            progress: None,
            executor: crate::jobs::types::JobExecutorInfo {
                executor_type: "mock".to_string(),
                executor_id: Some("mock-pid".to_string()),
            },
        })
    }
}