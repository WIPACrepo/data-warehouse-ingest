use reqwest::Client;
use serde::Serialize;
use std::time::Duration;
use tracing::{error, info, warn};

use crate::config::Config;

/// Citadel API client for triggering inbox operations
#[derive(Clone)]
pub struct CitadelClient {
    client: Client,
    config: Config,
    service_token: Option<String>,
}

/// Response from Citadel operation start
#[derive(Debug, serde::Deserialize)]
pub struct StartOperationResponse {
    pub job_id: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
struct CreateRequestBody {
    warehouse_path: String,
}

/// Empty body for publish/delete
#[derive(Debug, Serialize)]
struct EmptyBody {}

impl CitadelClient {
    /// Create a new Citadel client from configuration
    pub fn new(config: &Config) -> Result<Self, CitadelError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.citadel_api_timeout_secs))
            .build()
            .map_err(|e| CitadelError::ClientCreation(e.to_string()))?;

        let service_token = if let Some(token_file) = &config.citadel_service_token_file {
            match std::fs::read_to_string(token_file) {
                Ok(token) => Some(token.trim().to_string()),
                Err(e) => {
                    warn!("Failed to read service token file {}: {}", token_file, e);
                    None
                }
            }
        } else {
            None
        };

        Ok(Self {
            client,
            config: config.clone(),
            service_token,
        })
    }

    /// Trigger a create operation in Citadel
    ///
    /// Calls `POST /api/v1/inbox/{id}/create` on Citadel.
    pub async fn trigger_create(
        &self,
        inbox_id: &str,
        warehouse_path: &str,
    ) -> Result<StartOperationResponse, CitadelError> {
        let url = format!("{}/api/v1/inbox/{}/create", self.config.citadel_api_url, inbox_id);
        self.call_with_retry(
            &url,
            &CreateRequestBody {
                warehouse_path: warehouse_path.into(),
            },
            "create",
        )
        .await
    }

    /// Trigger a delete operation in Citadel
    ///
    /// Calls `POST /api/v1/inbox/{id}/delete` on Citadel.
    pub async fn trigger_delete(
        &self,
        inbox_id: &str,
    ) -> Result<StartOperationResponse, CitadelError> {
        let url = format!("{}/api/v1/inbox/{}/delete", self.config.citadel_api_url, inbox_id);
        self.call_with_retry(&url, &EmptyBody {}, "delete").await
    }

    /// Trigger a publish operation in Citadel
    ///
    /// Calls `POST /api/v1/inbox/{id}/publish` on Citadel.
    pub async fn trigger_publish(
        &self,
        inbox_id: &str,
    ) -> Result<StartOperationResponse, CitadelError> {
        let url = format!("{}/api/v1/inbox/{}/publish", self.config.citadel_api_url, inbox_id);
        self.call_with_retry(&url, &EmptyBody {}, "publish").await
    }

    /// Generic helper: send a POST to Citadel with retry + backoff
    async fn call_with_retry(
        &self,
        url: &str,
        body: &impl Serialize,
        operation: &str,
    ) -> Result<StartOperationResponse, CitadelError> {
        let mut req_builder = self.client.post(url).json(body);

        if let Some(ref token) = self.service_token {
            req_builder = req_builder.bearer_auth(token);
        }

        let mut attempts = 0;
        loop {
            attempts += 1;

            let response = req_builder
                .try_clone()
                .ok_or_else(|| CitadelError::RequestFailed("Failed to clone request".to_string()))?
                .send()
                .await;

            match response {
                Ok(resp) if resp.status().is_success() => {
                    let result = resp
                        .json::<StartOperationResponse>()
                        .await
                        .map_err(|e| CitadelError::ParseResponse(e.to_string()))?;

                    info!("Successfully triggered {} operation: job_id={}", operation, result.job_id);
                    return Ok(result);
                }
                Ok(resp) => {
                    let status = resp.status();
                    let error_body = resp.text().await.unwrap_or_default();
                    error!(
                        "Citadel API returned error status {}: {}",
                        status, error_body
                    );
                    return Err(CitadelError::ApiError(status, error_body));
                }
                Err(e) => {
                    if attempts >= self.config.citadel_api_retry_max {
                        error!(
                            "Failed to trigger {} operation after {} attempts: {}",
                            operation, attempts, e
                        );
                        return Err(CitadelError::RequestFailed(e.to_string()));
                    }
                    warn!(
                        "Attempt {} failed to trigger {} operation, retrying: {}",
                        attempts, operation, e
                    );
                    tokio::time::sleep(Duration::from_millis(
                        (1000 * 2u64.pow(attempts - 1)) as u64,
                    ))
                    .await;
                }
            }
        }
    }
}

/// Citadel client errors
#[derive(Debug, thiserror::Error)]
pub enum CitadelError {
    #[error("Failed to create HTTP client: {0}")]
    ClientCreation(String),

    #[error("Failed to send request: {0}")]
    RequestFailed(String),

    #[error("Failed to parse response: {0}")]
    ParseResponse(String),

    #[error("Citadel API error ({0}): {1}")]
    ApiError(reqwest::StatusCode, String),

    #[error("Configuration error: {0}")]
    ConfigError(String),
}