//! Authentication middleware

use axum::{
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::Response,
    body::Body,
};
use tracing::{info, warn};

use crate::auth::{AuthState, Claims, JwksCache};
use crate::config::CitadelConfig;
use crate::error::{CitadelError, Result};
use crate::AppState;

/// Extract JWT from Authorization header
fn extract_token(req: &Request<Body>) -> Option<String> {
    req.headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            if value.starts_with("Bearer ") {
                Some(value[7..].to_string())
            } else {
                None
            }
        })
}

/// Authentication middleware for protected endpoints
pub async fn auth_middleware(
    State(state): State<AppState>,
    mut req: Request<Body>,
    next: Next,
) -> std::result::Result<Response, StatusCode> {
    let token = extract_token(&req)
        .ok_or_else(|| {
            warn!("Missing authorization header");
            StatusCode::UNAUTHORIZED
        })?;
    
    let claims = crate::auth::validate_jwt(&token, &state.jwks_cache, &state.config)
        .await
        .map_err(|e| {
            warn!("JWT validation failed: {}", e);
            StatusCode::UNAUTHORIZED
        })?;
    
    // Check if user has required role
    let has_required_role = state.config.required_roles.iter()
        .any(|role| claims.has_role(role));
    
    if !has_required_role {
        warn!("User lacks required roles");
        return Err(StatusCode::FORBIDDEN);
    }
    
    // Insert claims into request extensions for handlers to use
    req.extensions_mut().insert(AuthState::new(claims));
    
    Ok(next.run(req).await)
}