//! File Catalog notification integration

use crate::config::CitadelConfig;
use crate::error::Result;
use common::models::InboxDB;
use crate::models::TransferResult;
use tracing::info;

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