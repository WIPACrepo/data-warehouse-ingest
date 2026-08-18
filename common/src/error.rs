//! Application-level error types and helpers

use axum::{
    extract::FromRequest,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use thiserror::Error;

// ---------------------------------------------------------------------------
// AppJson — custom JSON extractor/response wrapper
// ---------------------------------------------------------------------------

// Create our own JSON extractor by wrapping `axum::Json`. This makes it easy to override the
// rejection and provide our own which formats errors to match our application.
//
// `axum::Json` responds with plain text if the input is invalid.
#[derive(FromRequest)]
#[from_request(via(axum::Json), rejection(AppError))]
pub struct AppJson<T>(pub T);

impl<T> AppJson<T> {
    /// Creates a new AppJson wrapper
    pub fn new(value: T) -> Self {
        AppJson(value)
    }
}

impl<T> IntoResponse for AppJson<T>
where
    axum::Json<T>: IntoResponse,
{
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

// ---------------------------------------------------------------------------
// AppError
// ---------------------------------------------------------------------------

/// The kinds of errors we can have in our application.
#[derive(Debug, Error)]
pub enum AppError {
    // -- HTTP-level rejections --
    #[error("Invalid JSON: {0}")]
    JsonRejection(axum::extract::rejection::JsonRejection),

    // -- Inbox / domain errors --
    #[error("Inbox not found: {0}")]
    InboxNotFound(String),

    #[error("Inbox cannot be deleted: {0}")]
    InboxNotDeletable(String),

    #[error("Inbox already publishing: {0}")]
    InboxAlreadyPublishing(String),

    #[error("Invalid publish parameters: {0}")]
    InvalidPublishParameters(String),

    #[error("Invalid state transition: {0}")]
    InvalidStateTransition(String),

    #[error("Validation error: {0}")]
    ValidationError(String),

    #[error("Conflict: {0}")]
    Conflict(String),

    // -- Job errors --
    #[error("Job not found: {0}")]
    JobNotFound(String),

    #[error("Job timeout: {0}")]
    JobTimeout(String),

    // -- Callback errors --
    #[error("Invalid callback secret")]
    InvalidCallbackSecret,

    #[error("Callback timestamp expired")]
    CallbackTimestampExpired,

    #[error("Callback nonce already used")]
    CallbackNonceReused,

    // -- Storage errors --
    #[error("Storage error: {0}")]
    StorageError(String),

    #[error("File not found: {0}")]
    FileNotFound(String),

    #[error("File transfer failed: {0}")]
    FileTransferFailed(String),

    #[error("Checksum mismatch: {0}")]
    ChecksumMismatch(String),

    // -- Kubernetes errors --
    #[error("Kubernetes error: {0}")]
    KubernetesError(String),

    #[error("K8s API error: {0}")]
    K8sApiError(String),

    // -- Database --
    #[error("Database error: {0}")]
    DatabaseError(String),

    // -- Authentication / authorization --
    #[error("Authentication error: {0}")]
    AuthError(String),

    #[error("Forbidden: {0}")]
    Forbidden(String),

    // -- Infrastructure --
    #[error("Bad gateway: {0}")]
    BadGateway(String),

    #[error("Service unavailable: {0}")]
    ServiceUnavailable(String),

    #[error("Internal error: {0}")]
    InternalError(String),
}

// ---------------------------------------------------------------------------
// IntoResponse
// ---------------------------------------------------------------------------

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        #[derive(Serialize)]
        struct ErrorResponse {
            error: &'static str,
            message: String,
        }

        let (status, error_code, message) = match &self {
            AppError::JsonRejection(rejection) => {
                // Bad user input — don't log it
                (rejection.status(), "invalid_json", rejection.body_text().to_string())
            }
            AppError::InboxNotFound(d) => {
                (StatusCode::NOT_FOUND, "inbox_not_found", d.clone())
            }
            AppError::InboxNotDeletable(d) => {
                (StatusCode::CONFLICT, "inbox_not_deletable", d.clone())
            }
            AppError::InboxAlreadyPublishing(d) => {
                (StatusCode::CONFLICT, "inbox_already_publishing", d.clone())
            }
            AppError::InvalidPublishParameters(d) => {
                (StatusCode::BAD_REQUEST, "invalid_publish_parameters", d.clone())
            }
            AppError::InvalidStateTransition(d) => {
                (StatusCode::UNPROCESSABLE_ENTITY, "invalid_state_transition", d.clone())
            }
            AppError::ValidationError(d) => {
                (StatusCode::BAD_REQUEST, "validation_error", d.clone())
            }
            AppError::Conflict(d) => (StatusCode::CONFLICT, "conflict", d.clone()),
            AppError::JobNotFound(d) => {
                (StatusCode::NOT_FOUND, "job_not_found", d.clone())
            }
            AppError::JobTimeout(d) => {
                (StatusCode::GATEWAY_TIMEOUT, "job_timeout", d.clone())
            }
            AppError::StorageError(d) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "storage_error", d.clone())
            }
            AppError::FileNotFound(d) => {
                (StatusCode::NOT_FOUND, "file_not_found", d.clone())
            }
            AppError::FileTransferFailed(d) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "file_transfer_failed", d.clone())
            }
            AppError::ChecksumMismatch(d) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "checksum_mismatch", d.clone())
            }
            AppError::KubernetesError(d) => {
                (StatusCode::SERVICE_UNAVAILABLE, "kubernetes_error", d.clone())
            }
            AppError::K8sApiError(d) => {
                (StatusCode::SERVICE_UNAVAILABLE, "k8s_api_error", d.clone())
            }
            AppError::InvalidCallbackSecret => {
                (StatusCode::UNAUTHORIZED, "invalid_callback_secret", "Invalid callback secret".to_string())
            }
            AppError::CallbackTimestampExpired => {
                (StatusCode::UNAUTHORIZED, "callback_timestamp_expired", "Callback timestamp expired".to_string())
            }
            AppError::CallbackNonceReused => {
                (StatusCode::UNAUTHORIZED, "callback_nonce_reused", "Callback nonce already used".to_string())
            }
            AppError::DatabaseError(d) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "database_error",
                d.clone(),
            ),
            AppError::AuthError(d) => {
                (StatusCode::UNAUTHORIZED, "auth_error", d.clone())
            }
            AppError::Forbidden(d) => (StatusCode::FORBIDDEN, "forbidden", d.clone()),
            AppError::BadGateway(d) => {
                (StatusCode::BAD_GATEWAY, "bad_gateway", d.clone())
            }
            AppError::ServiceUnavailable(d) => {
                (StatusCode::SERVICE_UNAVAILABLE, "service_unavailable", d.clone())
            }
            AppError::InternalError(d) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error", d.clone())
            }
        };

        // Log at appropriate level based on status class
        match status {
            s if s.is_server_error() => {
                tracing::error!(?self, error_code, "Server error");
            }
            s if s.is_client_error() && !matches!(self, AppError::JsonRejection(_)) => {
                tracing::warn!(?self, error_code, "Client error");
            }
            _ => {
                tracing::debug!(?self, error_code, "AppError");
            }
        }

        let body = AppJson(ErrorResponse {
            error: error_code,
            message,
        });

        (status, body).into_response()
    }
}

impl From<axum::extract::rejection::JsonRejection> for AppError {
    fn from(rejection: axum::extract::rejection::JsonRejection) -> Self {
        Self::JsonRejection(rejection)
    }
}

impl From<mongodb::error::Error> for AppError {
    fn from(e: mongodb::error::Error) -> Self {
        AppError::DatabaseError(e.to_string())
    }
}

impl From<reqwest::Error> for AppError {
    fn from(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            AppError::ServiceUnavailable("Request timed out".into())
        } else if e.is_connect() {
            AppError::ServiceUnavailable("Connection failed".into())
        } else {
            AppError::BadGateway(e.to_string())
        }
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::ValidationError(e.to_string())
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::InternalError(e.to_string())
    }
}

// ---------------------------------------------------------------------------
// Middleware
// ---------------------------------------------------------------------------

/// Middleware that logs error responses (4xx / 5xx) with context.
///
/// Inspects the response status code and logs a structured warning or error
/// entry so that issues are visible in the application logs.
pub async fn log_app_errors(
    request: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> Response {
    let method = request.method().clone();
    let uri = request.uri().clone();

    let response = next.run(request).await;

    let status = response.status();

    if status.is_server_error() {
        tracing::error!(%method, %uri, %status, "Server error response");
    } else if status.is_client_error() {
        tracing::warn!(%method, %uri, %status, "Client error response");
    }

    response
}