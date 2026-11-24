use reqwest::Client;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;

use common::ServiceTokenProvider;
use tracing::{error, info, warn};

use crate::config::Config;

/// Citadel API client for triggering inbox operations
#[derive(Clone)]
pub struct CitadelClient {
    client: Client,
    config: Config,
    /// Bearer token source for Citadel's role-gated routes
    /// (`BARBICAN_CITADEL_CLIENT_ID`/`_CLIENT_SECRET`, or the dev-only
    /// `BARBICAN_CITADEL_SERVICE_TOKEN_FILE`). `None` sends no header.
    token_provider: Option<Arc<ServiceTokenProvider>>,
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
    /// Create a new Citadel client from configuration.
    ///
    /// The token provider refreshes the service token before it expires (the
    /// old implementation read a token file once at startup, so calls failed
    /// with 401 as soon as that token expired).
    pub fn new(
        config: &Config,
        token_provider: Option<Arc<ServiceTokenProvider>>,
    ) -> Result<Self, CitadelError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.citadel_api_timeout_secs))
            .build()
            .map_err(|e| CitadelError::ClientCreation(e.to_string()))?;

        Ok(Self {
            client,
            config: config.clone(),
            token_provider,
        })
    }

    /// Build one request, attaching a current service token when configured.
    async fn build_request(
        &self,
        url: &str,
        body: &impl Serialize,
    ) -> Result<reqwest::RequestBuilder, CitadelError> {
        let mut req = self.client.post(url).json(body);
        if let Some(provider) = &self.token_provider {
            let token = provider.bearer().await.map_err(|e| {
                CitadelError::RequestFailed(format!("Citadel service token unavailable: {e}"))
            })?;
            req = req.bearer_auth(token);
        }
        Ok(req)
    }

    /// Trigger a create operation in Citadel
    ///
    /// Calls `POST /api/v1/inbox/{id}/create` on Citadel.
    pub async fn trigger_create(
        &self,
        inbox_id: &str,
        warehouse_path: &str,
    ) -> Result<StartOperationResponse, CitadelError> {
        let url = format!(
            "{}/api/v1/inbox/{}/create",
            self.config.citadel_api_url, inbox_id
        );
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
        let url = format!(
            "{}/api/v1/inbox/{}/delete",
            self.config.citadel_api_url, inbox_id
        );
        self.call_with_retry(&url, &EmptyBody {}, "delete").await
    }

    /// Trigger a publish operation in Citadel
    ///
    /// Calls `POST /api/v1/inbox/{id}/publish` on Citadel.
    pub async fn trigger_publish(
        &self,
        inbox_id: &str,
    ) -> Result<StartOperationResponse, CitadelError> {
        let url = format!(
            "{}/api/v1/inbox/{}/publish",
            self.config.citadel_api_url, inbox_id
        );
        self.call_with_retry(&url, &EmptyBody {}, "publish").await
    }

    /// Generic helper: send a POST to Citadel with retry + backoff
    async fn call_with_retry(
        &self,
        url: &str,
        body: &impl Serialize,
        operation: &str,
    ) -> Result<StartOperationResponse, CitadelError> {
        let mut attempts = 0;
        let mut refreshed_token = false;
        loop {
            attempts += 1;

            let response = self.build_request(url, body).await?.send().await;

            // A 401 means the cached token was rejected (expired/revoked):
            // refresh it and retry once. Not counted as a retry attempt.
            if let Ok(resp) = &response
                && resp.status() == reqwest::StatusCode::UNAUTHORIZED
                && !refreshed_token
                && let Some(provider) = &self.token_provider
            {
                warn!("Citadel rejected the service token (401); refreshing and retrying once");
                provider.invalidate().await;
                refreshed_token = true;
                attempts -= 1;
                continue;
            }

            match response {
                Ok(resp) if resp.status().is_success() => {
                    let result = resp
                        .json::<StartOperationResponse>()
                        .await
                        .map_err(|e| CitadelError::ParseResponse(e.to_string()))?;

                    info!(
                        "Successfully triggered {} operation: job_id={}",
                        operation, result.job_id
                    );
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
                    tokio::time::sleep(Duration::from_millis(1000 * 2u64.pow(attempts - 1))).await;
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
