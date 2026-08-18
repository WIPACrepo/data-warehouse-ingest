//! Barbican API client for state coordination
//!
//! Calls Barbican's action-based endpoints to transition inbox state.
//! Each method corresponds to a specific `{operation}/{action}` endpoint.

use crate::config::CitadelConfig;
use crate::error::{AppError, Result};
use common::models::InboxDB;
use mongodb::bson::oid::ObjectId;
use tracing::{error, warn};

/// Barbican API client
pub struct BarbicanClient {
    config: CitadelConfig,
    client: reqwest::Client,
}

// ---------------------------------------------------------------------------
// Request bodies
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Serialize)]
struct StartRequest {
    job_id: String,
}

#[derive(Debug, serde::Serialize)]
struct ProgressRequest {
    job_id: String,
    phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_files: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    processed_files: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transferred_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deleted_bytes: Option<u64>,
}

#[derive(Debug, serde::Serialize)]
struct CreateCompleteRequest {
    job_id: String,
    inbox_path: String,
}

#[derive(Debug, serde::Serialize)]
struct FailureRequest {
    job_id: String,
    error_code: String,
    error_message: String,
}

#[derive(Debug, serde::Serialize)]
struct PublishCompleteRequest {
    job_id: String,
    files_transferred: u64,
    total_bytes: u64,
    protected_path: String,
}

// ---------------------------------------------------------------------------
// BarbicanClient
// ---------------------------------------------------------------------------

impl BarbicanClient {
    pub fn new(config: CitadelConfig) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(config.barbican_api_timeout_secs))
                .build()
                .unwrap_or_default(),
            config,
        }
    }

    // -----------------------------------------------------------------------
    // Helper: POST with retries
    // -----------------------------------------------------------------------

    /// POST a body to a Barbican action endpoint with exponential backoff.
    /// `BarbicanConflict` errors fail immediately (no retry).
    async fn post_action(
        &self,
        url: &str,
        body: &impl serde::Serialize,
        label: &str,
    ) -> Result<InboxDB> {
        let mut retries = 0;
        let mut backoff_ms = self.config.barbican_api_retry_backoff_ms;

        loop {
            match self.do_post(url, body).await {
                Ok(inbox) => return Ok(inbox),
                Err(e) => {
                    retries += 1;
                    if retries > self.config.barbican_api_retry_max {
                        error!("Barbican {} failed after {} retries: {}", label, retries, e);
                        return Err(e);
                    }

                    if matches!(e, AppError::Conflict(_)) {
                        return Err(e);
                    }

                    warn!("Barbican {} retry {} after {}ms: {}", label, retries, backoff_ms, e);
                    tokio::time::sleep(tokio::time::Duration::from_millis(backoff_ms)).await;
                    backoff_ms = (backoff_ms * 2).min(30000);
                }
            }
        }
    }

    async fn do_post(
        &self,
        url: &str,
        body: &impl serde::Serialize,
    ) -> Result<InboxDB> {
        let response = self
            .client
            .post(url)
            .json(body)
            .send()
            .await
            .map_err(|e| AppError::BadGateway(format!("Request failed: {}", e)))?;

        if response.status() == 409 {
            return Err(AppError::Conflict("Inbox state conflict".to_string()));
        }

        if !response.status().is_success() {
            return Err(AppError::BadGateway(format!(
                "Barbican returned status {}",
                response.status()
            )));
        }

        response
            .json()
            .await
            .map_err(|e| AppError::BadGateway(format!("Failed to parse response: {}", e)))
    }

    // -----------------------------------------------------------------------
    // GET
    // -----------------------------------------------------------------------

    /// Get inbox metadata from Barbican
    pub async fn get_inbox(&self, inbox_id: &ObjectId) -> Result<InboxDB> {
        let url = format!("{}/api/v1/inbox/{}", self.config.barbican_api_url, inbox_id);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| AppError::BadGateway(format!("Request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(AppError::BadGateway(format!(
                "Barbican returned status {}",
                response.status()
            )));
        }

        response
            .json()
            .await
            .map_err(|e| AppError::BadGateway(format!("Failed to parse response: {}", e)))
    }

    // -----------------------------------------------------------------------
    // CREATE actions
    // -----------------------------------------------------------------------

    /// `POST /api/v1/inbox/{id}/create/start`
    pub async fn create_start(&self, inbox_id: &ObjectId, job_id: &str) -> Result<InboxDB> {
        let url = format!(
            "{}/api/v1/inbox/{}/create/start",
            self.config.barbican_api_url, inbox_id
        );
        self.post_action(&url, &StartRequest { job_id: job_id.into() }, "create_start").await
    }

    /// `POST /api/v1/inbox/{id}/create/progress`
    pub async fn create_progress(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        phase: &str,
        transferred_bytes: u64,
    ) -> Result<InboxDB> {
        let url = format!(
            "{}/api/v1/inbox/{}/create/progress",
            self.config.barbican_api_url, inbox_id
        );
        self.post_action(
            &url,
            &ProgressRequest {
                job_id: job_id.into(),
                phase: phase.into(),
                total_files: None,
                processed_files: None,
                total_bytes: None,
                transferred_bytes: Some(transferred_bytes),
                deleted_bytes: None,
            },
            "create_progress",
        )
        .await
    }

    /// `POST /api/v1/inbox/{id}/create/complete`
    pub async fn create_complete(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        inbox_path: &str,
    ) -> Result<InboxDB> {
        let url = format!(
            "{}/api/v1/inbox/{}/create/complete",
            self.config.barbican_api_url, inbox_id
        );
        self.post_action(
            &url,
            &CreateCompleteRequest {
                job_id: job_id.into(),
                inbox_path: inbox_path.into(),
            },
            "create_complete",
        )
        .await
    }

    /// `POST /api/v1/inbox/{id}/create/failure`
    pub async fn create_failure(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        error_code: &str,
        error_message: &str,
    ) -> Result<InboxDB> {
        let url = format!(
            "{}/api/v1/inbox/{}/create/failure",
            self.config.barbican_api_url, inbox_id
        );
        self.post_action(
            &url,
            &FailureRequest {
                job_id: job_id.into(),
                error_code: error_code.into(),
                error_message: error_message.into(),
            },
            "create_failure",
        )
        .await
    }

    // -----------------------------------------------------------------------
    // PUBLISH actions
    // -----------------------------------------------------------------------

    /// `POST /api/v1/inbox/{id}/publish/start`
    pub async fn publish_start(&self, inbox_id: &ObjectId, job_id: &str) -> Result<InboxDB> {
        let url = format!(
            "{}/api/v1/inbox/{}/publish/start",
            self.config.barbican_api_url, inbox_id
        );
        self.post_action(&url, &StartRequest { job_id: job_id.into() }, "publish_start").await
    }

    /// `POST /api/v1/inbox/{id}/publish/progress`
    pub async fn publish_progress(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        phase: &str,
        total_files: u64,
        processed_files: u64,
        total_bytes: u64,
        transferred_bytes: u64,
    ) -> Result<InboxDB> {
        let url = format!(
            "{}/api/v1/inbox/{}/publish/progress",
            self.config.barbican_api_url, inbox_id
        );
        self.post_action(
            &url,
            &ProgressRequest {
                job_id: job_id.into(),
                phase: phase.into(),
                total_files: Some(total_files),
                processed_files: Some(processed_files),
                total_bytes: Some(total_bytes),
                transferred_bytes: Some(transferred_bytes),
                deleted_bytes: None,
            },
            "publish_progress",
        )
        .await
    }

    /// `POST /api/v1/inbox/{id}/publish/complete`
    pub async fn publish_complete(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        files_transferred: u64,
        total_bytes: u64,
        protected_path: &str,
    ) -> Result<InboxDB> {
        let url = format!(
            "{}/api/v1/inbox/{}/publish/complete",
            self.config.barbican_api_url, inbox_id
        );
        self.post_action(
            &url,
            &PublishCompleteRequest {
                job_id: job_id.into(),
                files_transferred,
                total_bytes,
                protected_path: protected_path.into(),
            },
            "publish_complete",
        )
        .await
    }

    /// `POST /api/v1/inbox/{id}/publish/failure`
    pub async fn publish_failure(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        error_code: &str,
        error_message: &str,
    ) -> Result<InboxDB> {
        let url = format!(
            "{}/api/v1/inbox/{}/publish/failure",
            self.config.barbican_api_url, inbox_id
        );
        self.post_action(
            &url,
            &FailureRequest {
                job_id: job_id.into(),
                error_code: error_code.into(),
                error_message: error_message.into(),
            },
            "publish_failure",
        )
        .await
    }

    // -----------------------------------------------------------------------
    // DELETE actions
    // -----------------------------------------------------------------------

    /// `POST /api/v1/inbox/{id}/delete/start`
    pub async fn delete_start(&self, inbox_id: &ObjectId, job_id: &str) -> Result<InboxDB> {
        let url = format!(
            "{}/api/v1/inbox/{}/delete/start",
            self.config.barbican_api_url, inbox_id
        );
        self.post_action(&url, &StartRequest { job_id: job_id.into() }, "delete_start").await
    }

    /// `POST /api/v1/inbox/{id}/delete/progress`
    pub async fn delete_progress(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        phase: &str,
        total_bytes: u64,
        deleted_bytes: u64,
    ) -> Result<InboxDB> {
        let url = format!(
            "{}/api/v1/inbox/{}/delete/progress",
            self.config.barbican_api_url, inbox_id
        );
        self.post_action(
            &url,
            &ProgressRequest {
                job_id: job_id.into(),
                phase: phase.into(),
                total_files: None,
                processed_files: None,
                total_bytes: Some(total_bytes),
                transferred_bytes: None,
                deleted_bytes: Some(deleted_bytes),
            },
            "delete_progress",
        )
        .await
    }

    /// `POST /api/v1/inbox/{id}/delete/complete`
    pub async fn delete_complete(&self, inbox_id: &ObjectId, job_id: &str) -> Result<InboxDB> {
        let url = format!(
            "{}/api/v1/inbox/{}/delete/complete",
            self.config.barbican_api_url, inbox_id
        );
        self.post_action(
            &url,
            &StartRequest { job_id: job_id.into() },
            "delete_complete",
        )
        .await
    }

    /// `POST /api/v1/inbox/{id}/delete/failure`
    pub async fn delete_failure(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        error_code: &str,
        error_message: &str,
    ) -> Result<InboxDB> {
        let url = format!(
            "{}/api/v1/inbox/{}/delete/failure",
            self.config.barbican_api_url, inbox_id
        );
        self.post_action(
            &url,
            &FailureRequest {
                job_id: job_id.into(),
                error_code: error_code.into(),
                error_message: error_message.into(),
            },
            "delete_failure",
        )
        .await
    }
}