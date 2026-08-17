//! Long Term Archive (LTA) notification integration

use crate::config::CitadelConfig;
use crate::error::Result;
use common::models::InboxDB;
use crate::models::TransferResult;
use tracing::info;

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