# Barbican Integration

Documentation for the integration between Citadel and Barbican, from the Citadel side — the coordination model, the Barbican API client, retry strategy, and inbox state ownership.

## Table of Contents

- [Overview](#overview)
- [Coordination Model](#coordination-model)
- [Barbican API Client](#barbican-api-client)
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
1. Citadel reads inbox metadata via the Barbican API (`get_inbox`)
2. Citadel requests inbox state transitions via the Barbican API (`transition_inbox_state`)
3. Both services share the MongoDB inbox collection (Citadel reads only; Barbican owns writes)

> The mirror document on the Barbican side is [`barbican/src/docs/citadel-integration.md`](../../barbican/src/docs/citadel-integration.md).

---

## Coordination Model

### Who Owns What

| Concern | Owner | Citadel's behavior |
|---------|-------|--------------------|
| Inbox metadata | Barbican | Reads via API; never writes directly |
| Inbox state transitions | Barbican | Requests via API (`/api/v1/inbox/{id}/transition`) |
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

The client lives at [`integrations/barbican.rs`](../src/integrations/barbican.rs).

### Methods

| Method | API call | Purpose |
|--------|----------|---------|
| `transition_inbox_state(&id, new_state)` | `POST /api/v1/inbox/{id}/transition` | Request a state transition (retry + backoff) |
| `get_inbox(&id)` | `GET /api/v1/inbox/{id}` | Fetch inbox metadata |

### Construction

```rust
pub struct BarbicanClient {
    config: CitadelConfig,
    client: reqwest::Client,
}

impl BarbicanClient {
    pub fn new(config: CitadelConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            config,
        }
    }
}
```

The client is created in `main.rs` and shared through `AppState.barbican_client` (`Arc<BarbicanClient>`).

### Transition Handling

`do_transition` posts to the transition endpoint and interprets responses:

- **409 Conflict** → returns `CitadelError::BarbicanConflict` — fail immediately, no retry
- **Other non-success** → returns `CitadelError::BarbicanApiError`
- **Success** → parses and returns the updated `InboxDB`

---

## Retry Strategy

### Exponential Backoff

`transition_inbox_state` retries transient errors with exponential backoff:

```
Attempt 1: immediate
Attempt 2: +backoff_ms        (default 1000ms)
Attempt 3: +2×backoff_ms
...
Attempt N: +... up to 30s max  (CITADEL_BARBICAN_API_RETRY_BACKOFF_MS, doubling, capped at 30s)
```

- **Max attempts**: `CITADEL_BARBICAN_API_RETRY_MAX` (default 5)
- **Timeouts**: `CITADEL_BARBICAN_API_TIMEOUT_SECS` (default 10s)

### Rules

| Outcome | Behavior |
|---------|----------|
| Transient error | Retry with backoff |
| Retries exhausted | Return error (job/operation fails) |
| **409 Conflict** | Fail immediately — no retry |

> **Planned / per PLAN.md**: circuit breaker for the Barbican client (open after 10 consecutive failures, 5-minute cooldown). The `citadel_circuit_breaker_state` gauge exists in metrics but no breaker logic is wired in yet.

---

## Inbox State Ownership

### Design Principle

Barbican is the **single writer** for inbox state. Citadel never calls MongoDB directly to change inbox status — it requests transitions so that Barbican can apply business rules, optimistic locking, and audit logging.

### Transitions Citadel Requests

| Operation | Handler | State string | Valid from (Barbican) |
|-----------|---------|--------------|------------------------|
| Create directory | `create_inbox` | `active` | inbox with no `inbox_path` |
| Publish | `publish_inbox` | `publishing` | inbox with `inbox_path` (Ready) |
| Delete | `delete_inbox` | `deleting` | inbox with `inbox_path` |

### State Change Flow

```
Citadel handler
  │
  ▼
barbican_client.transition_inbox_state(&id, "publishing")
  │  POST /api/v1/inbox/{id}/transition
  ▼
Barbican validates + applies (optimistic lock on version)
  │
  ├─ 200 OK ─────────────► proceed with physical operation
  ├─ 409 Conflict ───────► fail immediately (no retry)
  └─ 5xx/timeout ────────► retry (exponential backoff, max 5)
                            │
                            └─ exhausted ─► fail operation
```

### Reading Inbox State

Handlers call `barbican_client.get_inbox` before acting to validate preconditions:

- `create_inbox`: rejects if `inbox_path.is_some()`
- `publish_inbox` / `delete_inbox`: rejects if `inbox_path.is_none()`

---

## End-to-End Flows

### Create → Ready

```
User ──POST /api/v1/inbox──► Barbican (status: Active)
                                 │ auto-trigger
                                 ▼
User ──POST /inbox/{id}/create──► Citadel
                                 │ get_inbox → validate no path
                                 │ StorageManager::create_inbox
                                 │ transition("active") via Barbican
                                 ▼
                           201 Created { inbox_id, inbox_path }
```

### Publish

```
User uploads files (out of band)
  │
User ──POST /inbox/{id}/publish──► Citadel
  │  validate inbox_path present (get_inbox)
  │  create PublishInbox JobDB (jobs collection)
  │  transition("publishing") via Barbican
  │  executor.submit(job) → Running
  ▼
citadel-job: storage::transfer (lock → copy+verify → delete sources)
  │
  ▼
Callback POST /api/v1/job/callback → Completed / Failed
  └── (planned) notify catalog → notify LTA
```

### Delete

```
User ──POST /inbox/{id}/delete──► Citadel
  │  validate inbox_path present
  │  create DeleteInbox JobDB
  │  transition("deleting") via Barbican
  │  executor.submit(job) → Running
  ▼
worker removes inbox directory → callback → Deleted (via Barbican)
```

---

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `CITADEL_BARBICAN_API_URL` | `http://barbican:8080` | Barbican API base URL |
| `CITADEL_BARBICAN_API_RETRY_MAX` | `5` | Max retries for state transitions |
| `CITADEL_BARBICAN_API_RETRY_BACKOFF_MS` | `1000` | Initial backoff (doubles, capped 30s) |
| `CITADEL_BARBICAN_API_TIMEOUT_SECS` | `10` | Per-request timeout |

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
```

---

## Error Handling

### Failure Modes

| Failure | Behavior |
|---------|----------|
| Barbican API unreachable | Retry with backoff; after `RETRY_MAX` → operation fails |
| 409 Conflict | Fail immediately (no retry) |
| Inbox not found | `InboxNotFound` (404) |
| Inbox in wrong state | `InvalidInboxState` (409) |
| Drop/network timeout | Per-request timeout; retry |

### Response Codes Seen by Citadel Consumers

| Code | Meaning |
|------|---------|
| 404 | Inbox not found |
| 409 | Invalid state / Barbican conflict |
| 503 | Barbican API unavailable (after retries) |
| 401/403 | Invalid JWT / insufficient role |

### Best Practice

- Keep retries bounded (never indefinite) — PLAN.md mandates failing the job after `CITADEL_BARBICAN_API_RETRY_MAX`
- Distinguish transient retryable errors from conflicts (409)
- Log correlation/state transition results via `tracing`

---

## Monitoring

### Features

- **Retry metrics**: `citadel_barbican_api_errors_total` counter (registered in `metrics.rs`)
- **Circuit breaker gauge** (planned): `citadel_circuit_breaker_state` (0=closed, 1=open)

### Suggested Alerts

- `citadel_barbican_api_errors_total` rate spikes
- `citadel_circuit_breaker_state == 1` (planned)
- Job failures caused by Barbican coordination (`job.status = Failed`)

---

*See also:*
- [Architecture](architecture.md) - System design
- [State Machine](state-machine.md) - Coordinated lifecycles
- [Configuration](configuration.md) - Environment variables
- [Barbican's Citadel Integration](../../barbican/src/docs/citadel-integration.md) - The other side
- [Design Plan](../PLAN.md) Section 6 - MongoDB integration and coordination
