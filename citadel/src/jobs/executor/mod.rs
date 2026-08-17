//! Job executor trait and implementations

use crate::error::Result;
use crate::models::JobDB;
use crate::jobs::types::JobStatusDetail;

/// Job executor trait
#[async_trait::async_trait]
pub trait JobExecutor: Send + Sync {
    async fn submit(&self, job: &JobDB) -> Result<String>;
    async fn cancel(&self, job_id: &str) -> Result<()>;
    async fn status(&self, job_id: &str) -> Result<JobStatusDetail>;
}

pub mod kubernetes;
pub mod mock;

pub use kubernetes::KubernetesExecutor;
pub use mock::MockExecutor;