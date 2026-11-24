//! Mock job executor for testing
//!
//! Spawns local processes instead of Kubernetes Jobs.
//! Tracks running processes for cancellation.

use crate::config::CitadelConfig;
use crate::error::Result;
use crate::jobs::executor::JobExecutor;
use crate::jobs::types::{JobExecutorInfo, JobStatus, JobStatusDetail};
use crate::models::JobDB;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Mock executor that spawns local processes
pub struct MockExecutor {
    config: CitadelConfig,
    /// Running jobs tracked by job_id -> spawn timestamp
    running_jobs: Arc<RwLock<HashMap<String, chrono::DateTime<chrono::Utc>>>>,
    /// Completed job status map
    completed_jobs: Arc<RwLock<HashMap<String, JobStatus>>>,
}

impl MockExecutor {
    pub fn new(config: CitadelConfig) -> Self {
        Self {
            config,
            running_jobs: Arc::new(RwLock::new(HashMap::new())),
            completed_jobs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get the path to the citadel-job binary
    fn get_job_binary(&self) -> Option<std::path::PathBuf> {
        // Try common locations for the binary
        let candidates = vec![
            // Check target/debug or target/release
            std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|p| p.join("citadel-job"))),
            // Check the binary name from config
            Some(std::path::PathBuf::from(&self.config.job_binary)),
        ];

        for candidate in candidates {
            if let Some(path) = candidate
                && path.exists()
            {
                return Some(path);
            }
        }

        None
    }

    /// Build environment variables for the job process
    fn build_job_env(&self, job: &JobDB) -> Vec<(String, String)> {
        let job_type_str = match &job.job_type {
            crate::jobs::types::JobType::PublishInbox { .. } => "publish_inbox",
            crate::jobs::types::JobType::DeleteInbox { .. } => "delete_inbox",
        };

        let mut env = vec![
            ("JOB_ID".to_string(), job.job_id.clone()),
            ("JOB_TYPE".to_string(), job_type_str.to_string()),
            ("INBOX_ID".to_string(), job.inbox_id.to_string()),
            (
                "INBOX_PATH".to_string(),
                self.config
                    .inbox_base_path
                    .join(job.inbox_id.to_string())
                    .to_string_lossy()
                    .to_string(),
            ),
            (
                "WAREHOUSE_PATH".to_string(),
                self.config
                    .protected_base_path
                    .join(job.inbox_id.to_string())
                    .to_string_lossy()
                    .to_string(),
            ),
            (
                "CITADEL_CALLBACK_URL".to_string(),
                format!(
                    "http://localhost:{}/api/v1/job/callback",
                    self.config.bind_address.split(':').nth(1).unwrap_or("8081")
                ),
            ),
            (
                "CITADEL_TRANSFER_PARALLELISM".to_string(),
                self.config.transfer_parallelism.to_string(),
            ),
            (
                "CITADEL_TRANSFER_CHUNK_SIZE_MB".to_string(),
                self.config.transfer_chunk_size_mb.to_string(),
            ),
            (
                "CITADEL_JOB_METADATA_POLL_INTERVAL_SECS".to_string(),
                self.config.job_metadata_poll_interval_secs.to_string(),
            ),
            (
                "CITADEL_PROGRESS_UPDATE_INTERVAL_SECS".to_string(),
                self.config.progress_update_interval_secs.to_string(),
            ),
            (
                "CITADEL_MONGODB_URI".to_string(),
                self.config.mongodb_uri.clone(),
            ),
            (
                "CITADEL_MONGODB_DATABASE".to_string(),
                self.config.mongodb_database.clone(),
            ),
        ];

        // Add callback secret if file exists
        if self.config.job_callback_secret_file.exists() {
            env.push((
                "CITADEL_CALLBACK_SECRET_FILE".to_string(),
                self.config
                    .job_callback_secret_file
                    .to_string_lossy()
                    .to_string(),
            ));
        }

        // Forward the protected base path and inbox security settings so the
        // job validates and transfers with the same configuration as this
        // Citadel instance.
        for (name, value) in self.config.inbox_env_vars() {
            env.push((name, value));
        }

        env
    }
}

#[async_trait::async_trait]
impl JobExecutor for MockExecutor {
    async fn submit(&self, job: &JobDB) -> Result<String> {
        info!("Mock executor submitting job {}", job.job_id);

        // Build environment for the job
        let env_vars = self.build_job_env(job);

        // Try to spawn the binary
        let binary_path = self.get_job_binary();
        let _spawn_result = if let Some(ref path) = binary_path {
            match tokio::process::Command::new(path)
                .envs(env_vars.clone())
                .kill_on_drop(true)
                .spawn()
            {
                Ok(mut child) => {
                    let pid = child.id().unwrap_or(0);
                    info!(
                        "Mock executor spawned process for job {} (PID: {})",
                        job.job_id, pid
                    );

                    // Spawn a background task to monitor the child process
                    let job_id = job.job_id.clone();
                    let completed_jobs = self.completed_jobs.clone();
                    let running_jobs = self.running_jobs.clone();

                    tokio::spawn(async move {
                        match child.wait().await {
                            Ok(status) => {
                                let final_status = if status.success() {
                                    JobStatus::Completed
                                } else {
                                    JobStatus::Failed
                                };
                                info!(
                                    "Mock process for job {} exited with status: {:?}",
                                    job_id, final_status
                                );

                                let mut completed = completed_jobs.write().await;
                                completed.insert(job_id.clone(), final_status);

                                // Remove from running
                                let mut running = running_jobs.write().await;
                                running.remove(&job_id);
                            }
                            Err(e) => {
                                warn!("Mock process for job {} failed to wait: {}", job_id, e);
                                let mut completed = completed_jobs.write().await;
                                completed.insert(job_id.clone(), JobStatus::Failed);

                                let mut running = running_jobs.write().await;
                                running.remove(&job_id);
                            }
                        }
                    });

                    Some(true)
                }
                Err(e) => {
                    warn!("Failed to spawn mock process for job {}: {}", job.job_id, e);
                    Some(false)
                }
            }
        } else {
            warn!(
                "citadel-job binary not found, job {} will be tracked but not executed",
                job.job_id
            );
            None
        };

        // Track the job as running
        let mut running = self.running_jobs.write().await;
        running.insert(job.job_id.clone(), chrono::Utc::now());
        drop(running);

        Ok(job.job_id.clone())
    }

    async fn cancel(&self, job_id: &str) -> Result<()> {
        info!("Mock executor cancelling job {}", job_id);

        // Mark as completed with cancelled status
        let mut completed = self.completed_jobs.write().await;
        completed.insert(job_id.to_string(), JobStatus::Cancelled);

        // Remove from running
        let mut running = self.running_jobs.write().await;
        running.remove(job_id);

        Ok(())
    }

    async fn status(&self, job_id: &str) -> Result<JobStatusDetail> {
        // Check completed jobs first
        let completed = self.completed_jobs.read().await;
        if let Some(status) = completed.get(job_id) {
            return Ok(JobStatusDetail {
                job_id: job_id.to_string(),
                status: status.clone(),
                progress: None,
                executor: JobExecutorInfo {
                    executor_type: "mock".to_string(),
                    executor_id: Some(job_id.to_string()),
                },
            });
        }

        // Check running jobs
        let running = self.running_jobs.read().await;
        if running.contains_key(job_id) {
            return Ok(JobStatusDetail {
                job_id: job_id.to_string(),
                status: JobStatus::Running,
                progress: None,
                executor: JobExecutorInfo {
                    executor_type: "mock".to_string(),
                    executor_id: Some(job_id.to_string()),
                },
            });
        }

        // Default: assume running if not found
        Ok(JobStatusDetail {
            job_id: job_id.to_string(),
            status: JobStatus::Running,
            progress: None,
            executor: JobExecutorInfo {
                executor_type: "mock".to_string(),
                executor_id: Some(job_id.to_string()),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_executor_new() {
        let config = CitadelConfig::default();
        let executor = MockExecutor::new(config);
        assert!(executor.running_jobs.blocking_read().is_empty());
        assert!(executor.completed_jobs.blocking_read().is_empty());
    }

    #[tokio::test]
    async fn test_mock_executor_cancel() {
        let config = CitadelConfig::default();
        let executor = MockExecutor::new(config);

        let result = executor.cancel("test-job-123").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mock_executor_status_not_found() {
        let config = CitadelConfig::default();
        let executor = MockExecutor::new(config);

        let result = executor.status("test-job-123").await;
        assert!(result.is_ok());
        // Default status when job not found
        assert_eq!(result.unwrap().status, JobStatus::Running);
    }

    #[tokio::test]
    async fn test_mock_executor_status_completed() {
        let config = CitadelConfig::default();
        let executor = MockExecutor::new(config);

        // Mark as completed
        {
            let mut completed = executor.completed_jobs.write().await;
            completed.insert("test-job-123".to_string(), JobStatus::Completed);
        } // drop write lock before acquiring read lock

        let result = executor.status("test-job-123").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().status, JobStatus::Completed);
    }

    #[test]
    fn test_build_job_env() {
        let config = CitadelConfig::default();
        let executor = MockExecutor::new(config);

        let job = JobDB::new(
            "test-job".to_string(),
            mongodb::bson::oid::ObjectId::new(),
            crate::jobs::types::JobType::PublishInbox {
                inbox_id: mongodb::bson::oid::ObjectId::new(),
                notify_catalog: true,
                notify_lta: true,
            },
            "mock",
        );

        let env_vars = executor.build_job_env(&job);
        let env_map: HashMap<String, String> = env_vars.into_iter().collect();

        assert_eq!(env_map.get("JOB_ID"), Some(&"test-job".to_string()));
        assert_eq!(env_map.get("JOB_TYPE"), Some(&"publish_inbox".to_string()));
        assert_eq!(
            env_map.get("CITADEL_TRANSFER_PARALLELISM"),
            Some(&"4".to_string())
        );
        assert_eq!(
            env_map.get("CITADEL_TRANSFER_CHUNK_SIZE_MB"),
            Some(&"16".to_string())
        );
        assert_eq!(
            env_map.get("CITADEL_PROGRESS_UPDATE_INTERVAL_SECS"),
            Some(&"30".to_string())
        );
        assert_eq!(
            env_map.get("CITADEL_MONGODB_URI"),
            Some(&"mongodb://localhost:27017".to_string())
        );
        assert_eq!(
            env_map.get("CITADEL_MONGODB_DATABASE"),
            Some(&"data_warehouse".to_string())
        );
    }
}
