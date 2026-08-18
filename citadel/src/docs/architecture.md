# Citadel Architecture

This document describes the system architecture, component relationships, and module structure of the Citadel service.

## Table of Contents

- [System Overview](#system-overview)
- [High-Level Architecture](#high-level-architecture)
- [Component Flow](#component-flow)
- [Binaries](#binaries)
- [Module Structure](#module-structure)
- [Data Flow](#data-flow)
- [Key Design Decisions](#key-design-decisions)
- [Implementation Status](#implementation-status)

---

## System Overview

**Citadel** is the backend service in the data warehouse ingestion system. It controls the protected data areas of the data warehouse and is not meant to be called by end users — user-facing traffic flows through [Barbican](../../barbican/src/docs/README.md) (the frontend).

### Primary Responsibilities

1. **Inbox Directory Management**: Create temporary inbox directories for inbox metadata
2. **File Transfers**: Move data from inboxes into protected areas (copy-verify-then-delete-all)
3. **Cleanup**: Delete / clean up inbox directories after successful transfer
4. **Job Orchestration**: Run transfers asynchronously as jobs via a pluggable executor
5. **Downstream Notifications**: Notify the file catalog and the Long Term Archive (LTA)
6. **Barbican Coordination**: Transition inbox state strictly through the Barbican API

### Ownership Boundary

Barbican and Citadel share the same MongoDB inbox collection, but ownership is split:

| Concern | Owner | Notes |
|---------|-------|-------|
| Inbox metadata CRUD | Barbican | Single source of truth for inbox state |
| Inbox MongoDB writes | Barbican | Citadel never writes to the inbox collection directly |
| Inbox reads | Both | Citadel reads via the Barbican API and/or MongoDB |
| Inbox state transitions | Barbican (via API) | Citadel calls Barbican to transition state |
| Physical directories | Citadel | Inbox base path, protected base path |
| Jobs collection (`jobs`) | Citadel | Citadel-owned collection |
| Callback nonces (`callback_nonces`) | Citadel | Citadel-owned collection |

---

## High-Level Architecture

```
+-------------+      JWT      +-------------+
|  Barbican   | ------------> |  Citadel    |
|  (Frontend) |               |  (Backend)  |
+-------------+               +------+------+
        |                            |
        | (shared MongoDB)           |
        +------------+---------------+
                     |
        +------------+------------+
        |            |            |
        v            v            v
+-------------+ +-----------+ +------------+
|  MongoDB    | |  POSIX    | |  Executor  |
|(inboxes+jobs)| | Storage   | |(K8s/mock)  |
+-------------+ +-----------+ +------------+
                                       |
                              +--------+--------+
                              |                 |
                              v                 v
                       +-----------+    +------------+
                       | citadel-job|   | citadel-job|
                       | (prod K8s) |   | (test proc)|
                       +-----------+    +------------+
                              |                 |
                              +--------+--------+
                                       |
                              (callback to Citadel API)
```

### Architecture Principles

- **Separation of Concerns**: Barbican handles metadata and the user-facing API; Citadel handles physical operations
- **Shared Data Model**: Both services share the MongoDB inbox collection; Barbican owns all writes
- **Async Job Execution**: Transfer and delete operations run as jobs, decoupled from HTTP requests
- **Pluggable Execution**: The same job model runs on Kubernetes (production) or local mock processes (testing)
- **Fail-Fast Coordination**: Barbican API retries are bounded; conflicts fail the job immediately
- **Data Integrity First**: Copy-verify-delete-all; source files are never deleted before verification

### External Dependencies

| Dependency | Purpose | Usage |
|------------|---------|-------|
| MongoDB | Inbox, job, and nonce persistence | `jobs`, `callback_nonces` (owned); `inboxes` (read) |
| POSIX filesystem | Inbox and protected storage | `CITADEL_INBOX_BASE_PATH`, `CITADEL_PROTECTED_BASE_PATH` |
| Keycloak | JWT issuance and JWKS endpoint | Token validation, role extraction |
| Kubernetes (prod) | Job execution | `citadel-job` pods, job status polling |
| Barbican API | Inbox state coordination | State transitions, inbox metadata reads |
| File Catalog API | *Planned* | Notification of newly controlled files |
| LTA API | *Planned* | Notification of data to archive |

---

## Component Flow

### Inbox Creation Flow

1. Barbican creates inbox metadata (`status: active`) and triggers Citadel
2. Client calls `POST /api/v1/inbox/{id}/create` with a `warehouse_path`
3. Citadel auth middleware validates the JWT and `write` role
4. Handler fetches inbox metadata via the Barbican API
5. Handler validates the inbox has no `inbox_path` yet
6. `StorageManager::create_inbox` creates the physical directory
7. Handler transitions the inbox state via the Barbican API
8. Handler returns `201 Created` with the `inbox_id` and `inbox_path`

### Publish Flow

1. Files are uploaded to the inbox directory out of band
2. Client calls `POST /api/v1/inbox/{id}/publish` with a `write` JWT
3. Handler fetches inbox metadata and validates `inbox_path` is set
4. Handler creates a `PublishInbox` job record in MongoDB (`queued`)
5. Handler transitions the inbox to `publishing` via Barbican
6. Handler submits the job to the executor (`KubernetesExecutor` or `MockExecutor`)
7. Executor (K8s) launches a `citadel-job` process that performs the transfer
8. `citadel-job` executes copy-verify-delete on the storage
9. `citadel-job` calls back to `POST /api/v1/job/callback` with the result
10. Handler updates the job record (`completed` / `failed`) and records the result

> **Note:** Downstream notifications to the catalog / LTA are triggered from the callback handler but the notifier implementations are `NoOp` stubs (see [Implementation Status](#implementation-status)).

### Delete Flow

1. Client calls `POST /api/v1/inbox/{id}/delete` with a `write` JWT
2. Handler fetches inbox metadata and validates `inbox_path` is set
3. Handler creates a `DeleteInbox` job record in MongoDB (`queued`)
4. Handler transitions the inbox to `deleting` via Barbican
5. Handler submits the job to the executor
6. The worker cleans up the inbox directory

### Job Status Flow

1. Client calls `GET /api/v1/job/{job_id}` or `GET /api/v1/jobs` with a `read` JWT
2. Handler queries the `jobs` collection and returns a summary

### Job Cancellation Flow (admin only)

1. Admin calls `POST /api/v1/job/{job_id}/cancel` with an `admin` JWT
2. Handler validates the job isn't in a terminal state
3. Handler sets the job to `cancelling`, calls `executor.cancel()`, then marks it `cancelled`

---

## Binaries

Both binaries are built from the same `citadel` crate (see [`Cargo.toml`](../Cargo.toml)).

### `citadel` — HTTP Service

- Long-lived main service
- Serves the API on `CITADEL_BIND_ADDRESS` (default `127.0.0.1:8081`)
- Serves Prometheus metrics on `CITADEL_PROMETHEUS_PORT` (default `9090`)
- Owns: job queueing, executor selection, Barbican coordination, callback handling

### `citadel-job` — Transfer Worker

- One-shot command-line worker (`src/job_main.rs`)
- Reads job parameters from environment variables (`JOB_ID`, `JOB_TYPE`, `INBOX_ID`, `INBOX_PATH`, `WAREHOUSE_PATH`, etc.)
- Executes the file transfer via the shared `storage::transfer` module
- Sends a completion callback to the Citadel API (`CITADEL_CALLBACK_URL`)
- Exits `0` on success, `1` on failure
- In production runs as a Kubernetes Job (see [`PLAN.md`](../PLAN.md) Section 5.3); in development it can be run directly or by the mock executor

---

## Module Structure

```
citadel/src/
├── main.rs              # citadel service entry point: tracing, config, router, metrics server
├── job_main.rs          # citadel-job binary entry point (transfer worker + callback)
├── lib.rs               # Library exports and AppState (shared dependencies)
├── config.rs            # CitadelConfig: CITADEL_* environment loading, defaults
├── error.rs             # CitadelError, Result, StatusCode mapping, sanitized IntoResponse
├── db.rs                # MongoDB client setup, collection helpers, collection name constants
├── models.rs            # JobDB, JobError, JobResult, CallbackNonceDB, FileInfo, TransferResult
├── metrics.rs           # CitadelMetrics: Prometheus registry + counters/histogram/gauge
├── auth/                # JWT authentication
│   ├── mod.rs           # Module exports
│   ├── jwt_validator.rs # CustomJwks, Claims, JwksCache, validate_jwt, AuthState
├── middleware/          # Axum middleware
│   ├── mod.rs           # Module exports
│   ├── auth.rs          # auth_middleware: JWT extraction, validation, role gate
│   └── logging.rs       # Request logging middleware
├── handlers/            # HTTP handlers
│   ├── mod.rs           # Module exports
│   ├── create_inbox.rs  # POST /api/v1/inbox/{id}/create
│   ├── publish_inbox.rs # POST /api/v1/inbox/{id}/publish
│   ├── delete_inbox.rs  # POST /api/v1/inbox/{id}/delete
│   ├── get_job.rs       # GET /api/v1/job/{job_id}
│   ├── list_jobs.rs     # GET /api/v1/jobs
│   ├── cancel_job.rs    # POST /api/v1/job/{job_id}/cancel
│   ├── job_callback.rs  # POST /api/v1/job/callback
│   ├── health.rs        # GET /health
│   └── ready.rs         # GET /ready
├── jobs/                # Job model and execution
│   ├── mod.rs           # Module exports
│   ├── types.rs         # JobType, JobStatus, JobProgress, JobExecutorInfo, JobStatusDetail, Job
│   ├── queue.rs         # JobQueue: publish/delete semaphores with rate limits
│   ├── processor.rs     # JobProcessor: permit acquisition + executor submission
│   └── executor/
│       ├── mod.rs       # JobExecutor trait (submit/cancel/status)
│       ├── kubernetes.rs# KubernetesExecutor (stub: returns not-implemented errors)
│       └── mock.rs      # MockExecutor (returns immediately; used for testing)
├── storage/             # Filesystem operations
│   ├── mod.rs           # Module exports
│   ├── manager.rs       # StorageManager: create/delete inbox, transfer_files, validate_path
│   ├── transfer.rs      # transfer_files: lock (chmod 555) → parallel copy+verify → delete sources
│   ├── checksum.rs      # SHA-512 checksum utilities
│   ├── lock.rs          # Inbox lock helper
│   └── progress.rs      # Transfer progress tracking
├── integrations/        # External integrations
│   ├── mod.rs           # Module exports
│   ├── barbican.rs      # BarbicanClient: transition_inbox_state, get_inbox (retry/backoff)
│   ├── catalog.rs       # CatalogNotifier trait + NoOpCatalogNotifier
│   ├── lta.rs           # LtaNotifier trait + NoOpLtaNotifier
│   └── kubernetes.rs    # K8s job integration (stub: create_job_spec not implemented)
├── polling/             # Background reconciliation
│   └── job_status.rs    # JobStatusPoller (stub: log-only loop)
└── docs/                # This documentation directory
```

### Shared Application State

All handlers receive the application state via Axum's `State` extractor:

```rust
pub struct AppState {
    pub config: Arc<CitadelConfig>,
    pub jwks_cache: Arc<JwksCache>,
    pub storage_manager: Arc<StorageManager>,
    pub barbican_client: Arc<BarbicanClient>,
    pub executor: Arc<dyn JobExecutor>,
    pub catalog_notifier: Arc<dyn CatalogNotifier>,
    pub lta_notifier: Arc<dyn LtaNotifier>,
    pub metrics: Arc<CitadelMetrics>,
    pub db_client: Arc<Client>,
}
```

See [`lib.rs`](../lib.rs) for the full definition.

---

## Data Flow

### Storage Layout

```
CITADEL_INBOX_BASE_PATH/     (default ./data/inboxes)
└── {inbox_id}/              # Physical inbox directory, named by ObjectId
    └── files...             # Uploaded content

CITADEL_PROTECTED_BASE_PATH/ (default ./data/protected)
└── {warehouse_path}         # Protected destination for published data
    └── files...             # Transferred content (verified)
```

- Inbox directories are created by `StorageManager::create_inbox` using the inbox `ObjectId` as the directory name
- Protected destinations are derived from the `warehouse_path` in inbox metadata
- All paths are validated with `validate_path` (canonicalization) to prevent traversal

### Transfer Pipeline (`storage/transfer.rs`)

```
START
  │
  ▼
Lock inbox (chmod 555)          // InboxLock::lock — read-only during transfer
  │
  ▼
Enumerate files                 // fs::read_dir, files only
  │
  ▼
Parallel copy + verify          // Semaphore-limited; SHA-512 computed during streaming copy
  │                              // atomic rename after verification
  ├─ any failure ──────────────► Clean up transferred targets, preserve sources, return failure
  │
  ▼
All verified                    // only now are sources touched
  │
  ▼
Delete source files             // one removal per transferred file
  │
  ▼
Return TransferResult
```

Key safety invariants (see [`transfer.rs`](../src/storage/transfer.rs) and [`PLAN.md`](../PLAN.md) Section 4):

- **NEVER delete source files until ALL files are copied AND verified**
- **Copy and verify run in parallel** per-file, bounded by `CITADEL_TRANSFER_PARALLELISM`
- **On failure**: preserve all source files, clean up partial targets
- **Chunked streaming**: files are copied in chunks (`CITADEL_TRANSFER_CHUNK_SIZE_MB`, default 16MB)
- **Atomic writes**: each file is copied to a temp path then renamed
- **Memory bound**: peak memory ≈ `CHUNK_SIZE_MB × PARALLELISM` per job

### Job Submission Pipeline

```
POST /api/v1/inbox/{id}/publish (or delete)
  │
  ▼
Validate JWT (write role)
  │
  ▼
Fetch inbox metadata (Barbican API)
  │
  ▼
Validate state (inbox_path present)
  │
  ▼
Create JobDB record (jobs collection)  ──► status=Queued
  │
  ▼
Transition inbox state (Barbican API)   ──► publishing/deleting
  │
  ▼
executor.submit(&job)                    ──► Kubernetes (K8s Job) | Mock (no-op)
  │
  ▼
Update job: executor_id + status=Running
  │
  ▼
Worker runs; callback → /api/v1/job/callback
  │
  ▼
Update job: Completed / Failed / TimedOut
```

---

## Key Design Decisions

### 1. Separate Metadata Owner (Barbican) from Physical Owner (Citadel)

**Decision**: Barbican owns inbox metadata CRUD; Citadel owns physical operations. Citadel coordinates *all* inbox state changes through the Barbican API rather than writing to the shared collection directly.

**Rationale**: A single source of truth for inbox state avoids split-brain updates, and the API boundary makes coordination auditable (`barbican_client` with retries).

### 2. Async, Executor-Agnostic Jobs

**Decision**: Publish/delete operations are queued as jobs and executed by a pluggable `JobExecutor` (Kubernetes for production, mock for tests).

**Rationale**: Long-running file operations shouldn't block HTTP requests; abstracting execution allows the same service to run in a real cluster and on a laptop.

### 3. Copy-Verify-Delete-All

**Decision**: Source files are only removed after every file has been copied **and** SHA-512 verified.

**Rationale**: Prevents data loss from premature deletion even if individual files fail mid-transfer.

### 4. Parallelism Bounded by Env-Covered Knobs

**Decision**: Parallelism and chunk size are configurable (`CITADEL_TRANSFER_PARALLELISM`, `CITADEL_TRANSFER_CHUNK_SIZE_MB`), giving a predictable memory bound.

**Rationale**: Tune throughput without risking OOM on large inboxes.

### 5. Fail-Fast Barbican Coordination

**Decision**: Barbican API calls retry with exponential backoff up to `CITADEL_BARBICAN_API_RETRY_MAX` (default 5); a 409 conflict fails immediately.

**Rationale**: Infinite retry on a conflicted state transition risks hanging jobs and inconsistent inbox state.

### 6. Replay-Protected Callback Authentication

**Decision**: Job callbacks carry a timestamp (validated within 5 minutes, allowing 1 minute clock skew) and a per-call nonce stored in MongoDB to prevent replay.

**Rationale**: The callback endpoint is the trust boundary for job results; replay protection prevents forged/duplicated completions.

See [`PLAN.md`](../PLAN.md) Section 14 (Success Criteria) for the full decision ledger.

---

## Implementation Status

The current source tree implements the core framework; several `PLAN.md` features are stubs or `NoOp` implementations. Everything below is as it stands in the code today.

### Implemented

- HTTP server with routing, `/health`, `/ready` (axum 0.8)
- JWT validation with JWKS caching, algorithm whitelist, `alg: none` rejection
- Role-based authorization (`admin` / `write` / `read` / `internal`)
- MongoDB connectivity and `jobs` / `callback_nonces` collection helpers
- Job model (`JobDB`), per-type queue semaphores, processor
- Barbican client with exponential-backoff retry and 409 conflict handling
- Storage manager with path traversal validation
- File transfer engine: chmod-555 lock, parallel chunked copy, SHA-512 verify, atomic rename, delete-after-verify
- `citadel-job` worker binary with environment-driven config and callback reporting
- Callback handler with timestamp + nonce validation, job status updates
- Sanitized error responses with mapping to generic HTTP error codes
- Prometheus metrics registry (jobs, transfer bytes/files, Barbican errors, callback errors, circuit-breaker gauge)

### Planned / per PLAN.md (not fully implemented)

- **Kubernetes executor**: `submit`/`cancel`/`status` return "not fully implemented" errors
- **Kubernetes job spec creation** (`integrations/kubernetes.rs` `create_job_spec`): returns not-implemented
- **Mock executor process spawning**: `MockExecutor` returns immediately rather than spawning `citadel-job`
- **Catalog / LTA notifiers**: only `NoOp` implementations; the callback handler's notification calls are commented out (including the catalog-before-LTA ordering)
- **Background job poller** (`polling/job_status.rs`): log-only loop; no K8s API polling, timeout detection, or reconciliation
- **Callback secret validation**: callback handler relies on timestamp + nonce only; secret-file header validation is a `TODO`
- **Metadata polling during transfer**: not present in the transfer path
- **Open-handle verification** before/after locking: not present (lock is chmod + RAII restore)
- **Circuit breaker**: gauge defined in metrics but no breaker logic in the Barbican client
- **Secret hot-reload** (`notify` crate dependency present; no watcher wired up)
- **TTL index** on `callback_nonces.expires_at`: collection helper exists; index creation not wired

> See [`HANDOFF.md`](../HANDOFF.md) for the authoritative implementation-status log and next steps.

---

## Scalability Considerations

### Horizontal Scaling

- **Stateless Design**: Citadel instances share state via MongoDB; instances can be scaled
- **Shared MongoDB**: `jobs` and `callback_nonces` are instance-agnostic
- **Callback replay protection works across replicas**: nonces are stored in MongoDB (shared)
- **Executor Cluster**: Kubernetes Jobs provide parallel, distributed execution

### Performance Optimizations

- **JWKS Caching**: 60-second refresh (configurable), 1/minute rate limit
- **Parallel Transfers**: semaphore-bounded concurrency (default 4)
- **Connection Pooling**: reqwest and MongoDB connection reuse

### Failure Handling

- **Retry Logic**: Barbican API calls use exponential backoff (bounded)
- **Replay Protection**: timestamp + nonce prevent callback replay
- **Health Endpoints**: `/health` and `/ready` for load balancer checks
- **Sanitized Errors**: generic HTTP codes returned to clients; full details logged server-side

---

## Security Architecture

### Authentication Layers

1. **External clients** (via Barbican): JWT with user roles (`read`, `write`, `admin`)
2. **Service callers** of the Citadel API: JWT validated against Keycloak JWKS
3. **Job callback**: timestamp + nonce validation (planned: shared file-based secret)
4. **MongoDB**: native authentication/TLS (environment-dependent)

### Authorization Model

| Endpoint Pattern | Required Roles |
|------------------|----------------|
| `POST /api/v1/inbox/{id}/create` | `write` or `admin` |
| `POST /api/v1/inbox/{id}/publish` | `write` or `admin` |
| `POST /api/v1/inbox/{id}/delete` | `write` or `admin` |
| `GET /api/v1/job/{job_id}`, `GET /api/v1/jobs` | `read`, `write`, or `admin` |
| `POST /api/v1/job/{job_id}/cancel` | `admin` |
| `POST /api/v1/job/callback` | timestamp + nonce (planned: internal role / secret) |
| `/health`, `/ready` | None (public) |

### Security Measures in Code

- **Algorithm Whitelist**: `CITADEL_ALLOWED_ALGORITHMS`; `alg: none` explicitly rejected
- **No HMAC Fallback**: only asymmetric keys (RSA/EC/EdDSA) from JWKS are used
- **Path Traversal Prevention**: `validate_path` canonicalizes and checks prefix containment
- **Input Sanitization**: error responses map internal details to generic codes
- **Replay Prevention**: nonce uniqueness enforced via MongoDB insert-check

---

*See also:*
- [API Reference](api-reference.md) - Endpoint documentation
- [State Machine](state-machine.md) - Job lifecycle
- [Authentication](authentication.md) - JWT and role details
- [Barbican Integration](barbican-integration.md) - Frontend coordination
- [Design Plan](../PLAN.md) - Complete specification
