//! Authentication for the job-facing routes (`/api/v1/job/callback`,
//! `/api/v1/job/progress`).
//!
//! These routes accept **either** of two credentials:
//!
//! 1. **Bearer JWT with Citadel `admin`** — an operator acting on a job by
//!    hand (e.g. finishing a stuck job). The JWT signature and `exp` are the
//!    replay protection; the handler skips the callback-secret checks.
//! 2. **No `Authorization` header** — the `citadel-job` worker. The handler
//!    then requires `X-Callback-Secret` + a fresh `timestamp` + an unused
//!    `nonce` (see `handlers::job_callback::validate_callback_credentials`).
//!    This is the D9 trust edge: job pods have no Keycloak identity.
//!
//! **No fall-through:** when an `Authorization` header is present it is
//! authoritative. An invalid token (401) or a valid token without `admin`
//! (403) is rejected even if the request also carries a valid callback
//! secret, so the two credentials never combine.

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use tracing::{info, warn};

use crate::AppState;
use crate::error::AppError;
use common::AuthError;

/// Service name whose roles are checked (`resource_access.citadel.roles`).
const SERVICE: &str = "citadel";

/// Who is calling a job route; inserted into request extensions by
/// [`job_caller_auth`] and read by the job handlers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobCaller {
    /// Authenticated operator holding Citadel `admin` (JWT `sub` kept for audit).
    Admin { subject: String },
    /// Unauthenticated at this layer; the handler must verify the callback
    /// secret, timestamp and nonce.
    Job,
}

/// Middleware for the job routes; see the module docs for the rules.
pub async fn job_caller_auth(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let auth_header = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .map(|v| v.to_str().unwrap_or_default().to_string());

    let caller = match auth_header {
        None => JobCaller::Job,
        Some(header) => {
            let token = header.strip_prefix("Bearer ").ok_or_else(|| {
                warn!("Job route: malformed Authorization header");
                AppError::AuthError("Malformed Authorization header".to_string())
            })?;

            let claims = state
                .auth_state
                .validator
                .validate(token)
                .await
                .map_err(|e| match e {
                    AuthError::JwksNotAvailable
                    | AuthError::JwksFetchFailed(_)
                    | AuthError::JwksParseFailed(_) => AppError::ServiceUnavailable(
                        "JWT verification temporarily unavailable".to_string(),
                    ),
                    _ => {
                        warn!("Job route: JWT validation failed");
                        AppError::AuthError("Invalid or expired token".to_string())
                    }
                })?;

            if !claims.is_admin(SERVICE) {
                warn!("Job route: JWT caller lacks citadel admin role");
                return Err(AppError::Forbidden(
                    "Job routes require the citadel 'admin' role when called with a JWT"
                        .to_string(),
                ));
            }

            info!(subject = %claims.sub, "Job route called by admin JWT");
            JobCaller::Admin {
                subject: claims.sub,
            }
        }
    };

    request.extensions_mut().insert(caller);
    Ok(next.run(request).await)
}
