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
| Inbox reads | Both | Citadel reads metadata from the shared MongoDB (`inboxes`, read-only); state transitions via the Barbican API |
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
4. Handler reads inbox metadata directly from MongoDB
5. Handler validates the inbox has no `inbox_path` yet
6. Handler calls Barbican `create_start` (inbox → `creating`)
7. `StorageManager::create_inbox` creates the physical directory
8. Handler calls Barbican `create_complete`; if step 7 or 8 fails it compensates with `create_failure` (see [Barbican Integration](barbican-integration.md#compensation))
9. Handler returns `201 Created` with the `inbox_id` and `inbox_path`

### Publish Flow

1. Files are uploaded to the inbox directory out of band
2. Client calls `POST /api/v1/inbox/{id}/publish` with a `write` JWT
3. Handler reads inbox metadata from MongoDB and validates `inbox_path` is set
4. Handler creates a `PublishInbox` job record in MongoDB (`queued`)
5. Handler transitions the inbox to `publishing` via Barbican
6. Handler submits the job to the executor (`KubernetesExecutor` or `MockExecutor`); if submission fails it compensates (`publish_failure` + job `failed`)
7. Executor (K8s) launches a `citadel-job` process that performs the transfer
8. `citadel-job` executes recursive copy-verify-delete on the storage tree with the atomic transfer pattern
9. `citadel-job` calls back to `POST /api/v1/job/callback` with the result (`X-Callback-Secret` + timestamp + nonce; no JWT)
10. Handler updates the job record (`completed` / `failed`) and records the result

> **Note:** Downstream notifications to the catalog / LTA are triggered from the callback handler but the notifier implementations are `NoOp` stubs (see [Implementation Status](#implementation-status)).

### Delete Flow

1. Client calls `POST /api/v1/inbox/{id}/delete` with a `write` JWT
2. Handler reads inbox metadata from MongoDB and validates `inbox_path` is set
3. Handler creates a `DeleteInbox` job record in MongoDB (`queued`)
4. Handler transitions the inbox to `deleting` via Barbican
5. Handler submits the job to the executor; if submission fails it compensates (`delete_failure` + job `failed`)
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
- Sends progress reports and a completion callback to the Citadel API (`CITADEL_CALLBACK_URL`), minting a fresh timestamp + nonce for every send attempt
- Exits `0` on success, `1` on failure
- In production runs as a Kubernetes Job; in development it can be run directly or by the mock executor

---

## Module Structure

```
citadel/src/
├── main.rs              # citadel service entry point: tracing, config, Barbican token, create_app, metrics server
├── job_main.rs          # citadel-job binary entry point (transfer worker + callback)
├── lib.rs               # Library exports, AppState, create_app (router + per-group auth)
├── config.rs            # CitadelConfig (+ nested InboxConfig): CITADEL_* environment loading
├── error.rs             # Thin re-export of common::AppError (+ Result alias)
├── db.rs                # MongoDB client setup, collection helpers, collection name constants
├── models.rs            # JobDB, JobError, JobResult, CallbackNonceDB, FileInfo, TransferResult
├── metrics.rs           # CitadelMetrics: Prometheus registry + counters/histogram/gauge
├── secret_reloader.rs   # Callback secret file loading + notify-based hot reload
├── middleware/          # Re-exports (auth lives in common::auth)
│   ├── mod.rs           # Module exports
│   ├── auth.rs          # Re-exports common::auth_middleware / ClaimsExtractor
│   ├── job_auth.rs      # Job-route auth: admin JWT or citadel-job secret path (D11)
│   └── logging.rs       # Request logging middleware
├── handlers/            # HTTP handlers
│   ├── mod.rs           # Module exports
│   ├── create_inbox.rs  # POST /api/v1/inbox/{id}/create
│   ├── publish_inbox.rs # POST /api/v1/inbox/{id}/publish
│   ├── delete_inbox.rs  # POST /api/v1/inbox/{id}/delete
│   ├── get_job.rs       # GET /api/v1/job/{job_id}
│   ├── list_jobs.rs     # GET /api/v1/jobs
│   ├── cancel_job.rs    # POST /api/v1/job/{job_id}/cancel
│   ├── compensate.rs    # Best-effort *_failure + job `failed` after a post-start failure
│   ├── job_callback.rs  # POST /api/v1/job/callback (authenticate_job_caller; secret + timestamp + nonce)
│   ├── job_progress.rs  # POST /api/v1/job/progress (P4.1: progress forwarding → Barbican)
│   ├── health.rs        # GET /health
│   └── ready.rs         # GET /ready
├── jobs/                # Job model and execution
│   ├── mod.rs           # Module exports
│   ├── types.rs         # JobType, JobStatus, JobProgress, JobExecutorInfo, JobStatusDetail, Job
│   └── executor/
│       ├── mod.rs       # JobExecutor trait (submit/cancel/status)
│       ├── kubernetes.rs# KubernetesExecutor: K8s Job spec + submit/cancel/status
│       └── mock.rs      # MockExecutor: spawns local citadel-job processes for testing
├── storage/             # Filesystem operations
│   ├── mod.rs           # Module exports (transfer types re-exported)
│   ├── manager.rs       # StorageManager: create/delete inbox, transfer_files, validate_path
│   ├── transfer.rs      # Recursive transfer engine: lock → scan → collect (symlink-safe) →
│   │                    #   parallel copy+verify → atomic commit → cleanup; audit events
│   ├── checksum.rs      # SHA-512 checksum utilities
│   └── progress.rs      # ProgressTracker: wired into citadel-job + submitted through Citadel (P4.1)
├── integrations/        # External integrations
│   ├── mod.rs           # Module exports
│   ├── barbican.rs      # BarbicanClient: state transitions (retry/backoff + circuit breaker); InboxTransitioner trait
│   ├── catalog.rs       # CatalogNotifier trait + NoOp/HTTP implementations
│   └── lta.rs           # LtaNotifier trait + NoOp/HTTP implementations
├── polling/             # Background reconciliation
│   └── job_status.rs    # JobStatusPoller: active; K8s status, timeouts, terminal-state self-healing
└── docs/                # This documentation directory
```

### Shared Application State

All handlers receive the application state via Axum's `State` extractor:

```rust
pub struct AppState {
    pub config: Arc<CitadelConfig>,
    pub auth_state: Arc<AuthMiddlewareState>,
    pub storage_manager: Arc<StorageManager>,
    pub barbican_client: Arc<dyn InboxTransitioner>,
    pub executor: Arc<dyn JobExecutor>,
    pub catalog_notifier: Arc<dyn CatalogNotifier>,
    pub lta_notifier: Arc<dyn LtaNotifier>,
    pub metrics: Arc<CitadelMetrics>,
    pub db_client: Arc<Client>,
    pub callback_secret: Arc<RwLock<Option<String>>>, // hot-reloaded via notify
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
- Inbox trees are transferred **recursively**: subdirectories are preserved in
  the warehouse, symlinks are resolved safely (see
  [Recursive Transfer](recursive-transfer.md))

### Transfer Pipeline (`storage/transfer.rs`)

```
START
  │
  ▼
Validate warehouse path          // must stay inside CITADEL_PROTECTED_BASE_PATH
  │
  ▼
Lock inbox (chmod 555)           // InboxLock::lock — RAII restore on drop
  │
  ▼
Pre-transfer integrity scan?     // optional read-only security walk (fail fast)
  │  └► sums sizes ──► disk space check (statvfs, +10% buffer)
  ▼
Prepare target base              // atomic: {name}.transfer-{ms}-{rand} temp dir
  ▼
Producer (inline, backpressured) ────── bounded channel ──────► Copy workers
  │  collect_directory_tree:                                   (Semaphore =
  │  - active-chain (dev,ino) loop detection                       parallelism)
  │  - symlink chain resolution (read_link + lstat, depth-bounded)
  │  - multi-layer external check (canonical prefix + st_dev)
  │  - global inode reference counting (symlink farm)
  │  - depth / path-length / file / symlink limits
  │  - pre-creates warehouse dirs (empty subdirs preserved)
  ▼                                                        chunked copy + SHA-512
All files copied & verified                                verify on-disk target
  │                                                        atomic rename temp→target
  ├─ failure ─────────────────────────────────────► ROLLBACK: delete temp dir /
  │                                                 manifest files; inbox untouched;
  │                                                 return TransferFailed{errors,audit}
  ▼
COMMIT: rename temp dir → warehouse (atomic)   [atomic mode]
  │
  ▼
Delete sources + remove inbox
  │
  ▼
Return TransferResult (files, bytes, audit_events, sub-threshold errors)
```

Key safety invariants (see [`transfer.rs`](../storage/transfer.rs),
[Recursive Transfer](recursive-transfer.md), and
[Security Considerations](security-considerations.md)):

- **NEVER delete source files until ALL files are copied AND verified**
- **No partial warehouse data**: atomic mode writes to a sibling temp dir and
  commits with a single `rename()`; failure deletes the temp dir
- **Symlink safety**: loops detected via active-chain `(dev, ino)` tracking;
  external symlinks rejected by default (multi-layer canonical + `st_dev`
  check); chain resolution is depth-bounded and validated per hop (TOCTOU
  mitigation via non-following `read_link`/`lstat`)
- **Error collection**: up to `CITADEL_INBOX_MAX_ERRORS_BEFORE_FAIL` errors
  are collected and reported without aborting; the Nth cancels via a shared
  `CancellationToken`; structural failures (loops, limits) always fail
- **Chunked streaming**: files copied in `CITADEL_TRANSFER_CHUNK_SIZE_MB`
  chunks; per-file SHA-512 verified against the on-disk target after copy
- **Memory bound**: peak memory ≈ `CHUNK_SIZE_MB × PARALLELISM` per job

### Job Submission Pipeline

```
POST /api/v1/inbox/{id}/publish (or delete)
  │
  ▼
Validate JWT (write role)
  │
  ▼
Read inbox metadata (shared MongoDB, `inboxes` collection, read-only)
  │
  ▼
Validate state (inbox_path present)
  │
  ▼
Create JobDB record (jobs collection)  ──► status=Queued
  │
  ▼
Quota check (MongoDB in-flight count)  ──► 429 if at/above limit (D8)
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

**Decision**: Barbican API calls retry with exponential backoff up to `CITADEL_BARBICAN_API_RETRY_MAX` (default 5); a 409 conflict fails immediately; after `RETRY_MAX` consecutive failed calls a circuit breaker fails calls fast for 30 s.

**Rationale**: Infinite retry on a conflicted state transition risks hanging jobs and inconsistent inbox state.

### 6. Replay-Protected Callback Authentication

**Decision**: Job callbacks and progress reports carry a timestamp (validated: under 300 s old, at most 60 s in the future) and a UUID nonce stored in MongoDB (300 s TTL) to prevent replay. `citadel-job` mints a fresh pair for **every** send attempt, including retries, so jobs of any duration can report. When a callback secret is configured, the `X-Callback-Secret` header is verified with a **constant-time comparison** (`ct_eq`); the secret is hot-reloaded from file via the `notify` watcher. These checks apply to the worker path; an `admin` JWT caller relies on the JWT signature + `exp` instead (D11).

**Rationale**: The callback endpoint is the trust boundary for job results; replay protection prevents forged/duplicated completions, and the shared secret authenticates the job caller (constant-time compare avoids timing side channels).

### 7. Recursive Transfer with Symlink Safety

**Decision**: Directory tree traversal with active-chain `(dev, ino)` loop detection, multi-layer external symlink validation (canonical prefix + `st_dev`), TOCTOU mitigation via non-following `read_link`/`lstat` chain resolution, streaming collection over a bounded channel with backpressure, and the atomic transfer pattern (temp dir + rename, rollback on failure).

**Rationale**: Inboxes are an untrusted-content boundary; symlinks are the primary attack vector (loops, exfiltration, farms, races). Security-hardened defaults deny external links and bound all resources. See [Recursive Transfer](recursive-transfer.md), [Symlink Limitations](symlink-limitations.md), and [Security Considerations](security-considerations.md).

### 8. Shared Auth Parsing, Service-Owned Prefixes (D10)

**Decision**: JWT/Keycloak configuration is parsed once in `common` (`AuthConfig::from_env_prefixed(prefix)` + `build_auth_state`), while each service owns its variable namespace (`CITADEL_KEYCLOAK_URL`, `CITADEL_REQUIRED_ROLES`, ...). `CITADEL_KEYCLOAK_URL` includes any path prefix (e.g. `/auth` for Keycloak < 17) instead of `/auth/` being hard-coded. See [Configuration](configuration.md#keycloak-authentication).

**Rationale**: Both services validate tokens identically, misconfigurations fail at startup with the variable named, and two services in one environment cannot accidentally share (or shadow) each other's settings — previously Citadel read unprefixed `KEYCLOAK_*` names, so its documented `CITADEL_KEYCLOAK_*` variables were ignored.

### 9. Dual Job-Route Auth + Service-to-Service Tokens (D11)

**Decision**:
- `/api/v1/job/callback` and `/api/v1/job/progress` sit outside the user JWT layer and accept either a Bearer JWT with Citadel `admin` or, with no `Authorization` header, the `citadel-job` credentials (`X-Callback-Secret` + `timestamp` + `nonce`). A present header is authoritative (no fall-through). See [Job Route Security](authentication.md#job-route-security).
- Citadel authenticates to Barbican with a Keycloak client-credentials token (`CITADEL_BARBICAN_CLIENT_ID` / `CITADEL_BARBICAN_CLIENT_SECRET`), cached, refreshed before expiry, and re-fetched once on a `401`. See [Barbican Integration](barbican-integration.md#authentication-to-barbican).
- Router construction lives in `citadel::create_app(AppState) -> Router` so the auth wiring is tested at the router level.

**Rationale**: Both paths were broken in production: the callback routes were inside the user JWT layer, so every `citadel-job` callback (which has no Keycloak identity, D9) was rejected with `401`, and Citadel sent no `Authorization` header, so every Citadel → Barbican call was rejected with `401`. Job pods keep the shared-secret edge; operators get an audited admin path for manual job actions; the no-fall-through rule stops a low-privilege JWT from being combined with the secret.

### 10. Compensation After a Started Transition

**Decision**: If an operation fails after its Barbican `*/start` transition, Citadel issues the matching `*_failure` (and, for publish/delete, marks the job `failed`) best-effort and returns the original error — but never after a successful `executor.submit`. See [Barbican Integration](barbican-integration.md#compensation).

**Rationale**: Without it the inbox stays `creating`/`publishing`/`deleting` and the job counts toward the in-flight quota forever; compensating a live job would mark a running transfer failed.

The full decision ledger lives in [Implementation Status](#implementation-status) and the per-topic documents above.

---

## Implementation Status

The source tree implements the full core framework plus the recursive,
symlink-safe transfer engine. Everything below is as it stands in the code
today.

### Implemented

- HTTP server with routing, `/health`, `/ready` (axum 0.8)
- JWT validation with JWKS caching, algorithm whitelist, `alg: none` rejection
- Role-based authorization (`admin` / `write` / `read`; there is no `internal` role — D9)
- Auth config parsed by `common::AuthConfig::from_env_prefixed("CITADEL")` (D10)
- Job routes accept an admin JWT or the `citadel-job` secret path; router built by `create_app` (D11)
- Citadel → Barbican service token (client credentials, cached, refresh-on-401) (D11)
- Compensation (`*_failure` + job `failed`) after post-start failures in create/publish/delete
- MongoDB connectivity and `jobs` / `callback_nonces` collection helpers
- Job model (`JobDB`); per-type in-flight quota (`count_in_flight_jobs`, 429 when saturated — D8)
- Barbican client (`InboxTransitioner`) with exponential-backoff retry, 409 conflict handling, and a circuit breaker (30 s half-open)
- Storage manager with path traversal validation
- **Recursive file transfer engine**: chmod-555 lock, recursive tree walk with
  symlink loop detection (active-chain `(dev, ino)`), multi-layer external
  symlink validation, TOCTOU-safe chain resolution, producer/consumer
  pipeline over a bounded channel, parallel chunked copy, SHA-512 verify,
  per-file atomic rename, temp-dir + rename **atomic commit**, manifest
  rollback, delete-after-verify
- **Inbox security configuration** (`InboxConfig` nested in `CitadelConfig`):
  external-symlink policy, recursion/symlink-depth, path-length,
  file/symlink count, and symlink-farm limits, error threshold, atomic
  transfer toggle, pre-transfer integrity scan
- **Security audit events** (`SecurityAuditEvent`) emitted for every
  security-relevant incident and carried to callers on success *and* failure
- **Pre-transfer integrity scan** (read-only fail-fast) + disk-space check
  (`statvfs`, 10% buffer)
- `citadel-job` worker binary with environment-driven config (numeric
  settings strictly validated) and callback reporting
- Callback handler with **secret** (constant-time) + timestamp + nonce
  validation, job status updates, catalog/LTA notify hooks
- **Kubernetes executor**: real K8s Job spec creation, `submit`/`cancel`/`status`
- **Mock executor**: spawns local `citadel-job` processes for end-to-end tests
- **Catalog / LTA notifiers**: `NoOp` + `Http` implementations (wired as
  `NoOp` in `main.rs` by default)
- **Background job poller** (`polling/job_status.rs`): K8s status polling,
  timeout detection, missed-callback reconciliation, job cleanup
- **Callback secret hot-reload** via the `notify` file watcher
- Sanitized error responses with mapping to generic HTTP error codes
- Prometheus metrics (jobs, transfer files/bytes/errors/duration/rollbacks,
  security events, Barbican errors, callback errors)

### Known Gaps / Not Fully Wired

- **Catalog / LTA notifiers** are `NoOp` in the running service by default
  (`main.rs` constructs `NoOpCatalogNotifier` / `NoOpLtaNotifier`); the HTTP
  implementations exist but are not enabled without configuration.
- **Delete progress**: `execute_delete` in `citadel-job` does not send
  progress reports (the forwarding path to Barbican `delete/progress` exists).

The list above is the authoritative implementation-status log; keep it in
sync with the code.

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
3. **Job routes** (callback/progress): admin JWT, or (no `Authorization` header) `X-Callback-Secret` (constant-time compare, hot-reloaded) + timestamp + nonce validation (D11)
4. **Outbound to Barbican**: client-credentials service token with `barbican` in `aud` (D11)
5. **MongoDB**: native authentication/TLS (environment-dependent)

### Authorization Model

| Endpoint Pattern | Required Roles |
|------------------|----------------|
| `POST /api/v1/inbox/{id}/create` | `write` or `admin` |
| `POST /api/v1/inbox/{id}/publish` | `write` or `admin` |
| `POST /api/v1/inbox/{id}/delete` | `write` or `admin` |
| `GET /api/v1/job/{job_id}`, `GET /api/v1/jobs` | `read`, `write`, or `admin` |
| `POST /api/v1/job/{job_id}/cancel` | `admin` |
| `POST /api/v1/job/callback`, `POST /api/v1/job/progress` | `admin` JWT, or no JWT + callback secret + timestamp + nonce |
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
- [Recursive Transfer](recursive-transfer.md) - Transfer engine and symlink safety
- [Symlink Limitations](symlink-limitations.md) - Edge cases and accepted limits
- [Security Considerations](security-considerations.md) - Threat model and hardening
