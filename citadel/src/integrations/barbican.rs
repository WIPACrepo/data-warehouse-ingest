//! Barbican API client for state coordination

use crate::config::CitadelConfig;
use crate::error::{CitadelError, Result};
use common::models::InboxDB;
use mongodb::bson::oid::ObjectId;
use tracing::{error, info, warn};

/// Barbican API client
pub struct BarbicanClient {
    config: CitadelConfig,
    client: reqwest::Client,
}

impl BarbicanClient {
    pub fn new(config: CitadelConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            config,
        }
    }
    
    /// Transition inbox state via Barbican API
    pub async fn transition_inbox_state(
        &self,
        inbox_id: &ObjectId,
        new_state: &str,
    ) -> Result<InboxDB> {
        let url = format!(
            "{}/api/v1/inbox/{}/transition",
            self.config.barbican_api_url, inbox_id
        );
        
        let mut retries = 0;
        let mut backoff_ms = self.config.barbican_api_retry_backoff_ms;
        
        loop {
            match self.do_transition(&url, inbox_id, new_state).await {
                Ok(inbox) => return Ok(inbox),
                Err(e) => {
                    retries += 1;
                    if retries > self.config.barbican_api_retry_max {
                        error!("Barbican API failed after {} retries: {}", retries, e);
                        return Err(e);
                    }
                    
                    if matches!(e, CitadelError::BarbicanConflict(_)) {
                        // Conflict errors should fail immediately
                        return Err(e);
                    }
                    
                    warn!("Barbican API retry {} after {}ms: {}", retries, backoff_ms, e);
                    tokio::time::sleep(tokio::time::Duration::from_millis(backoff_ms)).await;
                    backoff_ms = (backoff_ms * 2).min(30000); // Exponential backoff, max 30s
                }
            }
        }
    }
    
    async fn do_transition(
        &self,
        url: &str,
        inbox_id: &ObjectId,
        new_state: &str,
    ) -> Result<InboxDB> {
        let response = self.client
            .post(url)
            .send()
            .await
            .map_err(|e| CitadelError::BarbicanApiError(format!("Request failed: {}", e)))?;
        
        if response.status() == 409 {
            return Err(CitadelError::BarbicanConflict(format!(
                "Inbox {} state conflict",
                inbox_id
            )));
        }
        
        if !response.status().is_success() {
            return Err(CitadelError::BarbicanApiError(format!(
                "Barbican API returned status: {}",
                response.status()
            )));
        }
        
        let inbox: InboxDB = response
            .json()
            .await
            .map_err(|e| CitadelError::BarbicanApiError(format!("Failed to parse response: {}", e)))?;
        
        info!("Successfully transitioned inbox {} to {}", inbox_id, new_state);
        Ok(inbox)
    }
    
    /// Get inbox metadata from Barbican
    pub async fn get_inbox(&self, inbox_id: &ObjectId) -> Result<InboxDB> {
        let url = format!("{}/api/v1/inbox/{}", self.config.barbican_api_url, inbox_id);
        
        let response = self.client
            .get(&url)
            .send()
            .await
            .map_err(|e| CitadelError::BarbicanApiError(format!("Request failed: {}", e)))?;
        
        if !response.status().is_success() {
            return Err(CitadelError::BarbicanApiError(format!(
                "Barbican API returned status: {}",
                response.status()
            )));
        }
        
        let inbox: InboxDB = response
            .json()
            .await
            .map_err(|e| CitadelError::BarbicanApiError(format!("Failed to parse response: {}", e)))?;
        
        Ok(inbox)
    }
}