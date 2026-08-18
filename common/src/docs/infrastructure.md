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
#[automock]
#[async_trait]
pub trait InboxStore: Send + Sync {
    async fn insert_one(&self, inbox: InboxDB) -> Result<InsertOneResult, mongodb::error::Error>;
    async fn find_one(&self, filter: Document) -> Result<Option<InboxDB>, mongodb::error::Error>;
    async fn find_many(&self, filter: Document, options: FindOptions)
        -> Result<Vec<InboxDB>, mongodb::error::Error>;
    async fn update_one(&self, filter: Document, update: Document)
        -> Result<UpdateResult, mongodb::error::Error>;
    async fn delete_one(&self, filter: Document) -> Result<DeleteResult, mongodb::error::Error>;
}
```

### The MongoDB adapter

`MongoInboxStore` (`common/src/adapters.rs`) implements `InboxStore` against a MongoDB `Collection<InboxDB>`:

```rust
let collection: Collection<InboxDB> = common::db_collection(client, "inboxes");
let store = Arc::new(MongoInboxStore::new(collection));
```

Then handlers receive `Arc<dyn InboxStore>` (Barbican stores it in `AppState.store`).

### The mock

Because of `#[automock]`, `mockall` generates `MockInboxStore` with the same interface. It is re-exported and used everywhere unit tests need a store without MongoDB. See [testing.md](testing.md).

### Why it matters

The trait is the seam that keeps Barbican's handler logic testable in-process (mock store) while running against real MongoDB in production (`MongoInboxStore`). Adding a future storage backend means implementing the trait — handlers don't change.

---

## Error Handling

### `AppError`

Defined in `common/src/error.rs`, `AppError` is the unified error enum that both services convert into HTTP responses. Variants cover JSON rejection and domain errors:

```rust
pub enum AppError {
    JsonRejection(axum::extract::rejection::JsonRejection),
    InboxNotFound(String),
    InboxNotDeletable(String),
    InboxAlreadyPublishing(String),
    InvalidPublishParameters(String),
    DatabaseError(String),
    ValidationError(String),
    Conflict(String),
    InvalidStateTransition(String),
}
```

The `IntoResponse` impl maps each variant to an HTTP status and a JSON body of shape `{ "message": "..." }`:

| Variant | HTTP Status | Message |
|---------|-------------|---------|
| `JsonRejection` | (from rejection) | rejection body text |
| `InboxNotFound` | `404 Not Found` | "Inbox not found" |
| `InboxNotDeletable` | `409 Conflict` | "Inbox cannot be deleted" |
| `InboxAlreadyPublishing` | `409 Conflict` | "Inbox is already publishing" |
| `InvalidPublishParameters` | `400 Bad Request` | "Invalid publish parameters" |
| `DatabaseError` | `500 Internal Server Error` | "Database error: {e}" |
| `ValidationError` | `400 Bad Request` | variant message |
| `Conflict` | `409 Conflict` | variant message |
| `InvalidStateTransition` | `422 Unprocessable Entity` | variant message |

Full error details are logged at `debug` level; clients see only the sanitized message.

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

Defined in `common/src/db.rs`:

```rust
pub async fn db_setup() -> Client;        // connect + ping, verify reachability
pub fn db_database(client: Client) -> Database;   // Database for DATABASE_NAME
pub fn db_collection<T: Send + Sync>(client: Client, name: &str) -> Collection<T>;
```

- `db_setup` reads `DATABASE_URL` (default `mongodb://localhost:27017`) and `DATABASE_NAME` (default `data-warehouse-ingress`), connects, and pings the database to confirm it is reachable.
- `db_database`/`db_collection` resolve the database/collection from the same `DATABASE_NAME` env var, so both services address the same database unless configured otherwise.

Barbican uses these in `main.rs` (`db_setup`) and `serve` (`db_collection`). Citadel's `db.rs` builds its `Collection<common::models::InboxDB>` for the shared `inboxes` collection.

---

## Metrics

Defined in `common/src/metrics.rs`, built on `axum-prometheus`:

- `metrics_app()` — returns an `axum::Router` with a `GET /metrics` endpoint rendering Prometheus metrics (with pre-configured duration buckets for `http_requests_duration_seconds`).
- `start_metrics_server()` — binds `127.0.0.1:3001` and serves the metrics app **on a separate port** from the main API, spawning it as a background task.
- `router_prometheus(router)` — adds the `PrometheusMetricLayer` to a router.

Services call `common::start_metrics_server()` for the dedicated metrics port and/or wrap their main router with `router_prometheus`.

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
- [barbican/src/docs/architecture.md](../../barbican/src/docs/architecture.md) - How Barbican wires these utilities together
- [citadel/src/docs/architecture.md](../../citadel/src/docs/architecture.md) - How Citadel wires these utilities together
