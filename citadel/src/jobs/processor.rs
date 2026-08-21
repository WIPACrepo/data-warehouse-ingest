//! Job processor - orchestrates job execution

use crate::config::CitadelConfig;
use crate::error::Result;
use crate::jobs::types::JobType;
use crate::jobs::queue::JobQueue;
use crate::jobs::executor::JobExecutor;
use crate::models::JobDB;
use tracing::{info, error};

/// Job processor that manages job lifecycle
pub struct JobProcessor {
    queue: JobQueue,
    executor: Box<dyn JobExecutor>,
}

impl JobProcessor {
    pub fn new(config: CitadelConfig, executor: Box<dyn JobExecutor>) -> Self {
        let queue = JobQueue::new(config);
        Self { queue, executor }
    }
    
    pub async fn process_job(&self, job: JobDB) -> Result<()> {
        info!("Processing job {}", job.job_id);
        
        // Acquire appropriate permit based on job type
        let _permit = match &job.job_type {
            JobType::PublishInbox { .. } => self.queue.acquire_publish_permit().await?,
            JobType::DeleteInbox { .. } => self.queue.acquire_delete_permit().await?,
        };
        
        // Execute job
        match self.executor.submit(&job).await {
            Ok(_) => info!("Job {} submitted successfully", job.job_id),
            Err(e) => {
                error!("Job {} submission failed: {}", job.job_id, e);
                return Err(e);
            }
        }
        
        Ok(())
    }
}