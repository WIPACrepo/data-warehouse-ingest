//! Mock Keycloak server for unit tests
//! Generates real keypairs for testing multiple algorithms (RS256, ES256, EdDSA)

use axum::{Router, http::StatusCode, response::Json, routing::get};
use serde_json::json;
use tokio::net::TcpListener;

/// Mock JWKS response for testing with multiple key types
pub fn mock_jwks() -> serde_json::Value {
    json!({
        "keys": [
            {
                "kty": "RSA",
                "kid": "test-key-rs256",
                "use": "sig",
                "alg": "RS256",
                "n": "mock-n-value-for-testing",
                "e": "AQAB"
            },
            {
                "kty": "RSA",
                "kid": "test-key-rs384",
                "use": "sig",
                "alg": "RS384",
                "n": "mock-n-value-for-testing",
                "e": "AQAB"
            },
            {
                "kty": "RSA",
                "kid": "test-key-rs512",
                "use": "sig",
                "alg": "RS512",
                "n": "mock-n-value-for-testing",
                "e": "AQAB"
            }
        ]
    })
}

/// Create a mock Keycloak server for testing.
///
/// Serves realm `test` at the layout `AuthConfig` expects (D10: no hard-coded
/// `/auth/` segment), so the returned base URL can be used directly as
/// `keycloak_url`.
pub async fn create_mock_keycloak_server() -> (String, tokio::task::JoinHandle<()>) {
    let app = Router::new()
        .route(
            "/realms/test/protocol/openid-connect/certs",
            get(|| async { Json(mock_jwks()) }),
        )
        .route("/realms/test", get(|| async { StatusCode::OK }));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{}", addr);

    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (base_url, handle)
}

/// Generate a test JWT token for unit testing
/// Note: This is a mock token for testing purposes only
pub fn generate_test_token() -> String {
    "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCIsImtpZCI6InRlc3Qta2V5LXxzMjU2In0.eyJzdWIiOiJ0ZXN0LXVzZXIiLCJpc3MiOiJodHRwOi8vbG9jYWxob3N0OjgwODAvYXV0aC9yZWFsbXMvZGF0YS13YXJlaG91c2UiLCJhdWQiOiJkZWZhdWx0LWNsaWVudCIsImV4cCI6OTk5OTk5OTk5OSwiaWF0IjowLCJyZXNvdXJjZV9hY2Nlc3MiOnsiYmFyYmljYW4iOnsicm9sZXMiOlsicmVhZCIsIndyaXRlIl19fX0.mock-signature".to_string()
}

// ---------------------------------------------------------------------------
// Real RS256 signing for tests (test-utils only)
// ---------------------------------------------------------------------------

/// `kid` of the runtime-generated test signing key.
pub const TEST_RSA_KID: &str = "test-signing-key";

/// Test signing key, generated at runtime (never stored in the repository).
struct TestSigningKey {
    encoding: jsonwebtoken::EncodingKey,
    /// Base64url modulus for the JWK.
    n: String,
    /// Base64url public exponent for the JWK.
    e: String,
}

/// The process-wide test signing key: a fresh 2048-bit RSA key generated on
/// first use (2048 is the minimum `jsonwebtoken`/ring accept for RS256) and
/// shared by all tests in the process, since key generation is slow.
fn test_signing_key() -> &'static TestSigningKey {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use rsa::pkcs1::EncodeRsaPrivateKey;
    use rsa::traits::PublicKeyParts;

    static KEY: std::sync::OnceLock<TestSigningKey> = std::sync::OnceLock::new();
    KEY.get_or_init(|| {
        let private = rsa::RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048)
            .expect("generate test RSA key");
        let der = private.to_pkcs1_der().expect("encode test RSA key");
        TestSigningKey {
            encoding: jsonwebtoken::EncodingKey::from_rsa_der(der.as_bytes()),
            n: URL_SAFE_NO_PAD.encode(private.n().to_bytes_be()),
            e: URL_SAFE_NO_PAD.encode(private.e().to_bytes_be()),
        }
    })
}

/// JWKS containing only the test signing key.
pub fn test_signing_jwks() -> serde_json::Value {
    let key = test_signing_key();
    json!({
        "keys": [{
            "kty": "RSA",
            "kid": TEST_RSA_KID,
            "use": "sig",
            "alg": "RS256",
            "n": key.n,
            "e": key.e
        }]
    })
}

/// Sign `claims` with the test key (RS256, `kid` = [`TEST_RSA_KID`]).
pub fn sign_test_jwt(claims: &serde_json::Value) -> String {
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some(TEST_RSA_KID.to_string());
    jsonwebtoken::encode(&header, claims, &test_signing_key().encoding).expect("test JWT must sign")
}

/// Claims for a test token issued by `issuer` for audience `aud`, carrying
/// `roles` under `resource_access.<service>.roles`, expiring in `ttl_secs`
/// (negative = already expired).
pub fn test_claims(
    issuer: &str,
    aud: &str,
    service: &str,
    roles: &[&str],
    ttl_secs: i64,
) -> serde_json::Value {
    let now = chrono::Utc::now().timestamp();
    json!({
        "sub": "test-subject",
        "iss": issuer,
        "aud": aud,
        "iat": now,
        "exp": now + ttl_secs,
        "resource_access": { service: { "roles": roles } }
    })
}

/// A running mock Keycloak for one realm, serving the test JWKS and a
/// client-credentials token endpoint.
pub struct TestKeycloak {
    /// Base URL to use as `{P}_KEYCLOAK_URL` (no `/auth` prefix).
    pub base_url: String,
    pub realm: String,
    /// Number of token-endpoint requests served (successful or not).
    pub token_requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    /// `expires_in` (seconds) returned by the token endpoint; adjustable.
    pub token_ttl_secs: std::sync::Arc<std::sync::atomic::AtomicI64>,
    pub handle: tokio::task::JoinHandle<()>,
}

/// Client secret the mock token endpoint accepts.
pub const TEST_CLIENT_SECRET: &str = "test-client-secret";

impl TestKeycloak {
    /// Issuer for this realm (`{base_url}/realms/{realm}`).
    pub fn issuer(&self) -> String {
        format!("{}/realms/{}", self.base_url, self.realm)
    }

    /// An `AuthConfig` pointed at this mock, expecting audience `aud`.
    pub fn auth_config(&self, aud: &str) -> crate::auth::AuthConfig {
        crate::auth::AuthConfig {
            keycloak_url: self.base_url.clone(),
            keycloak_realm: self.realm.clone(),
            keycloak_client_id: aud.to_string(),
            required_roles: vec!["read".to_string(), "write".to_string()],
            allowed_algorithms: ["RS256".to_string()].into_iter().collect(),
            jwks_refresh_secs: 60,
            leeway_ms: 0,
        }
    }

    /// Sign a token for this realm.
    pub fn token(&self, aud: &str, service: &str, roles: &[&str], ttl_secs: i64) -> String {
        sign_test_jwt(&test_claims(&self.issuer(), aud, service, roles, ttl_secs))
    }
}

/// Start a mock Keycloak for `realm` on an ephemeral port.
///
/// - `GET  /realms/{realm}/protocol/openid-connect/certs` → [`test_signing_jwks`]
/// - `POST /realms/{realm}/protocol/openid-connect/token` (form,
///   `grant_type=client_credentials`): with `client_secret` ==
///   [`TEST_CLIENT_SECRET`] returns a signed token whose `sub`/`azp` is the
///   `client_id` and whose `aud` is `"peer"`; otherwise 401.
pub async fn start_test_keycloak(realm: &str) -> TestKeycloak {
    use axum::extract::{Form, State};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

    #[derive(Clone)]
    struct MockState {
        issuer: Arc<std::sync::OnceLock<String>>,
        requests: Arc<AtomicUsize>,
        ttl: Arc<AtomicI64>,
    }

    let state = MockState {
        issuer: Arc::new(std::sync::OnceLock::new()),
        requests: Arc::new(AtomicUsize::new(0)),
        ttl: Arc::new(AtomicI64::new(300)),
    };

    let certs_path = format!("/realms/{realm}/protocol/openid-connect/certs");
    let token_path = format!("/realms/{realm}/protocol/openid-connect/token");
    let app = Router::new()
        .route(&certs_path, get(|| async { Json(test_signing_jwks()) }))
        .route(
            &token_path,
            axum::routing::post(
                |State(st): State<MockState>, Form(form): Form<HashMap<String, String>>| async move {
                    st.requests.fetch_add(1, Ordering::SeqCst);
                    let ok = form.get("grant_type").map(String::as_str)
                        == Some("client_credentials")
                        && form.get("client_secret").map(String::as_str)
                            == Some(TEST_CLIENT_SECRET);
                    if !ok {
                        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "invalid_client"})));
                    }
                    let client_id = form.get("client_id").cloned().unwrap_or_default();
                    let ttl = st.ttl.load(Ordering::SeqCst);
                    let mut claims = test_claims(
                        st.issuer.get().map(String::as_str).unwrap_or_default(),
                        "peer",
                        "peer",
                        &[],
                        ttl,
                    );
                    claims["sub"] = json!(client_id);
                    claims["azp"] = json!(client_id);
                    // Distinct per issuance so tests can tell tokens apart.
                    claims["jti"] = json!(st.requests.load(Ordering::SeqCst));
                    (
                        StatusCode::OK,
                        Json(json!({
                            "access_token": sign_test_jwt(&claims),
                            "token_type": "Bearer",
                            "expires_in": ttl,
                        })),
                    )
                },
            ),
        )
        .with_state(state.clone());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let _ = state.issuer.set(format!("{base_url}/realms/{realm}"));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    TestKeycloak {
        base_url,
        realm: realm.to_string(),
        token_requests: state.requests,
        token_ttl_secs: state.ttl,
        handle,
    }
}
