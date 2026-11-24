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
