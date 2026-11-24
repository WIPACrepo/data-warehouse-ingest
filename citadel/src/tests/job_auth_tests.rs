//! Router-level tests for job-route authentication (admin JWT *or* callback
//! secret + timestamp + nonce) and the user-route JWT gate.
//!
//! Requests that pass authentication continue into the handler and hit the
//! deliberately unreachable MongoDB, returning `database_error`. Asserting
//! that code (instead of an auth code) proves which layer accepted them.

use axum::http::StatusCode;
use axum_test::TestServer;
use chrono::Utc;
use serde_json::{Value, json};

use super::test_utils::{TEST_CALLBACK_SECRET, test_app_state};
use common::test_utils::{TestKeycloak, start_test_keycloak};

async fn server() -> (TestServer, TestKeycloak) {
    let kc = start_test_keycloak("test").await;
    let auth_state = common::build_auth_state(&kc.auth_config("citadel"), "citadel");
    let app = crate::create_app(test_app_state(auth_state).await);
    (TestServer::new(app).unwrap(), kc)
}

fn callback_body(with_replay_fields: bool) -> Value {
    let mut body = json!({ "job_id": "job-1", "status": "completed" });
    if with_replay_fields {
        body["timestamp"] = json!(Utc::now());
        body["nonce"] = json!(uuid::Uuid::new_v4().to_string());
    }
    body
}

fn progress_body(with_replay_fields: bool) -> Value {
    let mut body = json!({ "job_id": "job-1", "phase": "transferring" });
    if with_replay_fields {
        body["timestamp"] = json!(Utc::now());
        body["nonce"] = json!(uuid::Uuid::new_v4().to_string());
    }
    body
}

fn error_code(body: &str) -> String {
    serde_json::from_str::<Value>(body).unwrap()["error"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

const JOB_ROUTES: [&str; 2] = ["/api/v1/job/callback", "/api/v1/job/progress"];

fn body_for(route: &str, with_replay_fields: bool) -> Value {
    if route.ends_with("callback") {
        callback_body(with_replay_fields)
    } else {
        progress_body(with_replay_fields)
    }
}

#[tokio::test]
async fn test_job_secret_path_accepted() {
    let (server, _kc) = server().await;
    for route in JOB_ROUTES {
        let resp = server
            .post(route)
            .add_header("X-Callback-Secret", TEST_CALLBACK_SECRET)
            .json(&body_for(route, true))
            .await;
        // Auth passed; the nonce store (MongoDB) is unreachable.
        assert_eq!(error_code(&resp.text()), "database_error", "{route}");
    }
}

#[tokio::test]
async fn test_job_secret_path_wrong_secret_rejected() {
    let (server, _kc) = server().await;
    for route in JOB_ROUTES {
        let resp = server
            .post(route)
            .add_header("X-Callback-Secret", "wrong")
            .json(&body_for(route, true))
            .await;
        resp.assert_status(StatusCode::UNAUTHORIZED);
        assert_eq!(
            error_code(&resp.text()),
            "invalid_callback_secret",
            "{route}"
        );
    }
}

#[tokio::test]
async fn test_job_secret_path_requires_timestamp_and_nonce() {
    let (server, _kc) = server().await;
    for route in JOB_ROUTES {
        let resp = server
            .post(route)
            .add_header("X-Callback-Secret", TEST_CALLBACK_SECRET)
            .json(&body_for(route, false))
            .await;
        resp.assert_status(StatusCode::UNAUTHORIZED);
        assert_eq!(error_code(&resp.text()), "auth_error", "{route}");
    }
}

#[tokio::test]
async fn test_admin_jwt_accepted_without_secret_timestamp_or_nonce() {
    let (server, kc) = server().await;
    let token = kc.token("citadel", "citadel", &["admin"], 300);
    for route in JOB_ROUTES {
        let resp = server
            .post(route)
            .authorization_bearer(&token)
            .json(&body_for(route, false))
            .await;
        // Auth passed; the job lookup (MongoDB) is unreachable.
        assert_eq!(error_code(&resp.text()), "database_error", "{route}");
    }
}

#[tokio::test]
async fn test_non_admin_jwt_forbidden_even_with_valid_secret() {
    let (server, kc) = server().await;
    let token = kc.token("citadel", "citadel", &["write"], 300);
    for route in JOB_ROUTES {
        let resp = server
            .post(route)
            .authorization_bearer(&token)
            .add_header("X-Callback-Secret", TEST_CALLBACK_SECRET)
            .json(&body_for(route, true))
            .await;
        resp.assert_status(StatusCode::FORBIDDEN);
        assert_eq!(error_code(&resp.text()), "forbidden", "{route}");
    }
}

#[tokio::test]
async fn test_invalid_jwt_rejected_even_with_valid_secret() {
    let (server, kc) = server().await;
    let expired = kc.token("citadel", "citadel", &["admin"], -600);
    for token in [expired.as_str(), "not-a-jwt"] {
        for route in JOB_ROUTES {
            let resp = server
                .post(route)
                .authorization_bearer(token)
                .add_header("X-Callback-Secret", TEST_CALLBACK_SECRET)
                .json(&body_for(route, true))
                .await;
            resp.assert_status(StatusCode::UNAUTHORIZED);
            assert_eq!(error_code(&resp.text()), "auth_error", "{route}");
        }
    }
}

#[tokio::test]
async fn test_malformed_authorization_header_rejected() {
    let (server, _kc) = server().await;
    let resp = server
        .post("/api/v1/job/callback")
        .add_header("Authorization", "Basic dXNlcjpwYXNz")
        .add_header("X-Callback-Secret", TEST_CALLBACK_SECRET)
        .json(&callback_body(true))
        .await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
    assert_eq!(error_code(&resp.text()), "auth_error");
}

#[tokio::test]
async fn test_user_routes_still_require_jwt() {
    let (server, _kc) = server().await;
    let resp = server
        .post("/api/v1/inbox/000000000000000000000000/create")
        .add_header("X-Callback-Secret", TEST_CALLBACK_SECRET)
        .json(&json!({ "warehouse_path": "/x" }))
        .await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
    assert_eq!(error_code(&resp.text()), "auth_error");
}
