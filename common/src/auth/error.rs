//! Authentication error types

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("JWKS not available")]
    JwksNotAvailable,

    #[error("Failed to fetch JWKS: {0}")]
    JwksFetchFailed(String),

    #[error("Failed to parse JWKS: {0}")]
    JwksParseFailed(String),

    #[error("Invalid token: {0}")]
    InvalidToken(String),

    #[error("Token has expired")]
    TokenExpired,

    #[error("Invalid token issuer")]
    InvalidIssuer,

    #[error("Invalid token audience")]
    InvalidAudience,

    #[error("Invalid key: {0}")]
    InvalidKey(String),

    #[error("Missing key ID (kid) in JWT header")]
    MissingKeyId,

    #[error("Unknown key ID (kid): {0}")]
    UnknownKeyId(String),

    #[error("Algorithm not allowed: {0}")]
    AlgorithmNotAllowed(String),

    #[error("Unsupported algorithm: {0}")]
    UnsupportedAlgorithm(String),

    #[error("Unsupported key type: {0}")]
    UnsupportedKeyType(String),

    #[error("Unsupported curve: {0}")]
    UnsupportedCurve(String),
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Missing required environment variable: {0}")]
    MissingEnvVar(String),

    #[error("Invalid configuration value: {0}")]
    InvalidValue(String),
}

// Implement conversion to StatusCode for middleware
impl From<AuthError> for axum::http::StatusCode {
    fn from(err: AuthError) -> Self {
        match err {
            AuthError::TokenExpired
            | AuthError::InvalidToken(_)
            | AuthError::MissingKeyId
            | AuthError::InvalidIssuer
            | AuthError::InvalidAudience
            | AuthError::AlgorithmNotAllowed(_)
            | AuthError::UnknownKeyId(_) => Self::UNAUTHORIZED,

            AuthError::JwksNotAvailable
            | AuthError::JwksFetchFailed(_)
            | AuthError::JwksParseFailed(_)
            | AuthError::InvalidKey(_)
            | AuthError::UnsupportedAlgorithm(_)
            | AuthError::UnsupportedKeyType(_)
            | AuthError::UnsupportedCurve(_) => Self::SERVICE_UNAVAILABLE,
        }
    }
}
