//! JWT validation with comprehensive checks

use chrono::Utc;
use jsonwebtoken::{Validation, decode, decode_header};
use std::collections::HashSet;
use std::sync::Arc;

use crate::auth::claims::Claims;
use crate::auth::config::AuthConfig;
use crate::auth::error::AuthError;
use crate::auth::jwks_cache::JwksCache;
use crate::metrics;

pub struct JwtValidator {
    jwks_cache: Arc<JwksCache>,
    config: AuthConfig,
}

impl JwtValidator {
    pub fn new(jwks_cache: Arc<JwksCache>, config: AuthConfig) -> Self {
        Self { jwks_cache, config }
    }

    pub async fn validate(&self, token: &str) -> Result<Claims, AuthError> {
        let start = Utc::now();

        // 1. Decode header
        let header = decode_header(token)
            .map_err(|e| AuthError::InvalidToken(format!("Failed to decode header: {}", e)))?;

        // 2. Validate algorithm - EXPLICITLY REJECT SYMMETRIC ALGORITHMS
        let alg_str = format!("{:?}", header.alg);
        if alg_str == "none" || alg_str == "None" {
            metrics::ALGORITHM_REJECTED_TOTAL
                .with_label_values(&["none", "none_not_allowed"])
                .inc();
            return Err(AuthError::AlgorithmNotAllowed("none".to_string()));
        }

        if alg_str.starts_with("HS") {
            metrics::ALGORITHM_REJECTED_TOTAL
                .with_label_values(&[alg_str.as_str(), "symmetric_not_allowed"])
                .inc();
            return Err(AuthError::AlgorithmNotAllowed(format!(
                "Symmetric algorithm '{}' not allowed",
                alg_str
            )));
        }

        // 3. Check against configured whitelist
        if !self.config.allowed_algorithms.contains(&alg_str) {
            return Err(AuthError::AlgorithmNotAllowed(alg_str));
        }

        // 4. Check if kid exists
        let kid = header.kid.ok_or(AuthError::MissingKeyId)?;

        // 5. Optional: Refresh JWKS if kid not found (rate-limited)
        self.jwks_cache.refresh_on_unknown_kid(&kid).await.ok();

        // 6. Get decoding key
        let (decoding_key, alg) = self.jwks_cache.get_decoding_key(&kid).await.map_err(|e| {
            match e {
                AuthError::UnknownKeyId(_) => metrics::TOKEN_VALIDATION_TOTAL
                    .with_label_values(&["unknown_kid"])
                    .inc(),
                AuthError::JwksFetchFailed(_) | AuthError::JwksNotAvailable => {
                    metrics::TOKEN_VALIDATION_TOTAL
                        .with_label_values(&["jwks_error"])
                        .inc()
                }
                _ => metrics::TOKEN_VALIDATION_TOTAL
                    .with_label_values(&["key_error"])
                    .inc(),
            };
            e
        })?;

        // 7. Set up validation
        let mut validation = Validation::new(alg);
        validation.validate_exp = true;
        validation.leeway = self.config.leeway_ms;
        validation.iss = Some(HashSet::from_iter(vec![self.config.issuer()]));
        validation.aud = Some(HashSet::from_iter(vec![self.config.audience()]));

        // 8. Validate token and extract claims
        let token_data =
            decode::<Claims>(token, &decoding_key, &validation).map_err(|e| match e.kind() {
                jsonwebtoken::errors::ErrorKind::ExpiredSignature => {
                    metrics::TOKEN_VALIDATION_TOTAL
                        .with_label_values(&["expired"])
                        .inc();
                    AuthError::TokenExpired
                }
                jsonwebtoken::errors::ErrorKind::InvalidIssuer => {
                    metrics::TOKEN_VALIDATION_TOTAL
                        .with_label_values(&["invalid_issuer"])
                        .inc();
                    AuthError::InvalidIssuer
                }
                jsonwebtoken::errors::ErrorKind::InvalidAudience => {
                    metrics::TOKEN_VALIDATION_TOTAL
                        .with_label_values(&["invalid_audience"])
                        .inc();
                    AuthError::InvalidAudience
                }
                jsonwebtoken::errors::ErrorKind::InvalidSignature => {
                    metrics::TOKEN_VALIDATION_TOTAL
                        .with_label_values(&["invalid_signature"])
                        .inc();
                    AuthError::InvalidToken("Invalid signature".to_string())
                }
                _ => {
                    metrics::TOKEN_VALIDATION_TOTAL
                        .with_label_values(&["invalid_token"])
                        .inc();
                    AuthError::InvalidToken(e.to_string())
                }
            })?;

        // 9. Record metrics
        let duration = Utc::now().signed_duration_since(start).num_milliseconds() as f64 / 1000.0;
        metrics::TOKEN_VALIDATION_DURATION.observe(duration);
        metrics::TOKEN_VALIDATION_TOTAL
            .with_label_values(&["success"])
            .inc();

        Ok(token_data.claims)
    }
}
