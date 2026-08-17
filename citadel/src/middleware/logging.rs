//! Logging middleware

use axum::{
    http::{Request, StatusCode},
    middleware::Next,
    response::Response,
    body::Body,
};
use tracing::info;

pub async fn logging_middleware(
    req: Request<Body>,
    next: Next,
) -> std::result::Result<Response, StatusCode> {
    let method = req.method().clone();
    let uri = req.uri().clone();
    
    let response = next.run(req).await;
    
    info!(
        method = %method,
        uri = %uri,
        status = %response.status(),
        "Request completed"
    );
    
    Ok(response)
}