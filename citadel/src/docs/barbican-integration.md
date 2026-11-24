# Barbican Integration

Documentation for the integration between Citadel and Barbican, from the Citadel side — the coordination model, the Barbican API client, retry strategy, and inbox state ownership.

## Table of Contents

- [Overview](#overview)
- [Coordination Model](#coordination-model)
- [Barbican API Client](#barbican-api-client)
- [Authentication to Barbican](#authentication-to-barbican)
- [Retry Strategy](#retry-strategy)
- [Inbox State Ownership](#inbox-state-ownership)
- [End-to-End Flows](#end-to-end-flows)
- [Configuration](#configuration)
- [Error Handling](#error-handling)
- [Monitoring](#monitoring)

---

## Overview

Citadel and Barbican work together to manage the complete inbox lifecycle:

| Service | Role |
|---------|------|
| **Barbican** | Frontend: user-facing API, inbox metadata CRUD, state machine |
| **Citadel** | Backend: physical directories, file transfers, jobs, downstream notifications |

**Key Integration Points (Citadel's side):**
1. Citadel reads inbox metadata directly from the shared MongoDB `inboxes` collection (read-only)
2. Citadel requests inbox state transitions via the Barbican API (write operations)
3. Both services share the MongoDB inbox collection (Citadel reads only; Barbican owns writes)

> The mirror document on the Barbican side is [`barbican/src/docs/citadel-integration.md`](../../barbican/src/docs/citadel-integration.md).

---

## Coordination Model

### Who Owns What

| Concern | Owner | Citadel's behavior |
|---------|-------|--------------------|
| Inbox metadata | Barbican | Reads via API; never writes directly |
| Inbox state transitions | Barbican | Requests via API (`POST /api/v1/inbox/{id}/{create\|publish\|delete}/{start\|progress\|complete\|failure}`) |
| Inbox physical directory | Citadel | Created under `CITADEL_INBOX_BASE_PATH/{inbox_id}` |
| File transfers | Citadel | Executed by `citadel-job` |
| Jobs (`jobs` collection) | Citadel | Owned entirely |
| Callback nonces | Citadel | Owned entirely |

### Shared Data

Both services use the shared `InboxDB` model from the `common` crate. Citadel's `BarbicanClient` returns and accepts this model:

```rust
pub struct InboxDB {
    pub id: Option<ObjectId>,
    pub name: String,
    pub size: u64,
    pub status: InboxStatus,
    pub warehouse_path: String,
    pub inbox_path: Option<String>,
    pub lifetime: Option<u64>,
    pub archive_enabled: bool,
    pub archive_sites: Vec<String>,
    pub version: u32,             // Optimistic locking
    pub current_job_id: Option<String>,
    pub progress: Option<InboxProgress>,
    pub error: Option<InboxError>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

**Important:** Citadel does **not** write to the inbox collection. All state changes flow through the Barbican API (see [Inbox State Ownership](#inbox-state-ownership)).

---

## Barbican API Client

The client lives at [`integrations/barbican.rs`](../integrations/barbican.rs).

### Methods (`InboxTransitioner` trait)

Handlers and the poller depend on the `InboxTransitioner` trait (mockable via `MockInboxTransitioner`); `BarbicanClient` is the production implementation. Each method POSTs a JSON body (always including `job_id`) and returns the updated `InboxDB`.

| Method | Barbican route | Barbican state change |
|--------|----------------|-----------------------|
| `create_start(id, job_id)` | `POST /api/v1/inbox/{id}/create/start` | `Active` → `Creating` |
| `create_progress(...)` | `.../create/progress` | progress only (`Creating`) |
| `create_complete(id, job_id, inbox_path)` | `.../create/complete` | `Creating` → `Ready` |
| `create_failure(id, job_id, code, message)` | `.../create/failure` | `Creating` → `Failed` |
| `publish_start(id, job_id)` | `POST /api/v1/inbox/{id}/publish/start` | `Ready` → `Publishing` |
| `publish_progress(id, job_id, phase, total_files, processed_files, total_bytes, transferred_bytes)` | `.../publish/progress` | progress only (`Publishing`) |
| `publish_complete(id, job_id, files_transferred, total_bytes, protected_path)` | `.../publish/complete` | `Publishing` → `Published` |
| `publish_failure(id, job_id, code, message)` | `.../publish/failure` | `Publishing` → `Failed` |
| `delete_start(id, job_id)` | `POST /api/v1/inbox/{id}/delete/start` | `Ready`/`Failed` → `Deleting` |
| `delete_progress(id, job_id, phase, total_bytes, deleted_bytes)` | `.../delete/progress` | progress only (`Deleting`) |
| `delete_complete(id, job_id)` | `.../delete/complete` | `Deleting` → `Deleted` |
| `delete_failure(id, job_id, code, message)` | `.../delete/failure` | `Deleting` → `Failed` |

Routes are registered in [`barbican/src/lib.rs`](../../../barbican/src/lib.rs) as Barbican's internal routes (Bearer-only, no role gate — D9). See the trait in [`integrations/barbican.rs`](../integrations/barbican.rs) for exact signatures.

**Note:** Citadel reads inbox metadata directly from MongoDB using `inboxes_collection()` rather than calling the Barbican API. This is more efficient since both services share the same MongoDB database.

### Construction

```rust
// main.rs (simplified)
let token_source = common::ServiceTokenSource::from_env("CITADEL", "BARBICAN", &auth_config)?;
let mut barbican = BarbicanClient::new(config.clone()).with_metrics(metrics.clone());
if let Some(source) = token_source {
    barbican = barbican.with_token_provider(Arc::new(common::ServiceTokenProvider::new(source)));
} // else: warning, no Authorization header
let barbican_client: Arc<dyn InboxTransitioner> = Arc::new(barbican);
```

`BarbicanClient::new` builds a `reqwest::Client` with `CITADEL_BARBICAN_API_TIMEOUT_SECS` as the per-request timeout. `with_metrics` attaches `CitadelMetrics` so the client can increment `citadel_barbican_api_errors_total` and drive the `citadel_circuit_breaker_state` gauge. The client is shared through `AppState.barbican_client`.

### Response Handling

All methods go through a private `post_action` → `do_post` → `send_once` helper:

- **401 Unauthorized** (with a token provider) → invalidate the cached token, retry **once** with a fresh one; see [Authentication to Barbican](#authentication-to-barbican)
- **409 Conflict** → `AppError::Conflict` — fails immediately, no retry, does not trip the breaker
- **Request error / other non-2xx / unparseable body** → `AppError::BadGateway` — retried
- **2xx** → parses and returns the updated `InboxDB`

---

## Authentication to Barbican

Barbican's internal routes require a Bearer JWT whose `aud` includes `barbican`; no role is checked (D9). Before D11 Citadel sent **no** `Authorization` header, so every Citadel → Barbican call was rejected with `401`.

- **Token source**: `CITADEL_BARBICAN_CLIENT_ID` + `CITADEL_BARBICAN_CLIENT_SECRET` (secret from an environment variable; in Kubernetes from a Secret via `env.valueFrom.secretKeyRef`) → Keycloak client-credentials grant at `{CITADEL_KEYCLOAK_URL}/realms/{realm}/protocol/openid-connect/token`. Dev-only alternative: `CITADEL_BARBICAN_SERVICE_TOKEN_FILE` (re-read on refresh). See [Configuration](configuration.md#barbican-service-token) for startup validation.
- **Caching**: `common::ServiceTokenProvider` caches the token and refreshes it 30 s before expiry (capped at half the lifetime).
- **401 from Barbican**: `do_post` invalidates the token and resends once. This inner retry is not a `post_action` retry and does not count toward the circuit breaker. If Barbican still answers `401`, the call fails with `BadGateway` ("check `CITADEL_BARBICAN_CLIENT_ID/SECRET` and the Keycloak audience mapper"), which `post_action` then treats like any other failure (retried, counted).
- **Token unavailable** (Keycloak down, bad credentials): `ServiceUnavailable` (`503`) for that attempt.
- **Secret rotation** requires a pod restart (the secret is read from the environment once at startup).
- **Keycloak**: the `citadel` client needs an audience mapper adding `barbican` to its tokens' `aud`.

---

## Retry Strategy

### Exponential Backoff

Every `InboxTransitioner` call retries `BadGateway` errors with exponential backoff:

```
Attempt 1: immediate
Retry 1:   after CITADEL_BARBICAN_API_RETRY_BACKOFF_MS   (default 1000 ms)
Retry 2:   after 2x
...        doubling, capped at 30 000 ms
```

- **Retries**: `CITADEL_BARBICAN_API_RETRY_MAX` (default 5) retries after the first attempt, i.e. up to 6 HTTP attempts per call
- **Timeouts**: `CITADEL_BARBICAN_API_TIMEOUT_SECS` (default 10 s) per attempt

### Rules

| Outcome | Behavior |
|---------|----------|
| `BadGateway` (network error, non-2xx except 409) | Retry with backoff |
| Retries exhausted | Error returned to caller; counts as one breaker failure |
| **409 Conflict** | Fail immediately — no retry, no breaker failure |
| Breaker open | Fail fast with `ServiceUnavailable` (503), no HTTP request |

### Circuit Breaker

`BarbicanClient` has an in-process circuit breaker (`is_breaker_open` / `trip_breaker` / `close_breaker`):

- **Failure counting**: a call whose retries are exhausted increments a failure counter. Any successful call resets the counter and closes the breaker.
- **Trip threshold**: the breaker opens when the counter reaches `max(CITADEL_BARBICAN_API_RETRY_MAX, 1)` (default 5) consecutive failed calls. 409s never trip it.
- **Open**: calls fail fast with `AppError::ServiceUnavailable("Barbican circuit breaker open")`; the `citadel_circuit_breaker_state` gauge is `1`.
- **Cooldown / half-open**: after **30 s** (`HALF_OPEN_AFTER_SECS`, not configurable) calls are let through again. A success closes the breaker (gauge `0`); a failure leaves the counter above the threshold, so the breaker re-trips immediately.

The breaker state is per-process (not shared across Citadel replicas).

---

## Inbox State Ownership

### Design Principle

Barbican is the **single writer** for inbox state. Citadel never calls MongoDB directly to change inbox status — it requests transitions so that Barbican can apply business rules, optimistic locking, and audit logging.

### Transitions Citadel Requests

| Operation | Caller | Calls | Resulting state |
|-----------|--------|-------|-----------------|
| Create directory | `create_inbox` handler | `create_start` → (mkdir) → `create_complete` | `Creating` → `Ready` |
| Publish (start) | `publish_inbox` handler | `publish_start` | `Publishing` |
| Delete (start) | `delete_inbox` handler | `delete_start` | `Deleting` |
| Progress | `job_progress` handler | `publish_progress` / `delete_progress` | unchanged |
| Terminal | `job_callback` handler | `publish_complete`/`publish_failure`/`delete_complete`/`delete_failure` | `Published`/`Deleted`/`Failed` |
| Self-heal | `polling/job_status.rs` | e.g. re-issues `publish_complete` for an inbox stuck in `Publishing` | `Published` |

### Compensation

Handled by [`handlers/compensate.rs`](../handlers/compensate.rs) when an operation fails **after** its `*/start` transition succeeded (otherwise the inbox would stay `Creating`/`Publishing`/`Deleting` and, for publish/delete, the `queued` job would count toward the in-flight quota forever):

| Failure | Compensation | Error code |
|---------|--------------|------------|
| `StorageManager::create_inbox` fails after `create_start` | `create_failure` → `Failed` | `storage_error` |
| `create_complete` fails | `create_failure` → `Failed`; the directory is kept (idempotent create, reused on retry) | `barbican_error` |
| `executor.submit` fails after `publish_start` / `delete_start` | `publish_failure` / `delete_failure` → `Failed`, **and** job document set `failed` with an error entry (leaves the in-flight quota) | `executor_error` |
| `publish_start` / `delete_start` itself fails | No Barbican call (the inbox never changed state); the just-inserted `queued` job row is deleted (`rollback_job`) so it cannot leak into the in-flight quota | — |

Compensation is best-effort (its own failures are only logged) and the handler returns the **original** error. It never runs once a job may be running: if `executor.submit` succeeded but recording the executor info on the job fails, the error is logged and the request still returns `202`; the callback and the status poller reconcile the job.

### State Change Flow

```
Citadel handler
  │
  ▼
barbican_client.publish_start(&id, &job_id)
  │  POST /api/v1/inbox/{id}/publish/start
  ▼
Barbican validates + applies (optimistic lock on version)
  │
  ├─ 200 OK ─────────────► proceed with physical operation
  ├─ 409 Conflict ───────► fail immediately (no retry)
  └─ other error/timeout ► retry (exponential backoff, 5 retries default)
                            │
                            └─ exhausted ─► fail operation
```

### Reading Inbox State

Handlers read inbox metadata directly from the shared MongoDB `inboxes` collection using `inboxes_collection()` before acting to validate preconditions:

- `create_inbox`: rejects if `inbox_path.is_some()`
- `publish_inbox` / `delete_inbox`: rejects if `inbox_path.is_none()`

This direct read approach is more efficient than calling the Barbican API and aligns with the architecture decision that Citadel can read the `inboxes` collection directly while Barbican retains exclusive write ownership.

---

## End-to-End Flows

### Create → Ready

```
User ──POST /api/v1/inbox──► Barbican (status: Active)
                                 │ auto-trigger
                                 ▼
User ──POST /inbox/{id}/create──► Citadel
                                 │ read inbox from MongoDB → validate no path
                                 │ StorageManager::create_inbox
                                 │ create_start → mkdir → create_complete via Barbican
                                 ▼
                           201 Created { inbox_id, inbox_path }
```

### Publish

```
User uploads files (out of band)
  │
User ──POST /inbox/{id}/publish──► Citadel
  │  read inbox from MongoDB → validate inbox_path present
  │  create PublishInbox JobDB (jobs collection)
  │  publish_start via Barbican
  │  executor.submit(job) → Running
  ▼
citadel-job: storage::transfer (lock → copy+verify → delete sources)
  │
  ▼
Progress POST /api/v1/job/progress → publish_progress via Barbican (best-effort)
  │
Callback POST /api/v1/job/callback → Completed / Failed
  ├── notify catalog → notify LTA (LTA only if catalog succeeded)
  └── publish_complete / publish_failure via Barbican
```

### Delete

```
User ──POST /inbox/{id}/delete──► Citadel
  │  validate inbox_path present
  │  create DeleteInbox JobDB
  │  delete_start via Barbican
  │  executor.submit(job) → Running
  ▼
worker removes inbox directory → callback → delete_complete / delete_failure via Barbican
```

**Delete progress:** `JobProgressRequest.transferred_bytes` means *bytes processed*; for a delete job Citadel forwards it to Barbican's `delete/progress` as `deleted_bytes` (with `total_bytes` and `phase`). `citadel-job`'s `execute_delete` does not currently send progress reports, so in practice a delete goes straight from `Deleting` to its terminal state.

---

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `CITADEL_BARBICAN_API_URL` | `http://barbican:8080` | Barbican API base URL |
| `CITADEL_BARBICAN_API_RETRY_MAX` | `5` | Retries per call after the first attempt; also the circuit-breaker trip threshold |
| `CITADEL_BARBICAN_API_RETRY_BACKOFF_MS` | `1000` | Initial backoff (doubles, capped 30s) |
| `CITADEL_BARBICAN_API_TIMEOUT_SECS` | `10` | Per-request timeout |
| `CITADEL_BARBICAN_CLIENT_ID` / `CITADEL_BARBICAN_CLIENT_SECRET` | *(unset)* | Service-token client credentials ([Authentication to Barbican](#authentication-to-barbican)) |
| `CITADEL_BARBICAN_SERVICE_TOKEN_FILE` | *(unset)* | Dev-only static token file |

### Example (Development)

```bash
CITADEL_BARBICAN_API_URL=http://localhost:8080
CITADEL_BARBICAN_API_RETRY_MAX=5
CITADEL_BARBICAN_API_RETRY_BACKOFF_MS=1000
CITADEL_BARBICAN_API_TIMEOUT_SECS=10
```

### Example (Kubernetes)

```bash
CITADEL_BARBICAN_API_URL=http://barbican.data-warehouse.svc.cluster.local:8080
CITADEL_BARBICAN_API_RETRY_MAX=5
CITADEL_BARBICAN_API_RETRY_BACKOFF_MS=500
CITADEL_BARBICAN_API_TIMEOUT_SECS=5
CITADEL_BARBICAN_CLIENT_ID=citadel
# CITADEL_BARBICAN_CLIENT_SECRET from env.valueFrom.secretKeyRef
```

---

## Error Handling

### Failure Modes

| Failure | Behavior |
|---------|----------|
| Barbican API unreachable | Retry with backoff; after `RETRY_MAX` retries → operation fails (`bad_gateway`, 502); repeated failures open the breaker (`service_unavailable`, 503) |
| 409 Conflict | Fail immediately (no retry) |
| Inbox not found | `InboxNotFound` (404) |
| Inbox in wrong state | Barbican 409 → `AppError::Conflict` (409 `conflict`) |
| Drop/network timeout | Per-request timeout; retry |
| Barbican `401` (service token rejected) | Invalidate token, resend once; still `401` → `bad_gateway` (then retried as usual) |
| Service token cannot be obtained | `service_unavailable` (503) for that attempt |
| Failure after a `*/start` transition | [Compensation](#compensation) → inbox `Failed` |

### Response Codes Seen by Citadel Consumers

| Code | Meaning |
|------|---------|
| 404 | Inbox not found |
| 409 | Invalid state / Barbican conflict |
| 502 | Barbican request failed (after retries) |
| 503 | Barbican circuit breaker open |
| 401/403 | Invalid JWT / insufficient role |

### Best Practice

- Keep retries bounded (never indefinite) — the job must fail after `CITADEL_BARBICAN_API_RETRY_MAX` attempts
- Distinguish transient retryable errors from conflicts (409)
- Log correlation/state transition results via `tracing`

---

## Monitoring

### Features

- **Retry metrics**: `citadel_barbican_api_errors_total` counter (registered in `metrics.rs`)
- **Circuit breaker gauge**: `citadel_circuit_breaker_state` (0=closed, 1=open); set by `BarbicanClient` when constructed `with_metrics`

### Suggested Alerts

- `citadel_barbican_api_errors_total` rate spikes
- `citadel_circuit_breaker_state == 1`
- Job failures caused by Barbican coordination (`job.status = Failed`)

---

*See also:*
- [Architecture](architecture.md) - System design
- [State Machine](state-machine.md) - Coordinated lifecycles
- [Configuration](configuration.md) - Environment variables
- [Barbican's Citadel Integration](../../barbican/src/docs/citadel-integration.md) - The other side
