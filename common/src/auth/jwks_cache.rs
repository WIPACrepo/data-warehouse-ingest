//! JWKS caching with rate limiting

use jsonwebtoken::{DecodingKey, Algorithm};
use std::sync::Arc;
use tokio::sync::RwLock;
use chrono::{DateTime, Utc};
use tracing::{debug, info, warn};

use crate::auth::error::AuthError;

/// Custom JWKS structure that we can deserialize
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Jwks {
    pub keys: Vec<Jwk>,
}

/// Custom JWK structure supporting multiple key types
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Jwk {
    pub kty: String,
    pub kid: Option<String>,
    #[serde(rename = "use")]
    pub use_: Option<String>,
    pub alg: Option<String>,
    pub n: Option<String>,
    pub e: Option<String>,
    pub x: Option<String>,
    pub y: Option<String>,
    pub crv: Option<String>,
    #[serde(rename = "x5c")]
    pub x509_chain: Option<Vec<String>>,
}

pub struct JwksCache {
    jwks_url: String,
    cache: Arc<RwLock<Option<(Jwks, DateTime<Utc>)>>>,
    client: reqwest::Client,
    last_refresh: Arc<RwLock<DateTime<Utc>>>,
    refresh_interval_secs: u64,
    initialized: Arc<RwLock<bool>>,
}

impl JwksCache {
    pub fn new(jwks_url: String, refresh_interval_secs: u64) -> Self {
        Self {
            jwks_url,
            cache: Arc::new(RwLock::new(None)),
            client: reqwest::Client::new(),
            last_refresh: Arc::new(RwLock::new(Utc::now() - chrono::Duration::hours(1))),
            refresh_interval_secs,
            initialized: Arc::new(RwLock::new(false)),
        }
    }

    /// Initialize the cache on startup (prevents thundering herd)
    pub async fn initialize(&self) -> Result<(), AuthError> {
        let mut init_guard = self.initialized.write().await;
        if *init_guard {
            return Ok(()); // Already initialized
        }
        
        info!("Initializing JWKS cache from {}", self.jwks_url);
        match self.fetch_jwks().await {
            Ok(jwks) => {
                let now = Utc::now();
                let mut cache_guard = self.cache.write().await;
                *cache_guard = Some((jwks, now));
                let mut last_refresh_guard = self.last_refresh.write().await;
                *last_refresh_guard = now;
                *init_guard = true;
                info!("JWKS cache initialized successfully");
            }
            Err(e) => {
                warn!("Failed to initialize JWKS cache: {}", e);
                // Don't fail initialization, but log the error
            }
        }
        Ok(())
    }

    pub async fn get_jwks(&self) -> Result<Jwks, AuthError> {
        let now = Utc::now();
        
        // Check cache first
        {
            let cache_guard = self.cache.read().await;
            if let Some((jwks, cached_at)) = cache_guard.as_ref() {
                if now < *cached_at + chrono::Duration::seconds(self.refresh_interval_secs as i64) {
                    debug!("Using cached JWKS");
                    crate::metrics::JWKS_CACHE_HITS.inc();
                    crate::metrics::JWKS_CACHE_STALENESS.set(
                        now.signed_duration_since(*cached_at).num_seconds() as f64
                    );
                    return Ok(jwks.clone());
                } else {
                    // Cache exists but is stale - report staleness
                    crate::metrics::JWKS_CACHE_STALENESS.set(
                        now.signed_duration_since(*cached_at).num_seconds() as f64
                    );
                }
            }
        }
        
        // Rate limit: only refresh if 60 seconds have passed since last refresh attempt
        {
            let last_refresh_guard = self.last_refresh.read().await;
            if now < *last_refresh_guard + chrono::Duration::seconds(60) {
                // Return stale cache if available
                if let Some((jwks, _)) = self.cache.read().await.as_ref() {
                    warn!("JWKS refresh rate-limited, using stale cache");
                    crate::metrics::JWKS_REFRESH_RATE_LIMITED.inc();
                    return Ok(jwks.clone());
                }
            }
        }
        
        // Fetch new JWKS
        info!("Fetching JWKS from {}", self.jwks_url);
        crate::metrics::JWKS_FETCH_TOTAL.inc();
        let jwks = match self.fetch_jwks().await {
            Ok(jwks) => jwks,
            Err(e) => {
                crate::metrics::JWKS_FETCH_FAILURES.inc();
                return Err(e);
            }
        };
        
        // Update cache and last refresh time
        {
            let mut cache_guard = self.cache.write().await;
            *cache_guard = Some((jwks.clone(), now));
        }
        {
            let mut last_refresh_guard = self.last_refresh.write().await;
            *last_refresh_guard = now;
        }
        
        Ok(jwks)
    }

    async fn fetch_jwks(&self) -> Result<Jwks, AuthError> {
        let response = self.client
            .get(&self.jwks_url)
            .send()
            .await
            .map_err(|e| AuthError::JwksFetchFailed(e.to_string()))?;
        
        if !response.status().is_success() {
            return Err(AuthError::JwksFetchFailed(format!(
                "JWKS endpoint returned status: {}",
                response.status()
            )));
        }
        
        let jwks = response
            .json()
            .await
            .map_err(|e| AuthError::JwksParseFailed(e.to_string()))?;
        
        Ok(jwks)
    }

    pub async fn get_decoding_key(&self, kid: &str) -> Result<(DecodingKey, Algorithm), AuthError> {
        let jwks = self.get_jwks().await?;
        
        let jwk = jwks.keys.iter()
            .find(|k| k.kid.as_deref() == Some(kid))
            .ok_or_else(|| AuthError::UnknownKeyId(kid.to_string()))?;
        
        let alg = match jwk.alg.as_deref().unwrap_or("RS256") {
            "RS256" => Algorithm::RS256,
            "RS384" => Algorithm::RS384,
            "RS512" => Algorithm::RS512,
            "ES256" => Algorithm::ES256,
            "ES384" => Algorithm::ES384,
            "EdDSA" => Algorithm::EdDSA,
            other => return Err(AuthError::UnsupportedAlgorithm(other.to_string())),
        };
        
        let decoding_key = match jwk.kty.as_str() {
            "RSA" => {
                let n = jwk.n.as_ref()
                    .ok_or_else(|| AuthError::InvalidKey("Missing 'n' in RSA JWK".to_string()))?;
                let e = jwk.e.as_ref()
                    .ok_or_else(|| AuthError::InvalidKey("Missing 'e' in RSA JWK".to_string()))?;
                DecodingKey::from_rsa_components(n, e)
                    .map_err(|e| AuthError::InvalidKey(format!("Invalid RSA key: {}", e)))?
            }
            "EC" => {
                let x = jwk.x.as_ref()
                    .ok_or_else(|| AuthError::InvalidKey("Missing 'x' in EC JWK".to_string()))?;
                let y = jwk.y.as_ref()
                    .ok_or_else(|| AuthError::InvalidKey("Missing 'y' in EC JWK".to_string()))?;
                let crv = jwk.crv.as_ref()
                    .ok_or_else(|| AuthError::InvalidKey("Missing 'crv' in EC JWK".to_string()))?;
                if crv == "P-256" {
                    DecodingKey::from_ec_components(x, y)
                        .map_err(|e| AuthError::InvalidKey(format!("Invalid EC key: {}", e)))?
                } else {
                    return Err(AuthError::UnsupportedCurve(crv.to_string()));
                }
            }
            "OKP" => {
                let x = jwk.x.as_ref()
                    .ok_or_else(|| AuthError::InvalidKey("Missing 'x' in OKP JWK".to_string()))?;
                DecodingKey::from_ed_components(x)
                    .map_err(|e| AuthError::InvalidKey(format!("Invalid OKP key: {}", e)))?
            }
            other => return Err(AuthError::UnsupportedKeyType(other.to_string())),
        };
        
        Ok((decoding_key, alg))
    }

    /// Refresh JWKS if kid not found (rate-limited by get_jwks)
    pub async fn refresh_on_unknown_kid(&self, kid: &str) -> Result<(), AuthError> {
        // Check if kid exists in cache
        {
            let cache_guard = self.cache.read().await;
            if let Some((jwks, _)) = cache_guard.as_ref() {
                if jwks.keys.iter().any(|k| k.kid.as_deref() == Some(kid)) {
                    return Ok(()); // Kid found, no refresh needed
                }
            }
        }
        
        // Kid not found, force refresh (rate-limited by get_jwks)
        let _ = self.get_jwks().await?;
        Ok(())
    }
}