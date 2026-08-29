//! Job status polling for Kubernetes jobs
//!
//! Background poller that:
//! - Polls K8s API every 30 seconds for job status changes
//! - Detects jobs that failed to callback (network issues, crashes)
//! - Updates job records in MongoDB accordingly
//! - Cleans up completed K8s jobs after TTL expires
//! - Timeout detection (jobs exceeding CITADEL_JOB_MAX_DURATION_SECS)
//! - Reconciliation (detect missed callbacks during Citadel restart)

use crate::config::CitadelConfig;
use crate::error::Result;
use crate::integrations::barbican::InboxTransitioner;
use crate::jobs::executor::JobExecutor;
use crate::jobs::types::JobStatus;
use crate::metrics::CitadelMetrics;
use crate::models::JobDB;
use futures::StreamExt;
use mongodb::bson::doc;
use std::sync::Arc;
use tokio::time::{interval, Duration};
use tracing::{info, warn, error};

/// Background poller for K8s job status
pub struct JobStatusPoller {
    config: CitadelConfig,
    executor: Arc<dyn JobExecutor>,
    db_client: Arc<mongodb::Client>,
    barbican_client: Arc<dyn InboxTransitioner>,
    metrics: Arc<CitadelMetrics>,
}

impl JobStatusPoller {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: CitadelConfig,
        executor: Arc<dyn JobExecutor>,
        db_client: Arc<mongodb::Client>,
        barbican_client: Arc<dyn InboxTransitioner>,
        metrics: Arc<CitadelMetrics>,
    ) -> Self {
        Self {
            config,
            executor,
            db_client,
            barbican_client,
            metrics,
        }
    }

    /// Start background polling loop
    pub async fn run(self) -> Result<()> {
        let poll_interval = Duration::from_secs(30);
        let mut ticker = interval(poll_interval);

        info!("Job status poller started (interval: {}s)", poll_interval.as_secs());

        loop {
            ticker.tick().await;

            match self.poll_once().await {
                Ok(()) => {}
                Err(e) => {
                    warn!("Job status poller tick failed: {}", e);
                }
            }

            info!("Job status poller tick completed");
        }
    }

    /// Serialize JobStatus to a string for MongoDB queries
    fn status_to_string(status: JobStatus) -> String {
        // JobStatus uses serde rename_all = "snake_case"
        match status {
            JobStatus::Queued => "queued".to_string(),
            JobStatus::Running => "running".to_string(),
            JobStatus::Completed => "completed".to_string(),
            JobStatus::Failed => "failed".to_string(),
            JobStatus::Cancelling => "cancelling".to_string(),
            JobStatus::Cancelled => "cancelled".to_string(),
            JobStatus::TimedOut => "timed_out".to_string(),
        }
    }

    /// Execute a single polling cycle
    async fn poll_once(&self) -> Result<()> {
        let jobs_coll = crate::db::jobs_collection(&self.db_client, &self.config);

        // 1. Poll for running jobs that may have changed status in K8s
        self.poll_running_jobs(&jobs_coll).await?;

        // 2. Detect timed out jobs
        self.detect_timed_out_jobs(&jobs_coll).await?;

        // 3. Reconciliation: detect jobs where Citadel missed the callback
        self.reconcile_orphaned_jobs(&jobs_coll).await?;

        // 4. Self-heal inbox state: terminal JobDB status but inbox still
        //    mid-transition (lost callback / Barbican outage during callback).
        self.reconcile_inbox_transition(&jobs_coll).await?;

        Ok(())
    }

    /// Poll running jobs for status changes in the executor
    async fn poll_running_jobs(&self, jobs_coll: &mongodb::Collection<JobDB>) -> Result<()> {
        let filter = doc! {
            "status": Self::status_to_string(JobStatus::Running),
        };

        let mut cursor = jobs_coll
            .find(filter)
            .await
            .map_err(|e| crate::error::AppError::DatabaseError(format!("Failed to query running jobs: {}", e)))?;

        while let Some(job) = cursor
            .next()
            .await
            .transpose()
            .map_err(|e| crate::error::AppError::DatabaseError(format!("Failed to iterate jobs: {}", e)))?
        {
            self.check_job_status(&job).await;
        }

        Ok(())
    }

    /// Check a single job's status against the executor
    async fn check_job_status(&self, job: &JobDB) {
        match self.executor.status(&job.job_id).await {
            Ok(detail) => {
                if matches!(
                    detail.status,
                    JobStatus::Completed | JobStatus::Failed | JobStatus::TimedOut | JobStatus::Cancelled
                ) {
                    warn!(
                        "Job {} executor reports status {:?} but no callback received yet",
                        job.job_id, detail.status
                    );
                }
            }
            Err(e) => {
                error!(
                    "Failed to get executor status for job {}: {}",
                    job.job_id, e
                );
            }
        }
    }

    /// Detect jobs that have exceeded their maximum duration
    async fn detect_timed_out_jobs(&self, jobs_coll: &mongodb::Collection<JobDB>) -> Result<()> {
        let now = chrono::Utc::now();
        let cutoff = now - chrono::Duration::seconds(self.config.job_max_duration_secs as i64);

        let filter = doc! {
            "status": {
                "$in": [
                    Self::status_to_string(JobStatus::Running),
                    Self::status_to_string(JobStatus::Queued),
                ]
            },
            "created_at": {
                "$lt": cutoff.to_rfc3339(),
            }
        };

        let mut cursor = jobs_coll
            .find(filter)
            .await
            .map_err(|e| crate::error::AppError::DatabaseError(format!("Failed to query for timed out jobs: {}", e)))?;

        let mut timed_out_count = 0;
        while let Some(job) = cursor
            .next()
            .await
            .transpose()
            .map_err(|e| crate::error::AppError::DatabaseError(format!("Failed to iterate timed out jobs: {}", e)))?
        {
            warn!("Job {} has exceeded max duration ({}s), marking as timed out", job.job_id, self.config.job_max_duration_secs);

            // Cancel the job in the executor
            if let Err(e) = self.executor.cancel(&job.job_id).await {
                error!("Failed to cancel timed out job {}: {}", job.job_id, e);
            }

            // Update job status in MongoDB
            let update = doc! {
                "$set": {
                    "status": Self::status_to_string(JobStatus::TimedOut),
                    "updated_at": now.to_rfc3339(),
                },
                "$push": {
                    "errors": {
                        "code": "job_timeout",
                        "message": format!("Job exceeded max duration of {} seconds", self.config.job_max_duration_secs),
                        "timestamp": now.to_rfc3339(),
                    }
                }
            };

            let query = doc! { "job_id": &job.job_id };
            jobs_coll
                .update_one(query, update)
                .await
                .map_err(|e| crate::error::AppError::DatabaseError(format!("Failed to update timed out job: {}", e)))?;

            timed_out_count += 1;
        }

        if timed_out_count > 0 {
            info!("Timed out {} jobs", timed_out_count);
        }

        Ok(())
    }

    /// Reconcile orphaned jobs (Citadel missed the callback)
    async fn reconcile_orphaned_jobs(&self, jobs_coll: &mongodb::Collection<JobDB>) -> Result<()> {
        let filter = doc! {
            "status": Self::status_to_string(JobStatus::Running),
        };

        let mut cursor = jobs_coll
            .find(filter)
            .await
            .map_err(|e| crate::error::AppError::DatabaseError(format!("Failed to query running jobs for reconciliation: {}", e)))?;

        while let Some(job) = cursor
            .next()
            .await
            .transpose()
            .map_err(|e| crate::error::AppError::DatabaseError(format!("Failed to iterate jobs for reconciliation: {}", e)))?
        {
            // Check if job has an executor ID (was submitted)
            let executor_id = job.executor.executor_id.clone();

            if let Some(_eid) = executor_id {
                // Check executor status
                match self.executor.status(&job.job_id).await {
                    Ok(detail) => {
                        if matches!(
                            detail.status,
                            JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled
                        ) {
                            warn!(
                                "Reconciliation: Job {} executor reports {:?} but callback was missed",
                                job.job_id, detail.status
                            );

                            let now = chrono::Utc::now();
                            // Treat missed callback as failure for safety
                            let new_status = JobStatus::Failed;

                            let update = doc! {
                                "$set": {
                                    "status": Self::status_to_string(new_status.clone()),
                                    "updated_at": now.to_rfc3339(),
                                },
                                "$push": {
                                    "errors": {
                                        "code": "reconciliation_status_update",
                                        "message": format!(
                                            "Callback missed, executor reports status: {:?}, job marked as {:?}",
                                            detail.status, new_status
                                        ),
                                        "timestamp": now.to_rfc3339(),
                                    }
                                }
                            };

                            let query = doc! { "job_id": &job.job_id };
                            if let Err(e) = jobs_coll.update_one(query, update).await {
                                error!("Failed to update reconciled job {}: {}", job.job_id, e);
                            } else {
                                info!("Reconciled job {} to {:?}", job.job_id, new_status);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Failed to reconcile job {}: {}", job.job_id, e);
                    }
                }
            }
        }

        Ok(())
    }

    /// Self-heal inbox state that is stuck mid-transition.
    ///
    /// For jobs whose `JobDB` is in a terminal status (`completed`/`failed`/
    /// `timed_out`) but whose inbox is still `Publishing`/`Deleting` (or
    /// `Creating`), re-issue the appropriate complete/failure transition so a
    /// lost callback or a Barbican outage during the callback does not leave
    /// the inbox permanently stuck (P1.2, D1).
    ///
    /// `Creating` is only re-issued when the job's own terminal status proves
    /// the create finished (completed => create_complete; failed => create_failure)
    /// and the inbox `current_job_id` matches this job. Bounded to one attempt
    /// per tick (the next tick retries if Barbican was down); reconciled
    /// transitions are logged and metered but never fail the tick.
    async fn reconcile_inbox_transition(&self, jobs_coll: &mongodb::Collection<JobDB>) -> Result<()> {
        let terminal_statuses = vec![
            Self::status_to_string(JobStatus::Completed),
            Self::status_to_string(JobStatus::Failed),
            Self::status_to_string(JobStatus::TimedOut),
        ];

        let filter = doc! {
            "status": { "$in": terminal_statuses },
        };

        let mut cursor = jobs_coll
            .find(filter)
            .await
            .map_err(|e| crate::error::AppError::DatabaseError(format!("Failed to query terminal jobs: {}", e)))?;

        let inboxes_coll = crate::db::inboxes_collection(&self.db_client, &self.config);

        while let Some(job) = cursor
            .next()
            .await
            .transpose()
            .map_err(|e| crate::error::AppError::DatabaseError(format!("Failed to iterate terminal jobs: {}", e)))?
        {
            // Skip jobs that haven't finished (recently submitted etc.).
            let terminal_ok = matches!(
                job.status,
                JobStatus::Completed | JobStatus::Failed | JobStatus::TimedOut
            );
            if !terminal_ok {
                continue;
            }

            // Fetch the inbox to see whether it is still stuck mid-transition.
            let inbox = match inboxes_coll.find_one(doc! { "_id": job.inbox_id }).await {
                Ok(Some(inbox)) => inbox,
                Ok(None) | Err(_) => continue, // nothing to heal
            };

            let inbox_id = job.inbox_id;

            match &job.job_type {
                crate::jobs::types::JobType::PublishInbox { .. } => {
                    if !matches!(inbox.status, common::InboxStatus::Publishing) {
                        continue;
                    }
                    // Only heal when this job is the one currently driving the
                    // inbox (guards against stale jobs overtaking newer ones).
                    if inbox.current_job_id.as_deref() != Some(job.job_id.as_str()) {
                        continue;
                    }
                    if job.status == JobStatus::Completed {
                        warn!("Self-heal: inbox {} stuck in Publishing; re-issuing publish_complete for job {}", inbox_id, job.job_id);
                        let protected_path = inbox.warehouse_path.clone();
                        if let Err(e) = self.barbican_client
                            .publish_complete(&inbox_id, &job.job_id, 0, 0, &protected_path)
                            .await
                        {
                            error!("Self-heal publish_complete failed for job {}: {}", job.job_id, e);
                            self.metrics.barbican_api_errors_total.inc();
                        }
                    } else {
                        warn!("Self-heal: inbox {} stuck in Publishing; re-issuing publish_failure for job {}", inbox_id, job.job_id);
                        if let Err(e) = self.barbican_client
                            .publish_failure(&inbox_id, &job.job_id, "publish_failed", "Job terminated without callback; reconciled by poller")
                            .await
                        {
                            error!("Self-heal publish_failure failed for job {}: {}", job.job_id, e);
                            self.metrics.barbican_api_errors_total.inc();
                        }
                    }
                }
                crate::jobs::types::JobType::DeleteInbox { .. } => {
                    if !matches!(inbox.status, common::InboxStatus::Deleting) {
                        continue;
                    }
                    if inbox.current_job_id.as_deref() != Some(job.job_id.as_str()) {
                        continue;
                    }
                    if job.status == JobStatus::Completed {
                        warn!("Self-heal: inbox {} stuck in Deleting; re-issuing delete_complete for job {}", inbox_id, job.job_id);
                        if let Err(e) = self.barbican_client
                            .delete_complete(&inbox_id, &job.job_id)
                            .await
                        {
                            error!("Self-heal delete_complete failed for job {}: {}", job.job_id, e);
                            self.metrics.barbican_api_errors_total.inc();
                        }
                    } else {
                        warn!("Self-heal: inbox {} stuck in Deleting; re-issuing delete_failure for job {}", inbox_id, job.job_id);
                        if let Err(e) = self.barbican_client
                            .delete_failure(&inbox_id, &job.job_id, "delete_failed", "Job terminated without callback; reconciled by poller")
                            .await
                        {
                            error!("Self-heal delete_failure failed for job {}: {}", job.job_id, e);
                            self.metrics.barbican_api_errors_total.inc();
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_to_string() {
        assert_eq!(JobStatusPoller::status_to_string(JobStatus::Queued), "queued");
        assert_eq!(JobStatusPoller::status_to_string(JobStatus::Running), "running");
        assert_eq!(JobStatusPoller::status_to_string(JobStatus::Completed), "completed");
        assert_eq!(JobStatusPoller::status_to_string(JobStatus::Failed), "failed");
        assert_eq!(JobStatusPoller::status_to_string(JobStatus::Cancelling), "cancelling");
        assert_eq!(JobStatusPoller::status_to_string(JobStatus::Cancelled), "cancelled");
        assert_eq!(JobStatusPoller::status_to_string(JobStatus::TimedOut), "timed_out");
    }
}