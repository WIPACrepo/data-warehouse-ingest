//! Service-to-service Bearer tokens (Citadel → Barbican, Barbican → Citadel).
//!
//! A service that calls a peer needs a JWT the peer accepts. The
//! [`ServiceTokenProvider`] obtains one and keeps it fresh:
//!
//! - **Client credentials** (production): Keycloak's
//!   `grant_type=client_credentials` at [`AuthConfig::token_url`], using a
//!   client ID and a client **secret read from an environment variable**
//!   (in Kubernetes, a Secret injected via `env.valueFrom.secretKeyRef`).
//! - **Static file** (development only): a pre-issued token re-read from disk
//!   on every refresh, so an external refresher can rewrite it.
//!
//! The token is cached and refreshed shortly before it expires. After a 401
//! from the peer, callers call [`ServiceTokenProvider::invalidate`] and retry
//! once. See `common/src/docs/auth.md` ("Service-to-service tokens").
//!
//! Variables (for caller prefix `P` and peer `Q`, e.g. `CITADEL`/`BARBICAN`):
//! `{P}_{Q}_CLIENT_ID`, `{P}_{Q}_CLIENT_SECRET`, `{P}_{Q}_SERVICE_TOKEN_FILE`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::auth::config::AuthConfig;
use crate::auth::error::{AuthError, ConfigError};

/// Refresh this long before expiry (capped at half the token lifetime).
const REFRESH_MARGIN: Duration = Duration::from_secs(30);
/// Re-read interval for a static token whose `exp` cannot be read.
const STATIC_FALLBACK_TTL: Duration = Duration::from_secs(60);

/// A secret string that never appears in `Debug`/`Display` output.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Access the secret value. Only call this where it is sent to Keycloak.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl std::fmt::Display for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// Where a service token comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceTokenSource {
    /// Keycloak client-credentials grant.
    ClientCredentials {
        token_url: String,
        client_id: String,
        client_secret: SecretString,
    },
    /// Pre-issued token file, re-read on refresh (development only).
    StaticFile(PathBuf),
}

impl ServiceTokenSource {
    /// Load from the process environment; see [`Self::from_lookup`].
    pub fn from_env(
        prefix: &str,
        peer: &str,
        auth: &AuthConfig,
    ) -> Result<Option<Self>, ConfigError> {
        Self::from_lookup(prefix, peer, auth, |name| std::env::var(name).ok())
    }

    /// Parse `{prefix}_{peer}_CLIENT_ID` / `_CLIENT_SECRET` /
    /// `_SERVICE_TOKEN_FILE` (and the global `PRODUCTION` flag).
    ///
    /// - client ID + secret → [`ServiceTokenSource::ClientCredentials`]
    ///   (token URL from `auth`, i.e. the caller's own Keycloak realm);
    /// - token file → [`ServiceTokenSource::StaticFile`];
    /// - nothing → `Ok(None)` (no `Authorization` header is sent), except in
    ///   production, where it is an error.
    ///
    /// Errors: both modes set; an ID without a secret or vice versa; empty
    /// values; static-file mode or nothing configured when `PRODUCTION=true`.
    pub fn from_lookup<F>(
        prefix: &str,
        peer: &str,
        auth: &AuthConfig,
        lookup: F,
    ) -> Result<Option<Self>, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let var = |suffix: &str| format!("{prefix}_{peer}_{suffix}");
        let get = |suffix: &str| lookup(&var(suffix)).filter(|v| !v.trim().is_empty());
        let production = lookup("PRODUCTION").is_some_and(|v| v == "true");

        let client_id = get("CLIENT_ID");
        let client_secret = get("CLIENT_SECRET");
        let token_file = get("SERVICE_TOKEN_FILE");

        let credentials = match (client_id, client_secret) {
            (Some(id), Some(secret)) => Some((id.trim().to_string(), SecretString::new(secret))),
            (None, None) => None,
            (Some(_), None) => {
                return Err(ConfigError::InvalidValue(format!(
                    "{} is set but {} is not",
                    var("CLIENT_ID"),
                    var("CLIENT_SECRET")
                )));
            }
            (None, Some(_)) => {
                return Err(ConfigError::InvalidValue(format!(
                    "{} is set but {} is not",
                    var("CLIENT_SECRET"),
                    var("CLIENT_ID")
                )));
            }
        };

        match (credentials, token_file) {
            (Some(_), Some(_)) => Err(ConfigError::InvalidValue(format!(
                "set either {}/{} or {}, not both",
                var("CLIENT_ID"),
                var("CLIENT_SECRET"),
                var("SERVICE_TOKEN_FILE")
            ))),
            (Some((client_id, client_secret)), None) => Ok(Some(Self::ClientCredentials {
                token_url: auth.token_url(),
                client_id,
                client_secret,
            })),
            (None, Some(_)) if production => Err(ConfigError::InvalidValue(format!(
                "{} (static token) is for development only; use {}/{} when PRODUCTION=true",
                var("SERVICE_TOKEN_FILE"),
                var("CLIENT_ID"),
                var("CLIENT_SECRET")
            ))),
            (None, Some(path)) => Ok(Some(Self::StaticFile(PathBuf::from(path)))),
            (None, None) if production => Err(ConfigError::InvalidValue(format!(
                "{}/{} must be set when PRODUCTION=true",
                var("CLIENT_ID"),
                var("CLIENT_SECRET")
            ))),
            (None, None) => Ok(None),
        }
    }
}

#[derive(Debug)]
struct CachedToken {
    token: String,
    refresh_at: Instant,
}

/// Successful client-credentials response (the fields we use).
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

/// Fetches and caches a service token; cheap to share via `Arc`.
pub struct ServiceTokenProvider {
    source: ServiceTokenSource,
    client: reqwest::Client,
    /// Held across a refresh so concurrent callers trigger a single fetch.
    cache: Mutex<Option<CachedToken>>,
}

impl std::fmt::Debug for ServiceTokenProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `source` redacts the secret; never print the cached token.
        f.debug_struct("ServiceTokenProvider")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

impl ServiceTokenProvider {
    pub fn new(source: ServiceTokenSource) -> Self {
        Self {
            source,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
            cache: Mutex::new(None),
        }
    }

    pub fn source(&self) -> &ServiceTokenSource {
        &self.source
    }

    /// A currently valid Bearer token (cached; refreshed near expiry).
    pub async fn bearer(&self) -> Result<String, AuthError> {
        let mut cache = self.cache.lock().await;
        if let Some(cached) = cache.as_ref()
            && Instant::now() < cached.refresh_at
        {
            return Ok(cached.token.clone());
        }
        let fresh = self.fetch().await?;
        let token = fresh.token.clone();
        *cache = Some(fresh);
        Ok(token)
    }

    /// Drop the cached token so the next [`Self::bearer`] fetches a new one
    /// (call after the peer answers 401).
    pub async fn invalidate(&self) {
        *self.cache.lock().await = None;
    }

    async fn fetch(&self) -> Result<CachedToken, AuthError> {
        match &self.source {
            ServiceTokenSource::ClientCredentials {
                token_url,
                client_id,
                client_secret,
            } => {
                let response = self
                    .client
                    .post(token_url)
                    .form(&[
                        ("grant_type", "client_credentials"),
                        ("client_id", client_id.as_str()),
                        ("client_secret", client_secret.expose()),
                    ])
                    .send()
                    .await
                    .map_err(|e| {
                        AuthError::TokenRequestFailed(format!("token request failed: {e}"))
                    })?;
                if !response.status().is_success() {
                    // Body may echo request details; log status only.
                    return Err(AuthError::TokenRequestFailed(format!(
                        "token endpoint returned {}",
                        response.status()
                    )));
                }
                let body: TokenResponse = response.json().await.map_err(|e| {
                    AuthError::TokenRequestFailed(format!("invalid token response: {e}"))
                })?;
                info!(client_id = %client_id, expires_in = body.expires_in, "Obtained service token");
                Ok(CachedToken {
                    token: body.access_token,
                    refresh_at: refresh_at(Duration::from_secs(body.expires_in)),
                })
            }
            ServiceTokenSource::StaticFile(path) => {
                let token = tokio::fs::read_to_string(path)
                    .await
                    .map_err(|e| {
                        AuthError::TokenRequestFailed(format!(
                            "cannot read service token file {}: {e}",
                            path.display()
                        ))
                    })?
                    .trim()
                    .to_string();
                let refresh_at = match unverified_expiry(&token) {
                    Some(remaining) => refresh_at(remaining),
                    None => {
                        warn!(
                            "Service token in {} has no readable exp; re-reading every {}s",
                            path.display(),
                            STATIC_FALLBACK_TTL.as_secs()
                        );
                        Instant::now() + STATIC_FALLBACK_TTL
                    }
                };
                Ok(CachedToken { token, refresh_at })
            }
        }
    }
}

/// When to refresh a token valid for `lifetime`: `REFRESH_MARGIN` early, but
/// never earlier than half-way through (short-lived tokens).
fn refresh_at(lifetime: Duration) -> Instant {
    let margin = REFRESH_MARGIN.min(lifetime / 2);
    Instant::now() + lifetime.saturating_sub(margin)
}

/// Remaining lifetime of a JWT from its `exp`, **without** verifying the
/// signature (only used to schedule our own refresh; the peer verifies it).
fn unverified_expiry(token: &str) -> Option<Duration> {
    #[derive(Deserialize)]
    struct Exp {
        exp: Option<i64>,
    }
    let mut validation = jsonwebtoken::Validation::default();
    validation.insecure_disable_signature_validation();
    validation.validate_exp = false;
    validation.validate_aud = false;
    validation.required_spec_claims.clear();
    let data = jsonwebtoken::decode::<Exp>(
        token,
        &jsonwebtoken::DecodingKey::from_secret(&[]),
        &validation,
    )
    .ok()?;
    let remaining = data.claims.exp? - chrono::Utc::now().timestamp();
    Some(Duration::from_secs(remaining.max(0) as u64))
}
