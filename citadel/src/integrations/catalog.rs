//! File Catalog notification integration
//!
//! Notifies the File Catalog service about newly controlled files.
//! Uses HTTP with exponential backoff retry logic.

use crate::config::CitadelConfig;
use crate::error::{AppError, Result};
use common::models::InboxDB;
use crate::models::TransferResult;
use tracing::{info, warn, error};

/// Catalog notifier trait
#[async_trait::async_trait]
pub trait CatalogNotifier: Send + Sync {
    async fn notify(&self, inbox: &InboxDB, transfer: &TransferResult) -> Result<()>;
    async fn retry(&self, notification_id: &str) -> Result<()>;
}

/// No-op catalog notifier for testing
pub struct NoOpCatalogNotifier;

#[async_trait::async_trait]
impl CatalogNotifier for NoOpCatalogNotifier {
    async fn notify(&self, _inbox: &InboxDB, _transfer: &TransferResult) -> Result<()> {
        info!("NoOp catalog notification");
        Ok(())
    }

    async fn retry(&self, _notification_id: &str) -> Result<()> {
        info!("NoOp catalog retry");
        Ok(())
    }
}

/// HTTP-based catalog notifier
pub struct HttpCatalogNotifier {
    config: CitadelConfig,
    client: reqwest::Client,
    catalog_url: String,
}

impl HttpCatalogNotifier {
    pub fn new(config: CitadelConfig, catalog_url: Option<String>) -> Self {
        Self {
            config: config.clone(),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(config.catalog_notify_timeout_secs))
                .build()
                .unwrap_or_default(),
            catalog_url: catalog_url.unwrap_or_else(|| "http://catalog:8080/api/v1/notify".to_string()),
        }
    }

    /// Send notification with exponential backoff retry
    async fn notify_with_retry(&self, inbox: &InboxDB, transfer: &TransferResult) -> Result<()> {
        let max_retries = self.config.catalog_notify_retry_max;
        let mut backoff_ms = self.config.catalog_notify_retry_backoff_ms;

        for attempt in 0..max_retries {
            match self.do_notify(inbox, transfer).await {
                Ok(()) => {
                    if attempt > 0 {
                        info!("Catalog notification succeeded on attempt {}", attempt + 1);
                    }
                    return Ok(());
                }
                Err(e) => {
                    warn!(
                        "Catalog notification failed on attempt {}/{}: {}",
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

        error!(
            "Catalog notification failed after {} retries",
            max_retries
        );
        Err(AppError::BadGateway(
            "Failed after all retries".to_string(),
        ))
    }

    /// Perform a single notification request
    async fn do_notify(&self, inbox: &InboxDB, transfer: &TransferResult) -> Result<()> {
        #[derive(serde::Serialize)]
        struct NotifyPayload {
            inbox_id: String,
            warehouse_path: String,
            files_transferred: Vec<serde_json::Value>,
            total_bytes: u64,
            success: bool,
        }

        let payload = NotifyPayload {
            inbox_id: inbox
                .id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            warehouse_path: inbox.warehouse_path.clone(),
            files_transferred: transfer
                .files_transferred
                .iter()
                .map(|f| {
                    serde_json::json!({
                        "path": f.path,
                        "size": f.size,
                        "checksum": f.checksum,
                    })
                })
                .collect(),
            total_bytes: transfer.total_bytes,
            success: transfer.success,
        };

        let response = self
            .client
            .post(&self.catalog_url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| AppError::BadGateway(format!("Request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(AppError::BadGateway(format!(
                "Catalog returned status: {}",
                response.status()
            )));
        }

        info!(
            "Catalog notification sent for inbox {} ({} files, {} bytes)",
            inbox.id.map(|id| id.to_string()).unwrap_or_default(),
            transfer.files_transferred.len(),
            transfer.total_bytes
        );

        Ok(())
    }
}

#[async_trait::async_trait]
impl CatalogNotifier for HttpCatalogNotifier {
    async fn notify(&self, inbox: &InboxDB, transfer: &TransferResult) -> Result<()> {
        self.notify_with_retry(inbox, transfer).await
    }

    async fn retry(&self, notification_id: &str) -> Result<()> {
        warn!("Catalog retry requested for notification: {}", notification_id);
        // In a full implementation, this would look up the pending notification
        // and re-send it. For now, we log the request.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_noop_catalog_notifier() {
        let notifier = NoOpCatalogNotifier;
        let inbox = InboxDB {
            id: None,
            name: "test".to_string(),
            size: 0,
            status: common::models::InboxStatus::Active,
            warehouse_path: "/data/test".to_string(),
            inbox_path: Some("/inboxes/test".to_string()),
            lifetime: None,
            archive_enabled: false,
            archive_sites: vec![],
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