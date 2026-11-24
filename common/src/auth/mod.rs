//! Authentication module
//!
//! Provides JWT validation, JWKS caching, claims parsing, and middleware
//! for authentication and authorization.

pub mod claims;
pub mod config;
pub mod error;
pub mod jwks_cache;
pub mod jwt_validator;
pub mod metrics;
pub mod middleware;
pub mod service_token;

pub use claims::{Claims, ResourceAccess};
pub use config::{AuthConfig, build_auth_state};
pub use error::{AuthError, ConfigError};
pub use jwks_cache::{Jwk, Jwks, JwksCache};
pub use jwt_validator::JwtValidator;
pub use middleware::{AuthMiddlewareState, ClaimsExtractor, auth_middleware};
pub use service_token::{SecretString, ServiceTokenProvider, ServiceTokenSource};
