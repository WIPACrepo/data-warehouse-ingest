//! Job queue management

use crate::config::CitadelConfig;
use crate::error::Result;
use crate::jobs::types::Job;
use tokio::sync::Semaphore;
use std::sync::Arc;

/// Job queue manager with rate limiting
pub struct JobQueue {
    publish_semaphore: Arc<Semaphore>,
    delete_semaphore: Arc<Semaphore>,
    config: CitadelConfig,
}

impl JobQueue {
    pub fn new(config: CitadelConfig) -> Self {
        Self {
            publish_semaphore: Arc::new(Semaphore::new(config.queue_publish_rate_limit)),
            delete_semaphore: Arc::new(Semaphore::new(config.queue_delete_rate_limit)),
            config,
        }
    }
    
    pub async fn acquire_publish_permit(&self) -> Result<tokio::sync::SemaphorePermit<'_>> {
        self.publish_semaphore.acquire().await
            .map_err(|e| crate::error::CitadelError::InternalError(format!("Semaphore error: {}", e)))
    }
    
    pub async fn acquire_delete_permit(&self) -> Result<tokio::sync::SemaphorePermit<'_>> {
        self.delete_semaphore.acquire().await
            .map_err(|e| crate::error::CitadelError::InternalError(format!("Semaphore error: {}", e)))
    }
}