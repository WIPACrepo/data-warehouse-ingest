use axum::{
    extract::FromRequest,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

// Create our own JSON extractor by wrapping `axum::Json`. This makes it easy to override the
// rejection and provide our own which formats errors to match our application.
//
// `axum::Json` responds with with plain text if the the input is invalid.
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

// The kinds of errors we can have in our application.
#[derive(Debug)]
pub enum AppError {
    // The request body contained invalid JSON
    JsonRejection(axum::extract::rejection::JsonRejection),
    
    // Barbican-specific errors
    InboxNotFound(String),
    InboxNotDeletable(String),
    InboxAlreadyPublishing(String),
    InvalidPublishParameters(String),
    DatabaseError(String),
    ValidationError(String),
}

// Tell axum how `AppError` should be converted into a response.
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        // How we want errors responses to be serialized
        #[derive(Serialize)]
        struct ErrorResponse {
            message: String,
        }
        
        let (status, message) = match &self {
            AppError::JsonRejection(rejection) => {
                // This error is caused by bad user input so don't log it
                (rejection.status(), rejection.body_text())
            }
            AppError::InboxNotFound(_) => (StatusCode::NOT_FOUND, "Inbox not found".to_string()),
            AppError::InboxNotDeletable(_) => (StatusCode::CONFLICT, "Inbox cannot be deleted".to_string()),
            AppError::InboxAlreadyPublishing(_) => (StatusCode::CONFLICT, "Inbox is already publishing".to_string()),
            AppError::InvalidPublishParameters(_) => (StatusCode::BAD_REQUEST, "Invalid publish parameters".to_string()),
            AppError::DatabaseError(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Database error: {}", e)),
            AppError::ValidationError(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
        };
        
        // Log full error details at debug level
        tracing::debug!(?self, "AppError");
        
        let response = (status, AppJson(ErrorResponse { message })).into_response();
        response
    }
}

impl From<axum::extract::rejection::JsonRejection> for AppError {
    fn from(rejection: axum::extract::rejection::JsonRejection) -> Self {
        Self::JsonRejection(rejection)
    }
}

// Our middleware is responsible for logging error details internally
pub async fn log_app_errors(_request: axum::http::Request<axum::body::Body>, next: axum::middleware::Next) -> Response {
    next.run(_request).await
}