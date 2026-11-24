# Citadel Integration

Documentation for the integration between Barbican and Citadel, including automatic triggers, progress reporting, and service-to-service communication.

## Table of Contents

- [Overview](#overview)
- [Architecture](#architecture)
- [Automatic Triggers](#automatic-triggers)
- [Progress Reporting](#progress-reporting)
- [Service-to-Service Communication](#service-to-service-communication)
- [Error Handling](#error-handling)
- [Configuration](#configuration)

---

## Overview

Barbican and Citadel work together to manage the complete inbox lifecycle:

- **Barbican**: Frontend service managing inbox metadata and API
- **Citadel**: Backend service handling physical file operations

**Key Integration Points:**
1. Barbican automatically triggers Citadel operations
2. Citadel reports progress back to Barbican
3. Citadel notifies Barbican of operation completion/failure
4. Both services share MongoDB inbox collection

---

## Architecture

### Integration Flow

```
┌─────────────┐      ┌─────────────┐      ┌─────────────┐
│   Client    │      │   Barbican  │      │   Citadel   │
└──────┬──────┘      └──────┬──────┘      └──────┬──────┘
       │                    │                    │
       │  1. Create Inbox   │                    │
       │───────────────────>│                    │
       │                    │                    │
       │  2. Accept (202)   │                    │
       │<───────────────────│                    │
       │                    │                    │
       │                    │  3. Trigger Create │
       │                    │───────────────────>│
       │                    │                    │
       │                    │  4. Create Directory
       │                    │                    │
       │                    │  5. Progress Update│
       │                    │<───────────────────│
       │                    │                    │
       │                    │  6. Complete       │
       │                    │<───────────────────│
       │                    │                    │
       │  7. Query Status   │                    │
       │───────────────────>│                    │
       │                    │                    │
       │  8. Return Status  │                    │
       │<───────────────────│                    │
       │                    │                    │
```

### Shared Data Model

Both services use the same MongoDB inbox collection:

```rust
pub struct InboxDB {
    pub _id: ObjectId,
    pub name: String,
    pub size: u64,
    pub status: InboxStatus,
    pub warehouse_path: String,
    pub inbox_path: Option<String>,
    pub lifetime: Option<u64>,
    pub archive_enabled: bool,
    pub archive_sites: Vec<String>,
    pub version: u32,  // Optimistic locking
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    
    // Progress tracking (updated by Citadel via Barbican)
    pub current_job_id: Option<String>,
    pub progress: Option<InboxProgress>,
    pub error: Option<InboxError>,
}
```

**Important:** Barbican owns all writes to the inbox collection. Citadel reads inbox metadata directly but must call Barbican's API for any state changes.

---

## Automatic Triggers

### Trigger Pattern

Barbican automatically triggers **Citadel** operations (the single outward
cross-service edge, D9) when external clients perform certain actions. The
triggers are `POST /api/v1/inbox/{id}/create|publish|delete` **on the Citadel
service** (per `barbican/src/clients/citadel.rs`); Citadel replies with a
`job_id` it generated. Barbican's own `create/start`, `publish/start`, etc.
action endpoints are invoked **by Citadel** (`BarbicanClient`), never by
Barbican.

| Client Action | Barbican Response | Citadel Trigger (on Citadel) |
|---------------|-------------------|------------------------------|
| `POST /api/v1/inbox` | 202 Accepted | `POST /api/v1/inbox/{id}/create` (body `{"warehouse_path": "..."}`) |
| `POST /api/v1/inbox/{id}/publish` | 202 Accepted | `POST /api/v1/inbox/{id}/publish` (no body) |
| `DELETE /api/v1/inbox/{id}` | 202 Accepted | `POST /api/v1/inbox/{id}/delete` (no body) |

All three triggers are **fire-and-forget** (`tokio::spawn`): the external
response is returned before Citadel acknowledges. If a trigger fails, the
asynchronous fallback (`apply_trigger_failure`, D2) transitions the inbox to
`Failed` so it never sits in `active`/`creating`/`publishing`/`deleting`
indefinitely.

### Implementation

```rust
// In handlers/create_inbox.rs — trigger fires after the insert is accepted
if let Some(ref citadel) = state.citadel_client {
    let warehouse_path = inbox.warehouse_path.clone();
    let citadel_clone = citadel.clone();
    let store = state.store.clone();
    let id = inserted_id;
    tokio::spawn(async move {
        match citadel_clone.trigger_create(&id_str, &warehouse_path).await {
            Ok(resp) => tracing::info!("Citadel create triggered: job_id={}", resp.job_id),
            Err(e) => {
                // D2 fallback: mark the inbox failed via the transition path
                apply_trigger_failure(&store, &id, "citadel_trigger_failed", &format!("{e}")).await;
            }
        }
    });
}
```

### Trigger Configuration

```bash
# Citadel API endpoint
BARBICAN_CITADEL_API_URL=http://citadel:8081

# Timeout for trigger calls
BARBICAN_CITADEL_API_TIMEOUT_SECS=30

# Max transport retries
BARBICAN_CITADEL_API_RETRY_MAX=3

# Service token for Citadel's role-gated routes (see Authentication below)
BARBICAN_CITADEL_CLIENT_ID=barbican
BARBICAN_CITADEL_CLIENT_SECRET=...
```

(`BARBICAN_CITADEL_TRIGGER_RETRY_MS` does **not** exist — see Retry Logic.)

### Retry Logic

Triggers use `CitadelClient::call_with_retry`:

- A transport failure (connect/timeout) is retried until
  `BARBICAN_CITADEL_API_RETRY_MAX` attempts have been made, with exponential
  backoff of 1 s, 2 s, 4 s, ... (`1000 * 2^(attempt-1)` ms; not
  configurable). On the final failure the error is logged and the D2
  fallback transitions the inbox.
- A **401** is answered once by invalidating the cached service token and
  re-sending with a fresh one; this retry is not counted as an attempt. A
  second 401 is returned as an error.
- Any other HTTP error status (4xx/5xx) is returned immediately.
- If no service token can be obtained (Keycloak unreachable, unreadable token
  file), the call fails immediately without retry.

```rust
// barbican/src/clients/citadel.rs — simplified shape
async fn call_with_retry(&self, url, body, operation) -> Result<StartOperationResponse, CitadelError> {
    let mut attempts = 0;
    let mut refreshed_token = false;
    loop {
        attempts += 1;
        // build_request attaches `provider.bearer().await?` when configured
        let response = self.build_request(url, body).await?.send().await;
        if is_401(&response) && !refreshed_token && let Some(provider) = &self.token_provider {
            provider.invalidate().await;
            refreshed_token = true;
            attempts -= 1;
            continue;
        }
        match response {
            Ok(resp) if resp.status().is_success() => return resp.json::<StartOperationResponse>().await,
            Ok(resp) => return Err(CitadelError::ApiError(resp.status(), resp.text().await.unwrap_or_default())),
            Err(e) if attempts >= self.config.citadel_api_retry_max => return Err(CitadelError::RequestFailed(...)),
            Err(_) => sleep(Duration::from_millis(1000 * 2u64.pow(attempts - 1))).await,
        }
    }
}
```
```

---

## Progress Reporting

### Progress Update Pattern

Citadel periodically reports progress to Barbican during long-running operations:

**Endpoint Pattern:**
```
POST /api/v1/inbox/{id}/{operation}/progress
```

**Request Body** (`create/progress`, `publish/progress`):
```json
{
  "job_id": "uuid-string",
  "phase": "copying",
  "total_files": 100,
  "processed_files": 45,
  "total_bytes": 1073741824,
  "transferred_bytes": 483183820
}
```

**Request Body** (`delete/progress`):
```json
{
  "job_id": "uuid-string",
  "phase": "deleting",
  "total_bytes": 1048576,
  "deleted_bytes": 524288
}
```

**Response:** `200 OK` with the updated inbox document (`InboxDB`), whose
`progress` field is the stored `InboxProgress`:
```json
{
  "status": "publishing",
  "current_job_id": "uuid-string",
  "progress": {
    "phase": "copying",
    "total_files": 100,
    "processed_files": 45,
    "total_bytes": 1073741824,
    "transferred_bytes": 483183820,
    "deleted_bytes": 0,
    "updated_at": "2024-01-01T00:00:00Z"
  }
}
```
(abridged — other inbox fields omitted)

### Stored Progress (`InboxProgress`)

`common/src/models.rs` defines exactly these fields; Barbican stores the
raw counters and computes **no** derived values (there is no
percent-complete field — clients derive it from the counters if needed):

| Field | Set by |
|-------|--------|
| `phase` | All progress endpoints |
| `total_files`, `processed_files` | All progress endpoints (optional, default 0) |
| `total_bytes` | All progress endpoints |
| `transferred_bytes` | create/publish progress (0 for delete) |
| `deleted_bytes` | delete progress (0 for create/publish) |
| `updated_at` | Server time of the update |

`progress` is cleared (`null`) on `*/complete` and `*/failure`.

### Delete Progress

`delete/progress` (documented in
[API Reference](api-reference.md#delete-progress)) takes `total_bytes` and
`deleted_bytes` (plus optional `total_files`/`processed_files`). Citadel's job-progress forwarder
(`citadel/src/handlers/job_progress.rs`) maps the worker's generic
bytes-processed counter (`transferred_bytes`) to `deleted_bytes`. The
`citadel-job` worker's delete path does **not** currently report progress,
so this endpoint is exercised only by tests today.

### Recommended Update Frequency

- **Progress Updates**: Every 30 seconds during active operations
- **Final Update**: Always send complete/failure notification
- **Idle Periods**: No updates needed when operation is paused

### Progress Phases

| Phase | Description | Operation |
|-------|-------------|-----------|
| `preparing` | Setting up directories, validating paths | create |
| `copying` | Transferring files to destination | publish |
| `verifying` | Checking file integrity after transfer | publish |
| `cleaning` | Removing source files after successful transfer | delete |
| `finalizing` | Updating metadata, completing operation | all |

---

## Service-to-Service Communication

### Authentication

Each direction uses a Keycloak service account; tokens come from the shared
`common::ServiceTokenProvider` (client-credentials grant, cached and refreshed
30 s before expiry, re-fetched once after a 401).

**Configuration:**
```bash
# Citadel → Barbican (Citadel side)
CITADEL_BARBICAN_API_URL=http://barbican:8080
CITADEL_BARBICAN_CLIENT_ID=citadel
CITADEL_BARBICAN_CLIENT_SECRET=...   # from a Kubernetes Secret

# Barbican → Citadel (Barbican side)
BARBICAN_CITADEL_CLIENT_ID=barbican
BARBICAN_CITADEL_CLIENT_SECRET=...   # from a Kubernetes Secret
```

**Citadel → Barbican** (`*/{start,progress,complete,failure}`):
- Citadel sends a Bearer token on every call. (Previously it sent none, so
  every call was rejected by Barbican's auth layer.)
- Barbican requires any valid JWT whose `aud` contains
  `BARBICAN_KEYCLOAK_CLIENT_ID`; **no role gating** (D9).

**Barbican → Citadel** (`POST /api/v1/inbox/{id}/{create,publish,delete}`):
- Citadel's inbox routes are role-gated: the token needs `citadel` in `aud`
  (audience mapper on the `barbican` client) and the `barbican` service
  account needs the `write` role.
- `BARBICAN_CITADEL_SERVICE_TOKEN_FILE` is a development-only alternative,
  re-read on every refresh; it is rejected when `PRODUCTION=true`. See
  [Configuration → Citadel service token](configuration.md#citadel-service-token).

> Since D9, there is **no `internal` role** and no
> `BARBICAN_INTERNAL_ROLES`. Barbican's 12 `*/{start,progress,complete,
> failure}` action endpoints are protected only by the generic auth layer
> (valid Bearer JWT); trust is delegated to Citadel, the only caller.

### HTTP Client

Barbican's Citadel client implementation:

```rust
pub struct CitadelClient {
    client: reqwest::Client,
    config: Config,
    /// `None` sends no Authorization header.
    token_provider: Option<Arc<ServiceTokenProvider>>,
}

impl CitadelClient {
    pub fn new(config: &Config, token_provider: Option<Arc<ServiceTokenProvider>>) -> Result<Self, CitadelError> { /* ... */ }

    pub async fn trigger_create(&self, inbox_id: &str, warehouse_path: &str) -> Result<StartOperationResponse, CitadelError> {
        let url = format!("{}/api/v1/inbox/{}/create", self.config.citadel_api_url, inbox_id);
        self.call_with_retry(&url, &CreateRequestBody { warehouse_path: warehouse_path.into() }, "create").await
    }
}
```

The `job_id` is **generated by Citadel** and returned in
`StartOperationResponse { job_id, message }`; Barbican's triggers never mint
their own job ids (D9).

### Network Considerations

- **Service Discovery**: Use Kubernetes service names or DNS
- **TLS**: Enable mutual TLS in production environments
- **Timeouts**: Configure appropriate timeouts (default: 30 seconds)
- **Retries**: Transport failures are retried with exponential backoff (see [Retry Logic](#retry-logic))
- **Circuit Breaker**: Consider circuit breaker pattern for resilience

---

## Error Handling

### Citadel Error Responses

When an operation fails **after** its Barbican `*/start` transition
succeeded, Citadel calls the matching `*/failure` endpoint
(`create_failure`/`publish_failure`/`delete_failure`) so the inbox does not
stay `creating`/`publishing`/`deleting`. Previously these early failures left
the inbox stuck. On the Citadel side this is best-effort compensation
(`citadel/src/handlers/compensate.rs`), which also marks a publish/delete job
document `failed`.

**Request:**
```http
POST /api/v1/inbox/{id}/create/failure
Authorization: Bearer <citadel_jwt>
Content-Type: application/json
```

**Body:**
```json
{
  "job_id": "uuid-string",
  "error_code": "storage_error",
  "error_message": "<Citadel error text>",
  "total_bytes": 0,
  "transferred_bytes": 0
}
```

`total_bytes`/`transferred_bytes` are optional (default 0). The response is
the updated inbox document (`status: "failed"`, `error.code` = the sent
`error_code`).

### Error Codes Sent by Citadel

`error_code` is a free-form string stored in `inbox.error.code`. Citadel
currently sends:

| Code | Endpoint(s) | When |
|------|-------------|------|
| `storage_error` | `create/failure` | The inbox directory could not be created |
| `barbican_error` | `create/failure` | Barbican rejected or did not answer `create/complete` |
| `executor_error` | `publish/failure`, `delete/failure` | The job could not be submitted to the executor |
| `publish_failed` | `publish/failure` | The publish job reported failure, or the poller found it terminated without a callback |
| `delete_failed` | `delete/failure` | Same, for delete jobs |

Barbican itself writes `citadel_trigger_failed` (D2 fallback, no endpoint
call) when its trigger to Citadel fails.

### Error Handling Strategy

1. **Log Error**: Citadel logs full error details
2. **Notify Barbican**: Call appropriate `/failure` endpoint
3. **Update State**: Barbican transitions inbox to `Failed`
4. **Alert Operators**: Monitoring system triggers alerts
5. **Manual Intervention**: Operators investigate and resolve

### Timeout Handling

**Citadel Responsibility:**
- Detect operation timeouts
- Report timeout as failure
- Clean up partial work

**Barbican Responsibility:**
- Record `progress.updated_at` on every progress update

Barbican does **not** implement stale-progress detection or SLA alerting;
staleness can be derived externally from `progress.updated_at` (e.g. no
update in 5 minutes while the inbox is `creating`/`publishing`/`deleting`).

---

## Configuration

### Barbican Configuration

```bash
# Citadel API endpoint
BARBICAN_CITADEL_API_URL=http://citadel:8081

# HTTP client timeout
BARBICAN_CITADEL_API_TIMEOUT_SECS=30

# Max transport retries
BARBICAN_CITADEL_API_RETRY_MAX=3

# Service account (client-credentials) for Barbican → Citadel triggers
BARBICAN_CITADEL_CLIENT_ID=barbican
BARBICAN_CITADEL_CLIENT_SECRET=...   # env.valueFrom.secretKeyRef
```

### Citadel Configuration

```bash
# Barbican API endpoint
CITADEL_BARBICAN_API_URL=http://barbican:8080

# Service account (client-credentials) for Citadel → Barbican calls
CITADEL_BARBICAN_CLIENT_ID=citadel
CITADEL_BARBICAN_CLIENT_SECRET=...

# Progress update interval
CITADEL_PROGRESS_UPDATE_INTERVAL_SECS=30

# Maximum job runtime (K8s activeDeadlineSeconds + poller timeout)
CITADEL_JOB_MAX_DURATION_SECS=86400
```

### Environment-Specific Settings

#### Development

```bash
BARBICAN_CITADEL_API_URL=http://localhost:8081
BARBICAN_CITADEL_API_TIMEOUT_SECS=60
BARBICAN_CITADEL_API_RETRY_MAX=5
```

#### Production

```bash
BARBICAN_CITADEL_API_URL=http://citadel.data-warehouse.svc.cluster.local:8081
BARBICAN_CITADEL_API_TIMEOUT_SECS=30
BARBICAN_CITADEL_API_RETRY_MAX=3
BARBICAN_CITADEL_CLIENT_ID=barbican
BARBICAN_CITADEL_CLIENT_SECRET=...   # env.valueFrom.secretKeyRef
```

---

## Monitoring and Observability

### Metrics to Track

**Barbican Side:**
- Citadel trigger success/failure rate
- Progress update frequency
- Operation completion time (P50, P95, P99)
- Error code distribution

**Citadel Side:**
- API response time from Barbican
- Token refresh frequency
- Retry attempt count
- Operation throughput

### Health Checks

**Barbican `/ready`** pings MongoDB only (`InboxStore::ping()`); it does
**not** check Citadel reachability or JWKS availability:

- `200 OK` — `{"status":"ready","mongodb":"connected"}`
- `503 Service Unavailable` — `{"status":"not_ready","mongodb":"disconnected"}`

See [API Reference](api-reference.md#ready).

### Logging

**Structured JSON logging recommended:**

```json
{
  "timestamp": "2024-01-01T00:00:00Z",
  "level": "INFO",
  "service": "barbican",
  "event": "citadel_trigger_success",
  "inbox_id": "507f1f77bcf86cd799439011",
  "operation": "create",
  "citadel_url": "http://citadel:8081",
  "duration_ms": 45
}
```

---

## Testing

### Test Layout

| Test | What it covers |
|------|----------------|
| `barbican/src/tests/citadel_client_tests.rs` | `CitadelClient` against a stub Citadel (axum on an ephemeral port) with a mock Keycloak from `common::test_utils::start_test_keycloak`: Bearer token sent, one refresh-and-retry on 401, persistent 401 is an error |
| `barbican/src/tests/handlers/transition_tests.rs` | The `*/{start,progress,complete,failure}` endpoints Citadel calls |
| `citadel/src/tests/barbican_client_tests.rs` | The other direction: Citadel's `BarbicanClient` token handling |
| `citadel/tests/compensation.rs` | Citadel calls `publish_failure` when job submission fails (MongoDB; `--features integration-tests`) |

### Example: stub Citadel

```rust
// Abridged from barbican/src/tests/citadel_client_tests.rs
let (url, stub) = start_stub(0).await;              // axum stub of Citadel
let kc = start_test_keycloak("test").await;         // JWKS + token endpoint
let provider = ServiceTokenProvider::new(ServiceTokenSource::ClientCredentials {
    token_url: kc.auth_config("x").token_url(),
    client_id: "barbican".into(),
    client_secret: SecretString::new(TEST_CLIENT_SECRET),
});
let config = Config { citadel_api_url: url, ..Config::default() };
let client = CitadelClient::new(&config, Some(Arc::new(provider))).unwrap();

client.trigger_publish("abc").await.unwrap();
assert!(stub.seen.lock().unwrap()[0].as_deref().unwrap().starts_with("Bearer "));
```

---

*See also:*
- [Architecture](architecture.md) - System design overview
- [State Machine](state-machine.md) - Inbox lifecycle
- [API Reference](api-reference.md) - Endpoint documentation
- [Configuration](configuration.md) - Environment variables