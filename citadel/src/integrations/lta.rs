//! Long Term Archive (LTA) notification integration
//!
//! Notifies the LTA service about data ready for archiving.
//! Uses HTTP with exponential backoff retry logic.
//! Only called AFTER catalog notification succeeds.

use crate::config::CitadelConfig;
use crate::error::{AppError, Result};
use crate::models::TransferResult;
use common::models::InboxDB;
use tracing::{error, info, warn};

/// LTA notifier trait
#[async_trait::async_trait]
pub trait LtaNotifier: Send + Sync {
    async fn notify(&self, inbox: &InboxDB, transfer: &TransferResult) -> Result<()>;
    async fn retry(&self, notification_id: &str) -> Result<()>;
}

/// No-op LTA notifier for testing
pub struct NoOpLtaNotifier;

#[async_trait::async_trait]
impl LtaNotifier for NoOpLtaNotifier {
    async fn notify(&self, _inbox: &InboxDB, _transfer: &TransferResult) -> Result<()> {
        info!("NoOp LTA notification");
        Ok(())
    }

    async fn retry(&self, _notification_id: &str) -> Result<()> {
        info!("NoOp LTA retry");
        Ok(())
    }
}

/// HTTP-based LTA notifier
pub struct HttpLtaNotifier {
    config: CitadelConfig,
    client: reqwest::Client,
    lta_url: String,
}

impl HttpLtaNotifier {
    pub fn new(config: CitadelConfig, lta_url: Option<String>) -> Self {
        Self {
            config: config.clone(),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(
                    config.lta_notify_timeout_secs,
                ))
                .build()
                .unwrap_or_default(),
            lta_url: lta_url
                .unwrap_or_else(|| "http://lta:8080/api/v1/archive-request".to_string()),
        }
    }

    /// Send notification with exponential backoff retry
    async fn notify_with_retry(&self, inbox: &InboxDB, transfer: &TransferResult) -> Result<()> {
        let max_retries = self.config.lta_notify_retry_max;
        let mut backoff_ms = self.config.lta_notify_retry_backoff_ms;

        for attempt in 0..max_retries {
            match self.do_notify(inbox, transfer).await {
                Ok(()) => {
                    if attempt > 0 {
                        info!("LTA notification succeeded on attempt {}", attempt + 1);
                    }
                    return Ok(());
                }
                Err(e) => {
                    warn!(
                        "LTA notification failed on attempt {}/{}: {}",
                        attempt + 1,
                        max_retries,
                        e
                    );

                    if attempt < max_retries - 1 {
                        tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                        backoff_ms = (backoff_ms * 2).min(30000);
                    }
                }
            }
        }

        error!("LTA notification failed after {} retries", max_retries);
        Err(AppError::BadGateway("Failed after all retries".to_string()))
    }

    /// Perform a single notification request
    async fn do_notify(&self, inbox: &InboxDB, transfer: &TransferResult) -> Result<()> {
        #[derive(serde::Serialize)]
        struct ArchivePayload {
            inbox_id: String,
            warehouse_path: String,
            archive_enabled: bool,
            archive_sites: Vec<String>,
            total_files: usize,
            total_bytes: u64,
        }

        let payload = ArchivePayload {
            inbox_id: inbox
                .id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            warehouse_path: inbox.warehouse_path.clone(),
            archive_enabled: inbox.archive_enabled,
            archive_sites: inbox.archive_sites.clone(),
            total_files: transfer.files_transferred.len(),
            total_bytes: transfer.total_bytes,
        };

        let response = self
            .client
            .post(&self.lta_url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| AppError::BadGateway(format!("Request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(AppError::BadGateway(format!(
                "LTA returned status: {}",
                response.status()
            )));
        }

        info!(
            "LTA notification sent for inbox {} ({} files, {} bytes)",
            inbox.id.map(|id| id.to_string()).unwrap_or_default(),
            transfer.files_transferred.len(),
            transfer.total_bytes
        );

        Ok(())
    }
}

#[async_trait::async_trait]
impl LtaNotifier for HttpLtaNotifier {
    async fn notify(&self, inbox: &InboxDB, transfer: &TransferResult) -> Result<()> {
        self.notify_with_retry(inbox, transfer).await
    }

    async fn retry(&self, notification_id: &str) -> Result<()> {
        warn!("LTA retry requested for notification: {}", notification_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_noop_lta_notifier() {
        let notifier = NoOpLtaNotifier;
        let inbox = InboxDB {
            id: None,
            name: "test".to_string(),
            size: 0,
            status: common::models::InboxStatus::Active,
            warehouse_path: "/data/test".to_string(),
            inbox_path: Some("/inboxes/test".to_string()),
            lifetime: None,
            archive_enabled: true,
            archive_sites: vec!["site-1".to_string()],
            version: 0,
            current_job_id: None,
            progress: None,
            error: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let transfer = TransferResult {
            success: true,
            files_transferred: vec![],
            files_failed: vec![],
            total_bytes: 0,
            partial_cleanup_completed: true,
        };

        assert!(notifier.notify(&inbox, &transfer).await.is_ok());
        assert!(notifier.retry("test-123").await.is_ok());
    }
}
