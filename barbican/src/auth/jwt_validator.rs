use jsonwebtoken::{decode, jwk::JwkSet, Algorithm, DecodingKey, Validation};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::info;

use crate::auth::claims::Claims;
use crate::config::Config;

/// JWT validator with JWKS caching
pub struct JwtValidator {
    config: Config,
    jwks: Arc<RwLock<Option<JwkSet>>>,
    last_refresh: Arc<RwLock<Instant>>,
}

impl JwtValidator {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            jwks: Arc::new(RwLock::new(None)),
            last_refresh: Arc::new(RwLock::new(Instant::now())),
        }
    }

    /// Validate a JWT token and return the claims
    pub async fn validate(&self, token: &str) -> Result<Claims, JwtError> {
        // Check if we need to refresh JWKS
        self.maybe_refresh_jwks().await?;

        // Get the JWKS
        let jwks_guard = self.jwks.read().await;
        let jwks = jwks_guard.as_ref().ok_or(JwtError::JwksNotAvailable)?;

        // Decode the token header to get the kid
        let header = jsonwebtoken::decode_header(token)
            .map_err(|e| JwtError::InvalidToken(format!("Failed to decode header: {}", e)))?;

        // Find the matching JWK
        let jwk = jwks.keys.iter().find(|k| k.common.key_id == header.kid)
            .ok_or_else(|| JwtError::InvalidToken("Unknown key ID (kid)".to_string()))?;

        // Get the public key from JWK - try x509 chain first
        let public_key = if let Some(x5c) = &jwk.common.x509_chain {
            if let Some(cert) = x5c.first() {
                format!("-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----", cert)
            } else {
                return Err(JwtError::InvalidKey("Empty x509 chain".to_string()));
            }
        } else {
            return Err(JwtError::InvalidKey("No x509 chain found in JWK".to_string()));
        };

        // Set up validation
        let mut validation = Validation::new(Algorithm::RS256);
        validation.leeway = 5000; // 5 seconds leeway for expiration (in milliseconds)
        validation.iss = Some(HashSet::from_iter(vec![self.config.keycloak_url.clone()]));
        validation.aud = Some(HashSet::from_iter(vec![self.config.keycloak_client_id.clone()]));

        // Explicitly reject "none" algorithm  
        // header.alg is already validated by jsonwebtoken library against allowed algorithms

        // Decode and validate the token
        let token_data = decode::<Claims>(
            token,
            &DecodingKey::from_rsa_pem(public_key.as_bytes())
                .map_err(|e| JwtError::InvalidKey(format!("Failed to parse public key: {}", e)))?,
            &validation,
        )
        .map_err(|e| match e.kind() {
            jsonwebtoken::errors::ErrorKind::ExpiredSignature => JwtError::TokenExpired,
            jsonwebtoken::errors::ErrorKind::InvalidIssuer => JwtError::InvalidIssuer,
            jsonwebtoken::errors::ErrorKind::InvalidAudience => JwtError::InvalidAudience,
            jsonwebtoken::errors::ErrorKind::InvalidAlgorithm => JwtError::InvalidToken("Invalid algorithm".to_string()),
            _ => JwtError::InvalidToken(e.to_string()),
        })?;

        Ok(token_data.claims)
    }

    /// Refresh JWKS if needed
    async fn maybe_refresh_jwks(&self) -> Result<(), JwtError> {
        let last_refresh = self.last_refresh.read().await;
        let refresh_interval = Duration::from_secs(self.config.jwks_refresh_secs);

        if last_refresh.elapsed() < refresh_interval {
            return Ok(());
        }
        drop(last_refresh);

        // Try to acquire write lock and refresh
        let mut jwks_guard = self.jwks.write().await;
        let mut last_refresh_guard = self.last_refresh.write().await;

        // Double-check after acquiring write lock
        if last_refresh_guard.elapsed() < refresh_interval {
            return Ok(());
        }

        // Fetch new JWKS
        let jwks_url = self.config.jwks_url();
        let response = reqwest::Client::new()
            .get(&jwks_url)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| JwtError::JwksFetchFailed(e.to_string()))?;

        if !response.status().is_success() {
            return Err(JwtError::JwksFetchFailed(format!("HTTP {}", response.status())));
        }

        let jwks: JwkSet = response
            .json()
            .await
            .map_err(|e| JwtError::JwksParseFailed(e.to_string()))?;

        info!("Refreshed JWKS from {}", jwks_url);
        *jwks_guard = Some(jwks);
        *last_refresh_guard = Instant::now();

        Ok(())
    }
}

/// JWT validation errors
#[derive(Debug, thiserror::Error)]
pub enum JwtError {
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
}