//! Barbican API client for state coordination
//!
//! Calls Barbican's action-based endpoints to transition inbox state.
//! Each method corresponds to a specific `{operation}/{action}` endpoint.

use crate::config::CitadelConfig;
use crate::error::{AppError, Result};
use common::models::InboxDB;
use mongodb::bson::oid::ObjectId;
use std::sync::Arc;
use tracing::{error, warn};

/// Trait abstracting the Barbican state-coordination endpoints.
///
/// Extracted from [`BarbicanClient`] so callers (notably the job callback
/// handler and the status poller) can be unit-tested against a mock without a
/// live HTTP client. All methods mirror the corresponding
/// `POST /api/v1/inbox/{id}/{operation}/{action}` Barbican endpoints and
/// return the updated `InboxDB`.
#[async_trait::async_trait]
#[cfg_attr(test, mockall::automock)]
// The progress-reporting methods take one arg per metric the endpoint needs.
#[allow(clippy::too_many_arguments)]
pub trait InboxTransitioner: Send + Sync {
    /// `create/start` — Active -> Creating
    async fn create_start(&self, inbox_id: &ObjectId, job_id: &str) -> Result<InboxDB>;
    /// `create/progress`
    async fn create_progress(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        phase: &str,
        transferred_bytes: u64,
    ) -> Result<InboxDB>;
    /// `create/complete` — Creating -> Ready
    async fn create_complete(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        inbox_path: &str,
    ) -> Result<InboxDB>;
    /// `create/failure` — Creating -> Failed
    async fn create_failure(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        error_code: &str,
        error_message: &str,
    ) -> Result<InboxDB>;
    /// `publish/start` — Ready -> Publishing
    async fn publish_start(&self, inbox_id: &ObjectId, job_id: &str) -> Result<InboxDB>;
    /// `publish/progress`
    async fn publish_progress(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        phase: &str,
        total_files: u64,
        processed_files: u64,
        total_bytes: u64,
        transferred_bytes: u64,
    ) -> Result<InboxDB>;
    /// `publish/complete` — Publishing -> Published
    async fn publish_complete(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        files_transferred: u64,
        total_bytes: u64,
        protected_path: &str,
    ) -> Result<InboxDB>;
    /// `publish/failure` — Publishing -> Failed
    async fn publish_failure(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        error_code: &str,
        error_message: &str,
    ) -> Result<InboxDB>;
    /// `delete/start` — Ready/Failed -> Deleting
    async fn delete_start(&self, inbox_id: &ObjectId, job_id: &str) -> Result<InboxDB>;
    /// `delete/progress`
    async fn delete_progress(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        phase: &str,
        total_bytes: u64,
        deleted_bytes: u64,
    ) -> Result<InboxDB>;
    /// `delete/complete` — Deleting -> Deleted
    async fn delete_complete(&self, inbox_id: &ObjectId, job_id: &str) -> Result<InboxDB>;
    /// `delete/failure` — Deleting -> Failed
    async fn delete_failure(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        error_code: &str,
        error_message: &str,
    ) -> Result<InboxDB>;
}

/// Barbican API client
pub struct BarbicanClient {
    config: CitadelConfig,
    client: reqwest::Client,
    /// Optional metrics handle (set via [`BarbicanClient::with_metrics`]).
    metrics: Option<Arc<crate::metrics::CitadelMetrics>>,
    /// Circuit-breaker state (P4.4): `0 = closed`, `1 = open`.
    breaker_open: std::sync::atomic::AtomicU8,
    /// Consecutive `post_action` failures while closed.
    breaker_failures: std::sync::atomic::AtomicU32,
    /// When the breaker was tripped (used to auto half-open).
    ///
    /// A `std::sync::Mutex`, not tokio's: the critical sections are tiny and
    /// never `.await`, and these helpers are called from async code, where
    /// `tokio::sync::Mutex::blocking_lock` panics.
    breaker_opened_at: std::sync::Mutex<Option<std::time::Instant>>,
    /// Source of the Bearer token Barbican's internal routes require (set via
    /// [`BarbicanClient::with_token_provider`]). `None` sends no
    /// `Authorization` header (local/mock setups only).
    token_provider: Option<Arc<common::ServiceTokenProvider>>,
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

#[async_trait::async_trait]
impl<T: InboxTransitioner + Send + Sync> InboxTransitioner for Arc<T> {
    async fn create_start(&self, inbox_id: &ObjectId, job_id: &str) -> Result<InboxDB> {
        (**self).create_start(inbox_id, job_id).await
    }
    async fn create_progress(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        phase: &str,
        transferred_bytes: u64,
    ) -> Result<InboxDB> {
        (**self)
            .create_progress(inbox_id, job_id, phase, transferred_bytes)
            .await
    }
    async fn create_complete(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        inbox_path: &str,
    ) -> Result<InboxDB> {
        (**self).create_complete(inbox_id, job_id, inbox_path).await
    }
    async fn create_failure(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        error_code: &str,
        error_message: &str,
    ) -> Result<InboxDB> {
        (**self)
            .create_failure(inbox_id, job_id, error_code, error_message)
            .await
    }
    async fn publish_start(&self, inbox_id: &ObjectId, job_id: &str) -> Result<InboxDB> {
        (**self).publish_start(inbox_id, job_id).await
    }
    async fn publish_progress(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        phase: &str,
        total_files: u64,
        processed_files: u64,
        total_bytes: u64,
        transferred_bytes: u64,
    ) -> Result<InboxDB> {
        (**self)
            .publish_progress(
                inbox_id,
                job_id,
                phase,
                total_files,
                processed_files,
                total_bytes,
                transferred_bytes,
            )
            .await
    }
    async fn publish_complete(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        files_transferred: u64,
        total_bytes: u64,
        protected_path: &str,
    ) -> Result<InboxDB> {
        (**self)
            .publish_complete(
                inbox_id,
                job_id,
                files_transferred,
                total_bytes,
                protected_path,
            )
            .await
    }
    async fn publish_failure(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        error_code: &str,
        error_message: &str,
    ) -> Result<InboxDB> {
        (**self)
            .publish_failure(inbox_id, job_id, error_code, error_message)
            .await
    }
    async fn delete_start(&self, inbox_id: &ObjectId, job_id: &str) -> Result<InboxDB> {
        (**self).delete_start(inbox_id, job_id).await
    }
    async fn delete_progress(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        phase: &str,
        total_bytes: u64,
        deleted_bytes: u64,
    ) -> Result<InboxDB> {
        (**self)
            .delete_progress(inbox_id, job_id, phase, total_bytes, deleted_bytes)
            .await
    }
    async fn delete_complete(&self, inbox_id: &ObjectId, job_id: &str) -> Result<InboxDB> {
        (**self).delete_complete(inbox_id, job_id).await
    }
    async fn delete_failure(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        error_code: &str,
        error_message: &str,
    ) -> Result<InboxDB> {
        (**self)
            .delete_failure(inbox_id, job_id, error_code, error_message)
            .await
    }
}

// ---------------------------------------------------------------------------
// InboxTransitioner impl for BarbicanClient
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl InboxTransitioner for BarbicanClient {
    async fn create_start(&self, inbox_id: &ObjectId, job_id: &str) -> Result<InboxDB> {
        self.create_start(inbox_id, job_id).await
    }
    async fn create_progress(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        phase: &str,
        transferred_bytes: u64,
    ) -> Result<InboxDB> {
        self.create_progress(inbox_id, job_id, phase, transferred_bytes)
            .await
    }
    async fn create_complete(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        inbox_path: &str,
    ) -> Result<InboxDB> {
        self.create_complete(inbox_id, job_id, inbox_path).await
    }
    async fn create_failure(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        error_code: &str,
        error_message: &str,
    ) -> Result<InboxDB> {
        self.create_failure(inbox_id, job_id, error_code, error_message)
            .await
    }
    async fn publish_start(&self, inbox_id: &ObjectId, job_id: &str) -> Result<InboxDB> {
        self.publish_start(inbox_id, job_id).await
    }
    async fn publish_progress(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        phase: &str,
        total_files: u64,
        processed_files: u64,
        total_bytes: u64,
        transferred_bytes: u64,
    ) -> Result<InboxDB> {
        self.publish_progress(
            inbox_id,
            job_id,
            phase,
            total_files,
            processed_files,
            total_bytes,
            transferred_bytes,
        )
        .await
    }
    async fn publish_complete(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        files_transferred: u64,
        total_bytes: u64,
        protected_path: &str,
    ) -> Result<InboxDB> {
        self.publish_complete(
            inbox_id,
            job_id,
            files_transferred,
            total_bytes,
            protected_path,
        )
        .await
    }
    async fn publish_failure(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        error_code: &str,
        error_message: &str,
    ) -> Result<InboxDB> {
        self.publish_failure(inbox_id, job_id, error_code, error_message)
            .await
    }
    async fn delete_start(&self, inbox_id: &ObjectId, job_id: &str) -> Result<InboxDB> {
        self.delete_start(inbox_id, job_id).await
    }
    async fn delete_progress(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        phase: &str,
        total_bytes: u64,
        deleted_bytes: u64,
    ) -> Result<InboxDB> {
        self.delete_progress(inbox_id, job_id, phase, total_bytes, deleted_bytes)
            .await
    }
    async fn delete_complete(&self, inbox_id: &ObjectId, job_id: &str) -> Result<InboxDB> {
        self.delete_complete(inbox_id, job_id).await
    }
    async fn delete_failure(
        &self,
        inbox_id: &ObjectId,
        job_id: &str,
        error_code: &str,
        error_message: &str,
    ) -> Result<InboxDB> {
        self.delete_failure(inbox_id, job_id, error_code, error_message)
            .await
    }
}

// ---------------------------------------------------------------------------
// BarbicanClient
// ---------------------------------------------------------------------------

impl BarbicanClient {
    pub fn new(config: CitadelConfig) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(
                    config.barbican_api_timeout_secs,
                ))
                .build()
                .unwrap_or_default(),
            config,
            metrics: None,
            breaker_open: std::sync::atomic::AtomicU8::new(0),
            breaker_failures: std::sync::atomic::AtomicU32::new(0),
            breaker_opened_at: std::sync::Mutex::new(None),
            token_provider: None,
        }
    }

    /// Attach the service-token provider used to authenticate to Barbican
    /// (`CITADEL_BARBICAN_CLIENT_ID` / `CITADEL_BARBICAN_CLIENT_SECRET`).
    pub fn with_token_provider(mut self, provider: Arc<common::ServiceTokenProvider>) -> Self {
        self.token_provider = Some(provider);
        self
    }

    /// Attach a metrics handle so the client can meter Barbican API errors and
    /// the circuit-breaker gauge.
    pub fn with_metrics(mut self, metrics: Arc<crate::metrics::CitadelMetrics>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// How long the breaker stays open before auto half-opening (seconds).
    const HALF_OPEN_AFTER_SECS: u64 = 30;

    /// Lock the trip timestamp, recovering from poisoning (the value is a
    /// plain `Option<Instant>`, so a panicked holder cannot leave it invalid).
    fn lock_opened_at(&self) -> std::sync::MutexGuard<'_, Option<std::time::Instant>> {
        self.breaker_opened_at
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn is_breaker_open(&self) -> bool {
        if self.breaker_open.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            return false;
        }
        // Auto half-open after the cooldown window.
        let opened = self.lock_opened_at();
        if let Some(opened_at) = *opened
            && opened_at.elapsed().as_secs() >= Self::HALF_OPEN_AFTER_SECS
        {
            return false;
        }
        true
    }

    fn trip_breaker(&self) {
        self.breaker_open
            .store(1, std::sync::atomic::Ordering::SeqCst);
        *self.lock_opened_at() = Some(std::time::Instant::now());
        if let Some(m) = self.metrics.as_ref() {
            m.circuit_breaker_state.set(1);
        }
        warn!("Barbican circuit breaker opened");
    }

    fn close_breaker(&self) {
        self.breaker_failures
            .store(0, std::sync::atomic::Ordering::SeqCst);
        self.breaker_open
            .store(0, std::sync::atomic::Ordering::SeqCst);
        *self.lock_opened_at() = None;
        if let Some(m) = self.metrics.as_ref() {
            m.circuit_breaker_state.set(0);
        }
    }

    fn record_failure(&self, label: &str, err: &AppError) {
        if let Some(m) = self.metrics.as_ref() {
            m.barbican_api_errors_total.inc();
        }
        let failures = self
            .breaker_failures
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        // A 409 is an expected/benign state race — don't trip the breaker on it.
        if !matches!(err, AppError::Conflict(_))
            && failures >= self.config.barbican_api_retry_max.max(1)
        {
            self.trip_breaker();
        }
        error!("Barbican {} failed: {}", label, err);
    }

    fn record_success(&self) {
        self.close_breaker();
    }

    // -----------------------------------------------------------------------
    // Helper: POST with retries
    // -----------------------------------------------------------------------

    /// POST a body to a Barbican action endpoint with exponential backoff.
    /// `Conflict` errors fail immediately (no retry, no breaker trip). When
    /// the breaker is open, calls fail fast with `ServiceUnavailable`.
    async fn post_action(
        &self,
        url: &str,
        body: &impl serde::Serialize,
        label: &str,
    ) -> Result<InboxDB> {
        // Fail fast while the breaker is open.
        if self.is_breaker_open() {
            warn!("Barbican circuit breaker open; failing fast for {}", label);
            return Err(AppError::ServiceUnavailable(
                "Barbican circuit breaker open".to_string(),
            ));
        }

        let mut retries = 0;
        let mut backoff_ms = self.config.barbican_api_retry_backoff_ms;

        loop {
            match self.do_post(url, body).await {
                Ok(inbox) => {
                    self.record_success();
                    return Ok(inbox);
                }
                Err(e) => {
                    retries += 1;
                    if retries > self.config.barbican_api_retry_max {
                        self.record_failure(label, &e);
                        return Err(e);
                    }

                    if matches!(e, AppError::Conflict(_)) {
                        // Benign state race — record error but do not trip.
                        if let Some(m) = self.metrics.as_ref() {
                            m.barbican_api_errors_total.inc();
                        }
                        return Err(e);
                    }

                    warn!(
                        "Barbican {} retry {} after {}ms: {}",
                        label, retries, backoff_ms, e
                    );
                    tokio::time::sleep(tokio::time::Duration::from_millis(backoff_ms)).await;
                    backoff_ms = (backoff_ms * 2).min(30000);
                }
            }
        }
    }

    /// Send one POST, attaching the service token when configured.
    async fn send_once(
        &self,
        url: &str,
        body: &impl serde::Serialize,
    ) -> Result<reqwest::Response> {
        let mut request = self.client.post(url).json(body);
        if let Some(provider) = &self.token_provider {
            let token = provider.bearer().await.map_err(|e| {
                AppError::ServiceUnavailable(format!("Barbican service token unavailable: {e}"))
            })?;
            request = request.bearer_auth(token);
        }
        request
            .send()
            .await
            .map_err(|e| AppError::BadGateway(format!("Request failed: {}", e)))
    }

    /// POST to Barbican. A 401 means our cached token was rejected (expired
    /// or revoked): invalidate it and retry once with a fresh token. This
    /// inner retry is not a `post_action` retry and does not count toward the
    /// circuit breaker.
    async fn do_post(&self, url: &str, body: &impl serde::Serialize) -> Result<InboxDB> {
        let mut response = self.send_once(url, body).await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            && let Some(provider) = &self.token_provider
        {
            warn!("Barbican rejected the service token (401); refreshing and retrying once");
            provider.invalidate().await;
            response = self.send_once(url, body).await?;
        }

        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(AppError::BadGateway(
                "Barbican rejected Citadel's service token (401); check \
                 CITADEL_BARBICAN_CLIENT_ID/SECRET and the Keycloak audience mapper"
                    .to_string(),
            ));
        }

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
    // CREATE actions
    // -----------------------------------------------------------------------

    /// `POST /api/v1/inbox/{id}/create/start`
    pub async fn create_start(&self, inbox_id: &ObjectId, job_id: &str) -> Result<InboxDB> {
        let url = format!(
            "{}/api/v1/inbox/{}/create/start",
            self.config.barbican_api_url, inbox_id
        );
        self.post_action(
            &url,
            &StartRequest {
                job_id: job_id.into(),
            },
            "create_start",
        )
        .await
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
        self.post_action(
            &url,
            &StartRequest {
                job_id: job_id.into(),
            },
            "publish_start",
        )
        .await
    }

    /// `POST /api/v1/inbox/{id}/publish/progress`
    #[allow(clippy::too_many_arguments)]
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
        self.post_action(
            &url,
            &StartRequest {
                job_id: job_id.into(),
            },
            "delete_start",
        )
        .await
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
            &StartRequest {
                job_id: job_id.into(),
            },
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

#[cfg(test)]
mod breaker_tests {
    use super::*;

    /// Regression: the breaker helpers run inside async request handlers.
    /// With `tokio::sync::Mutex::blocking_lock` they panicked ("Cannot block
    /// the current thread from within a runtime") on every successful call.
    #[tokio::test]
    async fn test_breaker_helpers_do_not_panic_in_async_context() {
        let client = BarbicanClient::new(CitadelConfig::default());
        assert!(!client.is_breaker_open());
        client.trip_breaker();
        assert!(client.is_breaker_open());
        client.close_breaker();
        assert!(!client.is_breaker_open());
    }
}
