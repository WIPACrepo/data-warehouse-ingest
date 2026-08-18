use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use std::sync::Arc;

use crate::auth::{JwtError, JwtValidator};
use crate::auth::claims::Claims;

/// Authentication middleware state
#[derive(Clone)]
pub struct AuthMiddlewareState {
    pub validator: Arc<JwtValidator>,
    pub required_roles: Vec<String>,
}

/// Authentication middleware
pub struct AuthMiddleware;

impl AuthMiddleware {
    /// Create a middleware that requires any of the specified roles
    pub fn require_roles(_roles: Vec<String>) -> Self {
        Self
    }

    /// Middleware function that validates JWT and checks roles
    pub async fn validate(
        State(state): State<AuthMiddlewareState>,
        mut request: Request,
        next: Next,
    ) -> Result<Response, StatusCode> {
        // Extract Authorization header
        let auth_header = request
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok());

        let token = match auth_header {
            Some(header) if header.starts_with("Bearer ") => &header[7..],
            Some(_) => return Err(StatusCode::UNAUTHORIZED),
            None => return Err(StatusCode::UNAUTHORIZED),
        };

        // Validate token
        let claims = match state.validator.validate(token).await {
            Ok(claims) => claims,
            Err(JwtError::TokenExpired) => return Err(StatusCode::UNAUTHORIZED),
            Err(JwtError::InvalidToken(_)) => return Err(StatusCode::UNAUTHORIZED),
            Err(JwtError::InvalidIssuer) => return Err(StatusCode::UNAUTHORIZED),
            Err(JwtError::InvalidAudience) => return Err(StatusCode::UNAUTHORIZED),
            Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
        };

        // Check if user has any of the required roles
        let has_required_role = state.required_roles.iter().any(|role| claims.has_role(role))
            || claims.is_admin();

        if !has_required_role {
            return Err(StatusCode::FORBIDDEN);
        }

        // Inject claims into request extensions
        request.extensions_mut().insert(claims);

        Ok(next.run(request).await)
    }
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