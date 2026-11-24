# Shared Infrastructure

This document covers the non-model utilities in `common` that both services reuse: the storage abstraction, error handling, configuration/logging, database helpers, metrics, and HTTP middleware.

## Table of Contents

- [Storage Abstraction](#storage-abstraction)
- [Error Handling](#error-handling)
- [Configuration & Logging](#configuration--logging)
- [Database Helpers](#database-helpers)
- [Metrics](#metrics)
- [HTTP Middleware](#http-middleware)
- [See also](#see-also)

---

## Storage Abstraction

### The `InboxStore` trait

Defined in `common/src/traits.rs`. `InboxStore` is the async storage interface that handlers depend on. It lets service code (and tests) program against an abstraction instead of MongoDB directly:

```rust
#[cfg_attr(any(test, feature = "test-utils"), mockall::automock)]
#[async_trait]
pub trait InboxStore: Send + Sync {
    async fn ping(&self) -> Result<(), mongodb::error::Error>;
    async fn insert_one(&self, inbox: InboxDB) -> Result<InsertOneResult, mongodb::error::Error>;
    async fn find_one(&self, filter: Document) -> Result<Option<InboxDB>, mongodb::error::Error>;
    async fn find_many(&self, filter: Document, options: FindOptions)
        -> Result<Vec<InboxDB>, mongodb::error::Error>;
    async fn count(&self, filter: Document) -> Result<u64, mongodb::error::Error>;
    async fn update_one(&self, filter: Document, update: Document)
        -> Result<UpdateResult, mongodb::error::Error>;
    async fn delete_one(&self, filter: Document) -> Result<DeleteResult, mongodb::error::Error>;
}
```

`count` backs Barbican's paginated `GET /api/v1/inbox` envelope (`total` + `has_more` via `count_documents`).

`ping` is a liveness check of the backing database, used by Barbican's `GET /ready` probe (`200` when it succeeds, `503` otherwise). `MongoInboxStore` implements it as a `{ping: 1}` command against the `admin` database of the collection's client, so readiness reflects the same client the handlers use.

### The MongoDB adapter

`MongoInboxStore` (`common/src/adapters.rs`) implements `InboxStore` against a MongoDB `Collection<InboxDB>`:

```rust
let collection: Collection<InboxDB> = common::db_collection(client, "inboxes");
let store = Arc::new(MongoInboxStore::new(collection));
```

Then handlers receive `Arc<dyn InboxStore>` (Barbican stores it in `AppState.store`).

### The mock

`mockall` generates `MockInboxStore` with the same interface, but **only** when `common` is compiled for its own tests (`cfg(test)`) or with the `test-utils` feature (`#[cfg_attr(any(test, feature = "test-utils"), mockall::automock)]`). `mockall` is an optional dependency enabled by `test-utils` (plus a dev-dependency for `common`'s own tests), so it is not part of either service's production dependency graph. Under those same conditions `MockInboxStore` is re-exported as `common::MockInboxStore`. See [testing.md](testing.md).

### Why it matters

The trait is the seam that keeps Barbican's handler logic testable in-process (mock store) while running against real MongoDB in production (`MongoInboxStore`). Adding a future storage backend means implementing the trait — handlers don't change.

---

## Error Handling

### `AppError`

Defined in `common/src/error.rs`, `AppError` is the unified error enum that both services convert into HTTP responses. Variants cover JSON rejection, domain, job, callback, storage, Kubernetes, database, auth, and rate-limiting errors:

```rust
pub enum AppError {
    JsonRejection(axum::extract::rejection::JsonRejection),
    // Inbox / domain
    InboxNotFound(String), InboxNotDeletable(String),
    InboxAlreadyPublishing(String), InvalidPublishParameters(String),
    InvalidStateTransition(String), ValidationError(String), Conflict(String),
    // Jobs
    JobNotFound(String), JobTimeout(String),
    // Callbacks
    InvalidCallbackSecret, CallbackTimestampExpired, CallbackNonceReused,
    // Storage
    StorageError(String), FileNotFound(String),
    FileTransferFailed(String), ChecksumMismatch(String),
    // Kubernetes
    KubernetesError(String), K8sApiError(String),
    // Database
    DatabaseError(String),
    // Auth
    AuthError(String), Forbidden(String),
    // Infrastructure
    BadGateway(String), ServiceUnavailable(String), InternalError(String),
    // Rate limiting
    TooManyRequests(String),
}
```

The `IntoResponse` impl maps each variant to an HTTP status and a **JSON body of shape `{ "error": "...", "message": "..." }`** where `error` is a lowercase snake_case code and `message` is a human-readable string (there is no `details` field; client errors carry only what the doc contract promises):

| Variant | HTTP Status | `error` code |
|---------|-------------|--------------|
| `JsonRejection` | (from rejection) | `invalid_json` |
| `InboxNotFound` | `404 Not Found` | `inbox_not_found` |
| `InboxNotDeletable` | `409 Conflict` | `inbox_not_deletable` |
| `InboxAlreadyPublishing` | `409 Conflict` | `inbox_already_publishing` |
| `InvalidPublishParameters` | `400 Bad Request` | `invalid_publish_parameters` |
| `InvalidStateTransition` | `422 Unprocessable Entity` | `invalid_state_transition` |
| `ValidationError` | `400 Bad Request` | `validation_error` |
| `Conflict` | `409 Conflict` | `conflict` |
| `JobNotFound` | `404 Not Found` | `job_not_found` |
| `JobTimeout` | `504 Gateway Timeout` | `job_timeout` |
| `StorageError` | `500 Internal Server Error` | `storage_error` |
| `FileNotFound` | `404 Not Found` | `file_not_found` |
| `FileTransferFailed` | `500 Internal Server Error` | `file_transfer_failed` |
| `ChecksumMismatch` | `500 Internal Server Error` | `checksum_mismatch` |
| `KubernetesError` | `503 Service Unavailable` | `kubernetes_error` |
| `K8sApiError` | `503 Service Unavailable` | `k8s_api_error` |
| `InvalidCallbackSecret` | `401 Unauthorized` | `invalid_callback_secret` |
| `CallbackTimestampExpired` | `401 Unauthorized` | `callback_timestamp_expired` |
| `CallbackNonceReused` | `401 Unauthorized` | `callback_nonce_reused` |
| `DatabaseError` | `500 Internal Server Error` | `database_error` |
| `AuthError` | `401 Unauthorized` | `auth_error` |
| `Forbidden` | `403 Forbidden` | `forbidden` |
| `BadGateway` | `502 Bad Gateway` | `bad_gateway` |
| `ServiceUnavailable` | `503 Service Unavailable` | `service_unavailable` |
| `InternalError` | `500 Internal Server Error` | `internal_error` |
| `TooManyRequests` | `429 Too Many Requests` | `too_many_requests` |

Server errors are logged at `error!`, client errors at `warn!`, everything else at `debug!`; clients see only the sanitized `{error, message}` body.

### `AppJson`

`AppJson<T>` is a thin wrapper around `axum::Json` that overrides the rejection behavior so invalid JSON yields the application's own error format (via `AppError::JsonRejection`) instead of axum's default plain-text rejection:

```rust
#[derive(FromRequest)]
#[from_request(via(axum::Json), rejection(AppError))]
pub struct AppJson<T>(pub T);
```

It implements `IntoResponse` by delegating to `axum::Json`, and provides `AppJson::new(value)`.

### `log_app_errors`

A middleware pass-through hook that is responsible for logging error details internally. It wraps the request and forwards to the next handler.

---

## Configuration & Logging

### `logging_setup(crate_name: &str)`

Initializes `tracing`/`tracing-subscriber` (EnvFilter + fmt layer) and returns an initialized global subscriber. The filter string is built from environment variables:

| Env Var | Default | Controls |
|---------|---------|----------|
| `THIRD_PARTY_LOG_LEVEL` | `warn` | Third-party crates |
| `HTTP_LOG_LEVEL` | `debug` | `tower_http` |
| `LOG_LEVEL` | `info` | The app crate (`crate_name`) and `common` itself |

Invalid values are collected and logged as errors, falling back to the default. Each service passes its own crate name, e.g. `common::logging_setup("barbican")`. `RUST_LOG` (if set) takes precedence via `EnvFilter::try_from_default_env`.

### `shutdown_signal()`

Returns a future that completes on SIGINT (Ctrl+C) or SIGTERM (Unix), logging "Shutting down". Used with `axum::serve(...).with_graceful_shutdown(common::shutdown_signal())` for graceful drain on shutdown.

---

## Database Helpers

Defined in `common/src/db.rs`. All helpers take **explicit arguments** — they do not read environment variables:

```rust
pub async fn db_setup(uri: &str, name: &str) -> Result<Client, AppError>;
pub async fn db_setup_with_shutdown<S, F>(uri: &str, name: &str, shutdown: S) -> Result<Client, AppError>
    where S: Fn() -> F + Send + Sync, F: Future<Output = ()> + Send + 'static;
pub fn db_database(client: &Client, name: &str) -> Database;
pub fn db_collection<T: Send + Sync>(client: &Client, db_name: &str, collection_name: &str) -> Collection<T>;
```

- `db_setup(uri, name)` connects and pings the database to confirm it is reachable; both services pass their `mongodb_uri`/`mongodb_database` config. `db_setup_with_shutdown` additionally allows the connect/ping to be interrupted by a shutdown signal (used by Barbican's `main.rs`).
- `db_database`/`db_collection` resolve a `Database`/`Collection<T>` from an explicit name, so both services address the same shared `inboxes` collection by passing the same database name.
- The `common/src/tests/db_tests.rs` unit tests read their own environment for the fixtures they use — that is test infrastructure, not part of the library's API contract.

Barbican calls `db_setup_with_shutdown` in `main.rs` and `db_collection` in `serve`; Citadel's `db.rs` builds its `Collection<common::models::InboxDB>` for the shared `inboxes` collection the same way.

---

## Metrics

Defined in `common/src/metrics.rs`, built on `axum-prometheus`:

- `metrics_app()` — returns an `axum::Router` with a `GET /metrics` endpoint rendering Prometheus metrics (with pre-configured duration buckets for `http_requests_duration_seconds`).
- `start_metrics_server(port)` — binds **`0.0.0.0:{port}`** and serves the metrics app on a separate port from the main API, spawning it as a background task. The caller supplies the port from its service config (Barbican `prometheus_port` 9091, Citadel 9090); the server is reachable on all interfaces, not just loopback.
- `start_metrics_server_with_router(port, router)` — same binding (also `0.0.0.0:{port}`) but serves a caller-provided router. Services with their own Prometheus registry (Citadel's `CitadelMetrics`, Barbican's `BarbicanMetrics`) pass a `/metrics` router that renders it; `start_metrics_server` is the thin wrapper that uses `metrics_app()`.
- `router_prometheus(router)` — adds the `PrometheusMetricLayer` to a router.

Services call `common::start_metrics_server_with_router(port, own_router)` for the dedicated metrics port (so their registry is rendered) and/or wrap their main router with `router_prometheus`.

---

## HTTP Middleware

Defined in `common/src/middleware.rs`:

```rust
pub fn router_logging_layer<T>(router: Router<T>) -> Router<T>
```

Wraps a router with two `tower-http` layers as a tuple:

1. **`TraceLayer::new_for_http()`** — request tracing with a span including `method`, `uri`, and `matched_path` (from axum's `MatchedPath` extension).
2. **`TimeoutLayer::with_status_code(StatusCode::INTERNAL_SERVER_ERROR, 10s)`** — caps every request at 10 seconds so graceful shutdown never hangs on a stuck request; overlong requests receive `500`.

Applied via `common::router_logging_layer(router)` in Barbican's `create_app`.

---

## See also

- [README.md](README.md) - Main index
- [models.md](models.md) - The data types persisted through the storage abstraction
- [testing.md](testing.md) - `MockInboxStore` and test conventions
- [auth.md](auth.md) - JWT/Keycloak configuration and middleware (`auth` feature)
- [barbican/src/docs/architecture.md](../../barbican/src/docs/architecture.md) - How Barbican wires these utilities together
- [citadel/src/docs/architecture.md](../../citadel/src/docs/architecture.md) - How Citadel wires these utilities together
