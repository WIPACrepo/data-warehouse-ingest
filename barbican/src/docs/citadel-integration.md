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
```

(`BARBICAN_CITADEL_TRIGGER_RETRY_MS` does **not** exist — see Retry Logic.)

### Retry Logic

Triggers use `CitadelClient::call_with_retry`: the request is retried up to
`BARBICAN_CITADEL_API_RETRY_MAX` times **only when the request transport
fails** (connect/timeout); on the final failure the error is logged and the
D2 fallback transitions the inbox. There is no backoff knob — each retry is
attempted immediately. An HTTP error status from Citadel (4xx/5xx) is not
retried.

```rust
// barbican/src/clients/citadel.rs — simplified shape
async fn call_with_retry(&self, url, body, operation) -> Result<StartOperationResponse, CitadelError> {
    let mut attempts = 0;
    loop {
        attempts += 1;
        let response = self.client.post(url).json(body).bearer_auth(token?).send().await;
        match response {
            Ok(resp) if resp.status().is_success() => return resp.json::<StartOperationResponse>().await,
            Ok(resp) => return Err(CitadelError::ApiError(resp.status(), resp.text().await.unwrap_or_default())),
            Err(e) if attempts >= self.config.citadel_api_retry_max => return Err(CitadelError::RequestFailed(...)),
            Err(_) => { /* sleep 1s, retry */ }
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

Citadel authenticates to Barbican using a JWT service account:

**Configuration:**
```bash
# On Citadel side — Citadel → Barbican is the only outward cross-service path
CITADEL_BARBICAN_API_URL=http://barbican:8080

# On Barbican side — Barbican → Citadel triggers use Bearer service_token
BARBICAN_CITADEL_SERVICE_TOKEN_FILE=/etc/barbican/citadel.jwt
```

**Token Requirements (Barbican → Citadel triggers):**
- Any valid (non-expired) Bearer `service_token`; **no role gating** (D9)
- Signed by the trusted Keycloak instance

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
    service_token: Option<String>,
}

impl CitadelClient {
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
- **Retries**: Implement exponential backoff for transient failures
- **Circuit Breaker**: Consider circuit breaker pattern for resilience

---

## Error Handling

### Citadel Error Responses

When Citadel operations fail, it notifies Barbican:

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
  "error_code": "DIRECTORY_CREATE_FAILED",
  "error_message": "Permission denied creating /data/inboxes/abc123",
  "total_bytes": 0,
  "transferred_bytes": 0
}
```

**Response:**
```json
{
  "inbox_id": "507f1f77bcf86cd799439011",
  "status": "failed",
  "error": "DIRECTORY_CREATE_FAILED"
}
```

### Error Code Categories

| Category | Error Codes | Description |
|----------|-------------|-------------|
| Directory | `DIRECTORY_CREATE_FAILED`, `DIRECTORY_NOT_FOUND` | File system errors |
| Transfer | `TRANSFER_FAILED`, `FILE_NOT_FOUND`, `INSUFFICIENT_SPACE` | File operation errors |
| Permission | `PERMISSION_DENIED`, `ACCESS_FORBIDDEN` | Authorization errors |
| Validation | `INVALID_PATH`, `CHECKSUM_MISMATCH` | Data integrity errors |
| System | `INTERNAL_ERROR`, `TIMEOUT`, `SERVICE_UNAVAILABLE` | Infrastructure errors |

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

# Service account token (Bearer) for Barbican → Citadel triggers
BARBICAN_CITADEL_SERVICE_TOKEN_FILE=/etc/barbican/citadel.jwt
```

### Citadel Configuration

```bash
# Barbican API endpoint
CITADEL_BARBICAN_API_URL=http://barbican:8080

# Service account token
CITADEL_BARBICAN_SERVICE_TOKEN_FILE=/etc/citadel/barbican.jwt

# Progress update interval
CITADEL_PROGRESS_UPDATE_INTERVAL_SECS=30

# Operation timeouts
CITADEL_CREATE_TIMEOUT_SECS=300
CITADEL_PUBLISH_TIMEOUT_SECS=600
CITADEL_DELETE_TIMEOUT_SECS=300
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
BARBICAN_CITADEL_SERVICE_TOKEN_FILE=/run/secrets/citadel-jwt
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

### Integration Test Strategy

1. **Mock Citadel**: Use mock server for unit tests
2. **Test Environment**: Deploy both services for integration tests
3. **Chaos Testing**: Simulate Citadel failures and timeouts
4. **Load Testing**: Verify performance under high trigger volume

### Example Test

```rust
#[tokio::test]
async fn test_citadel_trigger_on_create() {
    // Setup mock Citadel server
    let mock_citadel = MockServer::start().await;
    
    mock_citadel.mock(|when, then| {
        when.method(POST)
            .path("/api/v1/inbox/abc123/create");
        then.status(200)
            .json_body(json!({"job_id": "citadel-job-123", "message": "create initiated"}));
    });
    
    // Create test Barbican app with mock Citadel URL
    let config = Config {
        citadel_api_url: mock_citadel.uri(),
        ..Default::default()
    };
    
    let app = create_test_app(config).await;
    
    // Trigger inbox creation
    let response = app
        .post("/api/v1/inbox")
        .json(&CreateInboxRequest { /* ... */ })
        .send()
        .await;
    
    assert_eq!(response.status(), 202);
    
    // Verify Citadel was triggered
    mock_citadel.assert_mocked();
}
```

---

*See also:*
- [Architecture](architecture.md) - System design overview
- [State Machine](state-machine.md) - Inbox lifecycle
- [API Reference](api-reference.md) - Endpoint documentation
- [Configuration](configuration.md) - Environment variables