# Citadel Documentation

Welcome to the Citadel crate documentation. This directory contains comprehensive guides and references for understanding and working with Citadel — the backend service that controls the protected data areas of the data warehouse.

## Table of Contents

### Core Documentation

| Document | Description |
|----------|-------------|
| [Architecture](architecture.md) | System overview, component diagram, and module structure |
| [API Reference](api-reference.md) | Complete endpoint documentation with request/response examples |
| [State Machine](state-machine.md) | Job lifecycle, statuses, and inbox state coordination |
| [Authentication](authentication.md) | JWT validation, roles, and callback security |
| [Barbican Integration](barbican-integration.md) | Frontend service coordination from the Citadel side |
| [Configuration](configuration.md) | Environment variables and settings |
| [Development Guide](development.md) | Building, testing, and extending Citadel |
| [Recursive Transfer](recursive-transfer.md) | Transfer engine, symlink safety, atomic commit, error collection |
| [Symlink Limitations](symlink-limitations.md) | Edge cases, bind mounts, TOCTOU, accepted risks |
| [Security Considerations](security-considerations.md) | Threat model, hardening guide, incident response, compliance |

---

## Quick Start

**What is Citadel?**

Citadel is the backend service in the data warehouse ingestion system. It is responsible for the protected data areas of the warehouse and is not meant to be called directly by end users — user-facing traffic flows through [Barbican](../barbican/src/docs/README.md).

Citadel consists of **two binaries** built from the same crate:

| Binary | Purpose | Runs as |
|--------|---------|---------|
| `citadel` | Long-running HTTP service: API, job orchestration, queueing, integrations | Deployment/Pod |
| `citadel-job` | One-shot file transfer worker: copy-verify-delete, callback reporting | Kubernetes Job or local process |

**Key Responsibilities:**
- Create temporary inbox directories for inbox metadata
- Move data from inboxes into protected areas (copy-verify-then-delete-all)
- Delete / clean up inbox directories after successful transfer
- Run file transfers asynchronously as jobs, via a pluggable executor
- Notify the file catalog of newly controlled files
- Notify the Long Term Archive (LTA) of data that should be archived
- Authenticate all API calls via Keycloak JWTs and enforce role-based authorization

**Core Design Principles:**
- **Ownership Boundary**: Barbican owns inbox metadata (MongoDB); Citadel owns physical operations (filesystem, jobs). Citadel reads inbox metadata but transitions state **only** via the Barbican API.
- **Copy-Verify-Delete**: Source files are never deleted until every file is copied *and* SHA-512 verified.
- **Async Jobs**: File operations run as jobs (publish/delete) rather than blocking HTTP requests.
- **Pluggable Executor**: The same job model runs on Kubernetes Jobs (production) or local processes (mock, testing).
- **Fail-Fast Coordination**: Barbican API failures after retries, or conflicts, fail the job immediately.
- **Defense in Depth**: Path-traversal prevention, algorithm-confusion prevention, callback replay protection.

---

## Architecture Overview

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

See [Architecture](architecture.md) for the detailed component breakdown.

---

## Module Structure

```
citadel/src/
├── main.rs              # citadel service binary entry point, router, metrics server
├── job_main.rs          # citadel-job binary entry point (file transfer worker)
├── lib.rs               # Library exports and AppState
├── config.rs            # CITADEL_* environment configuration loading
├── error.rs             # Thin re-export of common::AppError (+ Result alias)
├── db.rs                # MongoDB client setup and collection helpers
├── models.rs            # JobDB, CallbackNonceDB, TransferResult, FileInfo
├── metrics.rs           # Prometheus metrics registry and collectors
├── middleware/          # Re-exports (auth lives in common::auth)
│   ├── auth.rs          # Re-exports common::auth_middleware / ClaimsExtractor
│   └── logging.rs       # Request logging middleware
├── handlers/            # HTTP handlers (create/publish/delete inbox, job CRUD, callback, progress, health)
├── jobs/
│   ├── types.rs         # JobType, JobStatus, JobProgress, JobExecutorInfo
│   └── executor/
│       ├── mod.rs       # JobExecutor trait
│       ├── kubernetes.rs# K8s executor: job spec + submit/cancel/status
│       └── mock.rs      # Mock executor (spawns local citadel-job processes)
├── storage/
│   ├── manager.rs       # create_inbox, delete_inbox, transfer_files, validate_path
│   ├── transfer.rs      # copy-verify-delete transfer engine, InboxLock (chmod 555)
│   ├── checksum.rs      # SHA-512 checksum utilities
│   └── progress.rs      # ProgressTracker: wired into citadel-job + submitted through Citadel (P4.1)
├── integrations/
│   ├── barbican.rs      # BarbicanClient (state transitions, retry/backoff + circuit breaker)
│   ├── catalog.rs       # CatalogNotifier trait + NoOp implementation
│   └── lta.rs           # LtaNotifier trait + NoOp implementation
├── polling/
│   └── job_status.rs    # JobStatusPoller: active — status polling, timeouts, terminal-state self-healing
└── docs/                # This documentation directory
```

---

## Common Workflows

### Creating an Inbox Directory

1. Barbican creates inbox metadata (`status: active`) and triggers Citadel
2. User calls `POST /api/v1/inbox/{id}/create` with `{"warehouse_path": "..."}` (JWT with `write`)
3. Citadel fetches inbox metadata via the Barbican API
4. Citadel creates the physical directory under `CITADEL_INBOX_BASE_PATH/{inbox_id}`
5. Citadel transitions the inbox state via the Barbican API

### Publishing an Inbox

1. User uploads files to the inbox directory (out of band, via normal methods)
2. User calls `POST /api/v1/inbox/{id}/publish` (JWT with `write`)
3. Citadel validates the inbox has an `inbox_path`
4. Citadel creates a `publish` job in MongoDB and transitions the inbox to `publishing`
5. Citadel submits the job to the executor (Kubernetes or mock)
6. The executor's `citadel-job` performs the transfer and calls back
7. Citadel updates the job record and records the transfer result

### Deleting an Inbox

1. User calls `POST /api/v1/inbox/{id}/delete` (JWT with `write`)
2. Citadel creates a `delete` job in MongoDB and transitions the inbox to `deleting`
3. Citadel submits the job to the executor, which cleans up the directory

See [API Reference](api-reference.md) for detailed endpoint documentation and [State Machine](state-machine.md) for the job lifecycle.

---

## Additional Resources

- [Barbican Documentation](../../barbican/src/docs/README.md) - Frontend service documentation
- [Common Crate Documentation](../../common/src/docs/README.md) - Shared types and utilities

---

*Last Updated: 2026-08-26*
