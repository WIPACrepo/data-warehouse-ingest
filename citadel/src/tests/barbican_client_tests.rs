//! `BarbicanClient` service-token behaviour against a stub Barbican.

use std::sync::{Arc, Mutex};

use axum::{Json, Router, extract::State, http::HeaderMap, http::StatusCode, routing::post};
use mongodb::bson::oid::ObjectId;

use crate::config::CitadelConfig;
use crate::integrations::barbican::BarbicanClient;
use common::test_utils::{TEST_CLIENT_SECRET, create_test_inbox_with_id, start_test_keycloak};
use common::{SecretString, ServiceTokenProvider, ServiceTokenSource};

#[derive(Clone, Default)]
struct Stub {
    /// Authorization headers seen, in order.
    seen: Arc<Mutex<Vec<Option<String>>>>,
    /// Number of initial requests to answer with 401.
    reject_first: usize,
}

async fn start_stub(reject_first: usize) -> (String, Stub) {
    let stub = Stub {
        reject_first,
        ..Stub::default()
    };
    let app = Router::new()
        .route(
            "/api/v1/inbox/{id}/create/start",
            post(|State(st): State<Stub>, headers: HeaderMap| async move {
                let auth = headers
                    .get("authorization")
                    .map(|v| v.to_str().unwrap().to_string());
                let n = {
                    let mut seen = st.seen.lock().unwrap();
                    seen.push(auth);
                    seen.len()
                };
                if n <= st.reject_first {
                    return Err(StatusCode::UNAUTHORIZED);
                }
                Ok(Json(create_test_inbox_with_id(ObjectId::new(), "x", 0)))
            }),
        )
        .with_state(stub.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, stub)
}

async fn client(barbican_url: &str) -> (BarbicanClient, common::test_utils::TestKeycloak) {
    let kc = start_test_keycloak("test").await;
    let provider = ServiceTokenProvider::new(ServiceTokenSource::ClientCredentials {
        token_url: kc.auth_config("x").token_url(),
        client_id: "citadel".into(),
        client_secret: SecretString::new(TEST_CLIENT_SECRET),
    });
    let config = CitadelConfig {
        barbican_api_url: barbican_url.to_string(),
        barbican_api_retry_max: 0,
        barbican_api_retry_backoff_ms: 1,
        ..CitadelConfig::default()
    };
    let client = BarbicanClient::new(config).with_token_provider(Arc::new(provider));
    (client, kc)
}

#[tokio::test]
async fn test_sends_bearer_token() {
    let (url, stub) = start_stub(0).await;
    let (client, _kc) = client(&url).await;
    client
        .create_start(&ObjectId::new(), "job-1")
        .await
        .unwrap();
    let seen = stub.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert!(
        seen[0].as_deref().unwrap().starts_with("Bearer "),
        "{seen:?}"
    );
}

#[tokio::test]
async fn test_401_refreshes_token_and_retries_once() {
    let (url, stub) = start_stub(1).await;
    let (client, kc) = client(&url).await;
    client
        .create_start(&ObjectId::new(), "job-1")
        .await
        .unwrap();
    let seen = stub.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2, "exactly one retry");
    assert_ne!(seen[0], seen[1], "retry must use a fresh token");
    assert_eq!(
        kc.token_requests.load(std::sync::atomic::Ordering::SeqCst),
        2
    );
}

#[tokio::test]
async fn test_persistent_401_is_an_error_without_looping() {
    let (url, stub) = start_stub(usize::MAX).await;
    let (client, _kc) = client(&url).await;
    let err = client
        .create_start(&ObjectId::new(), "job-1")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("401"), "{err}");
    // retry_max = 0: one attempt + one token-refresh retry.
    assert_eq!(stub.seen.lock().unwrap().len(), 2);
}
