//! `CitadelClient` service-token behaviour against a stub Citadel.

use std::sync::{Arc, Mutex};

use axum::{Json, Router, extract::State, http::HeaderMap, http::StatusCode, routing::post};
use serde_json::json;

use crate::Config;
use crate::clients::citadel::CitadelClient;
use common::test_utils::{TEST_CLIENT_SECRET, TestKeycloak, start_test_keycloak};
use common::{SecretString, ServiceTokenProvider, ServiceTokenSource};

#[derive(Clone, Default)]
struct Stub {
    seen: Arc<Mutex<Vec<Option<String>>>>,
    reject_first: usize,
}

async fn start_stub(reject_first: usize) -> (String, Stub) {
    let stub = Stub {
        reject_first,
        ..Stub::default()
    };
    let app = Router::new()
        .route(
            "/api/v1/inbox/{id}/publish",
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
                Ok(Json(json!({ "job_id": "job-1", "message": "ok" })))
            }),
        )
        .with_state(stub.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, stub)
}

async fn client(citadel_url: &str) -> (CitadelClient, TestKeycloak) {
    let kc = start_test_keycloak("test").await;
    let provider = ServiceTokenProvider::new(ServiceTokenSource::ClientCredentials {
        token_url: kc.auth_config("x").token_url(),
        client_id: "barbican".into(),
        client_secret: SecretString::new(TEST_CLIENT_SECRET),
    });
    let config = Config {
        citadel_api_url: citadel_url.to_string(),
        citadel_api_retry_max: 1,
        ..Config::default()
    };
    let client = CitadelClient::new(&config, Some(Arc::new(provider))).unwrap();
    (client, kc)
}

#[tokio::test]
async fn test_sends_bearer_token() {
    let (url, stub) = start_stub(0).await;
    let (client, _kc) = client(&url).await;
    client.trigger_publish("abc").await.unwrap();
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
    let (client, _kc) = client(&url).await;
    client.trigger_publish("abc").await.unwrap();
    let seen = stub.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    assert_ne!(seen[0], seen[1], "retry must use a fresh token");
}

#[tokio::test]
async fn test_persistent_401_returns_error() {
    let (url, stub) = start_stub(usize::MAX).await;
    let (client, _kc) = client(&url).await;
    assert!(client.trigger_publish("abc").await.is_err());
    assert_eq!(stub.seen.lock().unwrap().len(), 2, "one refresh retry only");
}
