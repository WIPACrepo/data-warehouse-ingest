use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use std::sync::Arc;
use tracing::{debug, warn};

use crate::AppState;
use crate::auth::claims::Claims;

/// Middleware function that validates JWT and checks roles
/// Used with from_fn_with_state, so State will be Arc<AppState>
pub async fn validate_auth(
    State(state): State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Get Authorization header
    let auth_header = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());

    let token = match auth_header {
        Some(header) if header.starts_with("Bearer ") => &header[7..],
        Some(_) => {
            warn!("Invalid authorization header format");
            return Err(StatusCode::UNAUTHORIZED);
        }
        None => {
            warn!("Missing authorization header");
            return Err(StatusCode::UNAUTHORIZED);
        }
    };

    // Validate token
    let claims = match state.jwt_validator.validate(token).await {
        Ok(claims) => claims,
        Err(e) => {
            debug!("Token validation error: {:?}", e);
            return Err(StatusCode::UNAUTHORIZED);
        }
    };

    // Check if user has any of the required roles or is admin
    let has_required_role = state.config.required_roles.iter().any(|role| claims.has_role(role))
        || claims.is_admin();

    if !has_required_role {
        warn!("User {:?} lacks required roles", claims.sub);
        return Err(StatusCode::FORBIDDEN);
    }

    // Inject claims into request extensions
    request.extensions_mut().insert(claims);

    Ok(next.run(request).await)
}

/// Extension trait to extract claims from request
pub trait ClaimsExtractor {
    fn claims(&self) -> Option<&Claims>;
}

impl ClaimsExtractor for Request {
    fn claims(&self) -> Option<&Claims> {
        self.extensions().get::<Claims>()
    }
}