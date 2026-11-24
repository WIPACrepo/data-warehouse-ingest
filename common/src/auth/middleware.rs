//! Authentication middleware

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use std::sync::Arc;

use crate::auth::JwtValidator;
use crate::auth::claims::Claims;
use crate::auth::error::AuthError;
use crate::error::AppError;
use crate::metrics;

#[derive(Clone)]
pub struct AuthMiddlewareState {
    pub validator: Arc<JwtValidator>,
    pub required_roles: Vec<String>,
    pub service_name: String,
}

/// Authenticate and authorize a request against the configured JWT validator.
///
/// Rejections return an [`AppError`], which serializes to a JSON body
/// `{error, message}` (decision D7) instead of a bare status code:
/// - missing/invalid `Authorization` header → `AppError::AuthError` (401,
///   code `auth_error`);
/// - JWT validation failure → `AppError::AuthError` (401, redacted reason);
/// - JWKS unavailable/fetch/parse failure → `AppError::ServiceUnavailable`
///   (503, code `service_unavailable`);
/// - missing required role → `AppError::Forbidden` (403, code `forbidden`).
///
/// An empty `required_roles` list disables the role gate entirely (valid
/// Bearer token only) — used by services like Barbican whose internal
/// transition endpoints are called only by a trusted peer (D9).
pub async fn auth_middleware(
    State(state): State<Arc<AuthMiddlewareState>>,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let auth_header = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());

    let token = match auth_header {
        Some(header) if header.starts_with("Bearer ") => &header[7..],
        _ => {
            tracing::warn!("Missing or invalid authorization header");
            return Err(AppError::AuthError(
                "Missing or invalid Authorization header".to_string(),
            ));
        }
    };

    let claims = match state.validator.validate(token).await {
        Ok(claims) => claims,
        Err(e) => {
            tracing::warn!("JWT validation failed: {}", redact_auth_error(&e));
            return Err(match e {
                AuthError::JwksNotAvailable
                | AuthError::JwksFetchFailed(_)
                | AuthError::JwksParseFailed(_) => AppError::ServiceUnavailable(
                    "JWT verification temporarily unavailable".to_string(),
                ),
                other => AppError::AuthError(redact_auth_error(&other)),
            });
        }
    };

    // An empty `required_roles` list means this route is public-Bearer: the
    // caller must present a valid JWT but no role gate applies (D9 — Barbican's
    // 12 internal `*/{start,progress,complete,failure}` routes are reachable by
    // any valid service token; trust is delegated to Citadel, the only caller).
    let has_required_role = state.required_roles.is_empty()
        || state.required_roles.iter().any(|role| {
            claims.has_role(&state.service_name, role) || claims.is_admin(&state.service_name)
        });

    if !has_required_role {
        // Track which specific role was missing for denied requests
        for role in &state.required_roles {
            if !claims.has_role(&state.service_name, role) && !claims.is_admin(&state.service_name)
            {
                metrics::AUTHZ_DENIED_BY_ROLE
                    .with_label_values(&[role, &state.service_name])
                    .inc();
            }
        }
        tracing::warn!(
            "User [REDACTED] lacks required roles for service '{}'",
            state.service_name
        );
        metrics::AUTHZ_CHECK_TOTAL
            .with_label_values(&["denied", &state.service_name])
            .inc();
        return Err(AppError::Forbidden(format!(
            "Missing required role for service '{}'",
            state.service_name
        )));
    }

    metrics::AUTHZ_CHECK_TOTAL
        .with_label_values(&["allowed", &state.service_name])
        .inc();
    request.extensions_mut().insert(claims);

    Ok(next.run(request).await)
}

fn redact_auth_error(err: &AuthError) -> String {
    match err {
        AuthError::InvalidToken(_) => "Invalid token".to_string(),
        AuthError::TokenExpired => "Token expired".to_string(),
        AuthError::InvalidIssuer => "Invalid issuer".to_string(),
        AuthError::InvalidAudience => "Invalid audience".to_string(),
        AuthError::MissingKeyId => "Missing key ID".to_string(),
        AuthError::UnknownKeyId(_) => "Unknown key ID".to_string(),
        AuthError::AlgorithmNotAllowed(_) => "Algorithm not allowed".to_string(),
        _ => "Authentication failed".to_string(),
    }
}

pub trait ClaimsExtractor {
    fn claims(&self) -> Option<&Claims>;
}

impl ClaimsExtractor for Request {
    fn claims(&self) -> Option<&Claims> {
        self.extensions().get::<Claims>()
    }
}
