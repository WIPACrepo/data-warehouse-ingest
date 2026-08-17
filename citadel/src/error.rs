//! Citadel error types

use thiserror::Error;

pub type Result<T> = std::result::Result<T, CitadelError>;

#[derive(Error, Debug)]
pub enum CitadelError {
    // Configuration errors
    #[error("Configuration error: {0}")]
    ConfigError(String),
    
    // Database errors
    #[error("Database error: {0}")]
    DatabaseError(String),
    
    // MongoDB errors
    #[error("MongoDB error: {0}")]
    MongoError(#[from] mongodb::error::Error),
    
    // Authentication errors
    #[error("Authentication error: {0}")]
    AuthError(String),
    
    #[error("Invalid JWT: {0}")]
    InvalidJwt(String),
    
    #[error("Missing or invalid roles")]
    MissingRoles,
    
    // Authorization errors
    #[error("Forbidden: {0}")]
    Forbidden(String),
    
    // Inbox errors
    #[error("Inbox not found: {0}")]
    InboxNotFound(String),
    
    #[error("Inbox in invalid state: {0}")]
    InvalidInboxState(String),
    
    #[error("Inbox path already exists: {0}")]
    InboxPathExists(String),
    
    // Job errors
    #[error("Job not found: {0}")]
    JobNotFound(String),
    
    #[error("Job already completed: {0}")]
    JobCompleted(String),
    
    #[error("Job cancellation failed: {0}")]
    JobCancellationFailed(String),
    
    #[error("Job timeout: {0}")]
    JobTimeout(String),
    
    // Storage errors
    #[error("Storage error: {0}")]
    StorageError(String),
    
    #[error("File not found: {0}")]
    FileNotFound(String),
    
    #[error("File transfer failed: {0}")]
    FileTransferFailed(String),
    
    #[error("Checksum verification failed: {0}")]
    ChecksumMismatch(String),
    
    #[error("Path traversal attempt detected")]
    PathTraversalAttempt,
    
    #[error("Invalid base path")]
    InvalidBasePath,
    
    // Kubernetes errors
    #[error("Kubernetes error: {0}")]
    KubernetesError(String),
    
    #[error("K8s API error: {0}")]
    K8sApiError(String),
    
    // Barbican API errors
    #[error("Barbican API error: {0}")]
    BarbicanApiError(String),
    
    #[error("Barbican API conflict: {0}")]
    BarbicanConflict(String),
    
    #[error("Barbican API unavailable")]
    BarbicanUnavailable,
    
    // Integration errors
    #[error("Catalog notification failed: {0}")]
    CatalogNotifyFailed(String),
    
    #[error("LTA notification failed: {0}")]
    LtaNotifyFailed(String),
    
    // Callback errors
    #[error("Invalid callback secret")]
    InvalidCallbackSecret,
    
    #[error("Callback timestamp expired")]
    CallbackTimestampExpired,
    
    #[error("Callback nonce already used")]
    CallbackNonceReused,
    
    // HTTP errors
    #[error("HTTP error: {0}")]
    HttpError(#[from] reqwest::Error),
    
    // JSON errors
    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),
    
    // IO errors
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    
    // Other errors
    #[error("Internal error: {0}")]
    InternalError(String),
}

impl From<&CitadelError> for axum::http::StatusCode {
    fn from(error: &CitadelError) -> Self {
        match error {
            CitadelError::ConfigError(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            CitadelError::DatabaseError(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            CitadelError::MongoError(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            CitadelError::AuthError(_) => axum::http::StatusCode::UNAUTHORIZED,
            CitadelError::InvalidJwt(_) => axum::http::StatusCode::UNAUTHORIZED,
            CitadelError::MissingRoles => axum::http::StatusCode::UNAUTHORIZED,
            CitadelError::Forbidden(_) => axum::http::StatusCode::FORBIDDEN,
            CitadelError::InboxNotFound(_) => axum::http::StatusCode::NOT_FOUND,
            CitadelError::InvalidInboxState(_) => axum::http::StatusCode::CONFLICT,
            CitadelError::InboxPathExists(_) => axum::http::StatusCode::CONFLICT,
            CitadelError::JobNotFound(_) => axum::http::StatusCode::NOT_FOUND,
            CitadelError::JobCompleted(_) => axum::http::StatusCode::CONFLICT,
            CitadelError::JobCancellationFailed(_) => axum::http::StatusCode::CONFLICT,
            CitadelError::JobTimeout(_) => axum::http::StatusCode::GATEWAY_TIMEOUT,
            CitadelError::StorageError(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            CitadelError::FileNotFound(_) => axum::http::StatusCode::NOT_FOUND,
            CitadelError::FileTransferFailed(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            CitadelError::ChecksumMismatch(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            CitadelError::PathTraversalAttempt => axum::http::StatusCode::FORBIDDEN,
            CitadelError::InvalidBasePath => axum::http::StatusCode::BAD_REQUEST,
            CitadelError::KubernetesError(_) => axum::http::StatusCode::SERVICE_UNAVAILABLE,
            CitadelError::K8sApiError(_) => axum::http::StatusCode::SERVICE_UNAVAILABLE,
            CitadelError::BarbicanApiError(_) => axum::http::StatusCode::SERVICE_UNAVAILABLE,
            CitadelError::BarbicanConflict(_) => axum::http::StatusCode::CONFLICT,
            CitadelError::BarbicanUnavailable => axum::http::StatusCode::SERVICE_UNAVAILABLE,
            CitadelError::CatalogNotifyFailed(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            CitadelError::LtaNotifyFailed(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            CitadelError::InvalidCallbackSecret => axum::http::StatusCode::UNAUTHORIZED,
            CitadelError::CallbackTimestampExpired => axum::http::StatusCode::UNAUTHORIZED,
            CitadelError::CallbackNonceReused => axum::http::StatusCode::UNAUTHORIZED,
            CitadelError::HttpError(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            CitadelError::JsonError(_) => axum::http::StatusCode::BAD_REQUEST,
            CitadelError::IoError(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            CitadelError::InternalError(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl axum::response::IntoResponse for CitadelError {
    fn into_response(self) -> axum::response::Response {
        let status_code = <axum::http::StatusCode as From<&CitadelError>>::from(&self);
        
        // Log full error details server-side
        tracing::error!(error = ?self, "Citadel error occurred");
        
        // Create error response before moving self
        let error_response = {
            let error_message = match &self {
                CitadelError::PathTraversalAttempt => "Path traversal attempt detected".to_string(),
                CitadelError::Forbidden(_) => "Access forbidden".to_string(),
                _ => {
                    // Map internal errors to generic messages for security
                    match status_code {
                        axum::http::StatusCode::NOT_FOUND => "Resource not found".to_string(),
                        axum::http::StatusCode::CONFLICT => "Resource conflict".to_string(),
                        axum::http::StatusCode::BAD_REQUEST => "Invalid request".to_string(),
                        axum::http::StatusCode::UNAUTHORIZED => "Authentication required".to_string(),
                        axum::http::StatusCode::FORBIDDEN => "Access denied".to_string(),
                        axum::http::StatusCode::GATEWAY_TIMEOUT => "Request timeout".to_string(),
                        axum::http::StatusCode::SERVICE_UNAVAILABLE => "Service temporarily unavailable".to_string(),
                        _ => "An error occurred".to_string(),
                    }
                }
            };
            
            let error_code = match &self {
                CitadelError::ConfigError(_) => "config_error",
                CitadelError::DatabaseError(_) => "database_error",
                CitadelError::MongoError(_) => "database_error",
                CitadelError::AuthError(_) => "auth_error",
                CitadelError::InvalidJwt(_) => "invalid_jwt",
                CitadelError::MissingRoles => "missing_roles",
                CitadelError::Forbidden(_) => "forbidden",
                CitadelError::InboxNotFound(_) => "inbox_not_found",
                CitadelError::InvalidInboxState(_) => "invalid_inbox_state",
                CitadelError::InboxPathExists(_) => "inbox_path_exists",
                CitadelError::JobNotFound(_) => "job_not_found",
                CitadelError::JobCompleted(_) => "job_completed",
                CitadelError::JobCancellationFailed(_) => "job_cancellation_failed",
                CitadelError::JobTimeout(_) => "job_timeout",
                CitadelError::StorageError(_) => "storage_error",
                CitadelError::FileNotFound(_) => "file_not_found",
                CitadelError::FileTransferFailed(_) => "file_transfer_failed",
                CitadelError::ChecksumMismatch(_) => "checksum_mismatch",
                CitadelError::PathTraversalAttempt => "path_traversal_attempt",
                CitadelError::InvalidBasePath => "invalid_base_path",
                CitadelError::KubernetesError(_) => "kubernetes_error",
                CitadelError::K8sApiError(_) => "k8s_api_error",
                CitadelError::BarbicanApiError(_) => "barbican_api_error",
                CitadelError::BarbicanConflict(_) => "barbican_conflict",
                CitadelError::BarbicanUnavailable => "barbican_unavailable",
                CitadelError::CatalogNotifyFailed(_) => "catalog_notify_failed",
                CitadelError::LtaNotifyFailed(_) => "lta_notify_failed",
                CitadelError::InvalidCallbackSecret => "invalid_callback_secret",
                CitadelError::CallbackTimestampExpired => "callback_timestamp_expired",
                CitadelError::CallbackNonceReused => "callback_nonce_reused",
                CitadelError::HttpError(_) => "http_error",
                CitadelError::JsonError(_) => "json_error",
                CitadelError::IoError(_) => "io_error",
                CitadelError::InternalError(_) => "internal_error",
            };
            
            #[derive(serde::Serialize)]
            struct ErrorResponse {
                error: &'static str,
                message: String,
            }
            
            ErrorResponse {
                error: error_code,
                message: error_message,
            }
        };
        
        (status_code, axum::Json(error_response)).into_response()
    }
}