//! JWT authentication and authorization

use chrono::{DateTime, Utc};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::config::CitadelConfig;
use crate::error::{CitadelError, Result};

/// Custom JWKS structure that we can deserialize
#[derive(Debug, Clone, Deserialize)]
struct CustomJwks {
    pub keys: Vec<CustomJwk>,
}

#[derive(Debug, Clone, Deserialize)]
struct CustomJwk {
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
}

/// Resource access information from JWT claims
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceAccess {
    pub roles: Vec<String>,
}

/// JWT claims structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub iss: String,
    pub aud: String,
    pub exp: usize,
    pub iat: usize,
    pub resource_access: Option<HashMap<String, ResourceAccess>>,
}

impl Claims {
    pub fn get_roles(&self) -> Vec<String> {
        self.resource_access
            .as_ref()
            .and_then(|ra| ra.get("citadel"))
            .map(|ra| ra.roles.clone())
            .unwrap_or_default()
    }
    
    pub fn has_role(&self, role: &str) -> bool {
        self.get_roles().contains(&role.to_string())
    }
    
    pub fn is_admin(&self) -> bool {
        self.has_role("admin")
    }
    
    pub fn can_write(&self) -> bool {
        self.is_admin() || self.has_role("write")
    }
    
    pub fn can_read(&self) -> bool {
        self.is_admin() || self.has_role("read") || self.has_role("write")
    }
    
    pub fn is_internal(&self) -> bool {
        self.has_role("internal")
    }
}

pub struct JwksCache {
    config: CitadelConfig,
    cache: RwLock<Option<(CustomJwks, DateTime<Utc>)>>,
    client: reqwest::Client,
    last_refresh: RwLock<DateTime<Utc>>,
}

impl JwksCache {
    pub fn new(config: CitadelConfig) -> Self {
        Self {
            config,
            cache: RwLock::new(None),
            client: reqwest::Client::new(),
            last_refresh: RwLock::new(Utc::now() - chrono::Duration::hours(1)),
        }
    }
    
    pub async fn get_jwks(&self) -> Result<CustomJwks> {
        let now = Utc::now();
        
        {
            let cache_guard = self.cache.read().await;
            if let Some((jwks, cached_at)) = cache_guard.as_ref() {
                if now < *cached_at + chrono::Duration::seconds(self.config.jwks_refresh_secs as i64) {
                    debug!("Using cached JWKS");
                    return Ok(jwks.clone());
                }
            }
        }
        
        {
            let last_refresh_guard = self.last_refresh.read().await;
            if now < *last_refresh_guard + chrono::Duration::minutes(1) {
                if let Some((jwks, _)) = self.cache.read().await.as_ref() {
                    warn!("JWKS refresh rate limited, using potentially stale cache");
                    return Ok(jwks.clone());
                }
            }
        }
        
        info!("Fetching JWKS from Keycloak");
        let jwks = self.fetch_jwks().await?;
        
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
    
    async fn fetch_jwks(&self) -> Result<CustomJwks> {
        let response = self.client
            .get(&self.config.jwks_url())
            .send()
            .await
            .map_err(|e| CitadelError::AuthError(format!("Failed to fetch JWKS: {}", e)))?;
        
        if !response.status().is_success() {
            return Err(CitadelError::AuthError(format!(
                "JWKS endpoint returned status: {}",
                response.status()
            )));
        }
        
        let jwks: CustomJwks = response
            .json()
            .await
            .map_err(|e| CitadelError::AuthError(format!("Failed to parse JWKS: {}", e)))?;
        
  
        Ok(jwks)
    }
    
    pub async fn get_decoding_key(&self, kid: &str) -> Result<(DecodingKey, Algorithm)> {
        let jwks = self.get_jwks().await?;
        
        if let Some(jwk) = jwks.keys.iter().find(|k| k.kid.as_deref() == Some(kid)) {
            // Convert from custom JWK to jsonwebtoken DecodingKey
            let alg = match jwk.alg.as_deref().unwrap_or("RS256") {
                "RS256" => Algorithm::RS256,
                "RS384" => Algorithm::RS384,
                "RS512" => Algorithm::RS512,
                "ES256" => Algorithm::ES256,
                "ES384" => Algorithm::ES384,
                "EdDSA" => Algorithm::EdDSA,
                other => return Err(CitadelError::AuthError(format!("Unsupported algorithm: {}", other))),
            };
            
            // Build decoding key based on key type
            let decoding_key = if jwk.kty == "RSA" {
                let n = jwk.n.as_ref().ok_or_else(|| CitadelError::AuthError("Missing 'n' in RSA JWK".to_string()))?;
                let e = jwk.e.as_ref().ok_or_else(|| CitadelError::AuthError("Missing 'e' in RSA JWK".to_string()))?;
                DecodingKey::from_rsa_components(n, e)
                    .map_err(|e| CitadelError::AuthError(format!("Invalid RSA key: {}", e)))?
            } else if jwk.kty == "EC" {
                let x = jwk.x.as_ref().ok_or_else(|| CitadelError::AuthError("Missing 'x' in EC JWK".to_string()))?;
                let y = jwk.y.as_ref().ok_or_else(|| CitadelError::AuthError("Missing 'y' in EC JWK".to_string()))?;
                let crv = jwk.crv.as_ref().ok_or_else(|| CitadelError::AuthError("Missing 'crv' in EC JWK".to_string()))?;
                if crv == "P-256" {
                    DecodingKey::from_ec_components(x, y)
                        .map_err(|e| CitadelError::AuthError(format!("Invalid EC key: {}", e)))?
                } else {
                    return Err(CitadelError::AuthError(format!("Unsupported EC curve: {}", crv)));
                }
            } else if jwk.kty == "OKP" {
                let x = jwk.x.as_ref().ok_or_else(|| CitadelError::AuthError("Missing 'x' in OKP JWK".to_string()))?;
                DecodingKey::from_ed_components(x)
                    .map_err(|e| CitadelError::AuthError(format!("Invalid OKP key: {}", e)))?
            } else {
                return Err(CitadelError::AuthError(format!("Unsupported key type: {}", jwk.kty)));
            };
            
            Ok((decoding_key, alg))
        } else {
            Err(CitadelError::AuthError(format!("Key ID '{}' not found in JWKS", kid)))
        }
    }
}

pub async fn validate_jwt(token: &str, jwks_cache: &JwksCache, config: &CitadelConfig) -> Result<Claims> {
    let header = decode_header(token)
        .map_err(|e| CitadelError::InvalidJwt(format!("Failed to decode JWT header: {}", e)))?;
    
    let alg_str = format!("{:?}", header.alg);
    if alg_str == "none" || alg_str == "None" {
        return Err(CitadelError::InvalidJwt("Algorithm 'none' is not allowed".to_string()));
    }
    
    if !config.allowed_algorithms.contains(&alg_str) {
        return Err(CitadelError::InvalidJwt(format!(
            "Algorithm '{}' is not in the allowed list",
            alg_str
        )));
    }
    
    let kid = header.kid
        .ok_or_else(|| CitadelError::InvalidJwt("Missing key ID (kid) in JWT header".to_string()))?;
    
    let (decoding_key, alg) = jwks_cache.get_decoding_key(&kid).await?;
    
    let mut validation = Validation::new(alg);
    validation.validate_exp = true;
    validation.leeway = 5000;
    validation.iss = Some(HashSet::from_iter(vec![config.jwt_issuer()]));
    validation.aud = Some(HashSet::from_iter(vec![config.keycloak_client_id.clone()]));
    
    let token_data = decode::<Claims>(
        token,
        &decoding_key,
        &validation,
    )
    .map_err(|e| CitadelError::InvalidJwt(format!("JWT validation failed: {}", e)))?;
    
    Ok(token_data.claims)
}

pub struct AuthState {
    pub claims: Claims,
}

impl Clone for AuthState {
    fn clone(&self) -> Self {
        Self {
            claims: self.claims.clone(),
        }
    }
}

impl AuthState {
    pub fn new(claims: Claims) -> Self {
        Self { claims }
    }
}