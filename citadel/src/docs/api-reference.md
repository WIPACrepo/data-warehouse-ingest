# Citadel API Reference

Complete documentation for all Citadel HTTP API endpoints.

## Table of Contents

- [Overview](#overview)
- [Authentication](#authentication)
- [Inbox Endpoints](#inbox-endpoints)
  - [Create Inbox Directory](#create-inbox-directory)
  - [Publish Inbox](#publish-inbox)
  - [Delete Inbox](#delete-inbox)
- [Job Endpoints](#job-endpoints)
  - [Get Job](#get-job)
  - [List Jobs](#list-jobs)
  - [Cancel Job](#cancel-job)
- [Job Callback Endpoint](#job-callback-endpoint)
- [Transfer Error & Audit Event Types](#transfer-error--audit-event-types)
- [Health Endpoints](#health-endpoints)
- [Error Responses](#error-responses)

---

## Overview

Citadel exposes a small, command-style HTTP API (rather than CRUD) for orchestrating inbox and job operations. All routes under `/api/v1/` require JWT authentication; `/health` and `/ready` are public.

The endpoints map to the following handlers (see [`src/handlers/`](../src/handlers/)):

| Method | Path | Handler | Role |
|--------|------|---------|------|
| `POST` | `/api/v1/inbox/{id}/create` | `create_inbox` | write |
| `POST` | `/api/v1/inbox/{id}/publish` | `publish_inbox` | write |
| `POST` | `/api/v1/inbox/{id}/delete` | `delete_inbox` | write |
| `GET` | `/api/v1/job/{job_id}` | `get_job` | read |
| `GET` | `/api/v1/jobs` | `list_jobs` | read |
| `POST` | `/api/v1/job/{job_id}/cancel` | `cancel_job` | admin |
| `POST` | `/api/v1/job/callback` | `job_callback` | timestamp+nonce |
| `GET` | `/health` | `health` | none |
| `GET` | `/ready` | `ready` | none |

### Base URL

```
http://<citadel-host>:8081
```

### API Version

All operational endpoints are versioned under `/api/v1/`.

> **Note**: This endpoint set reflects the current source code. The design in [`PLAN.md`](../PLAN.md) Section 1 describes the same set; the paths and roles match.

---

## Authentication

### JWT Token Requirements

All `/api/v1/` endpoints require a JWT in the `Authorization` header:

```http
Authorization: Bearer <jwt_token>
```

### Token Validation

Citadel validates JWT tokens with the following checks (see [`auth/jwt_validator.rs`](../src/auth/jwt_validator.rs)):

1. **Header algorithm**: must be in `CITADEL_ALLOWED_ALGORITHMS` (default `RS256`); `alg: none` is explicitly rejected
2. **Key ID**: the `kid` must match an entry in the Keycloak JWKS
3. **Signature**: verified using the JWKS public key
4. **Expiration**: must not be expired (leeway used)
5. **Issuer**: must match the configured Keycloak realm issuer
6. **Audience**: must match `CITADEL_KEYCLOAK_CLIENT_ID`
7. **Role gate** (middleware): the token must carry at least one role from `CITADEL_REQUIRED_ROLES` (default `read,write`)

### Role-Based Authorization

| Endpoint | Required Role(s) |
|----------|------------------|
| Create / Publish / Delete inbox | `write` or `admin` |
| Get / List jobs | `read` or `write` or `admin` |
| Cancel job | `admin` only |
| Job callback | timestamp + nonce (see below) |
| `/health`, `/ready` | none |

Roles are read from the `resource_access.citadel.roles` claim. Helper methods on `Claims`: `can_read()`, `can_write()`, `is_admin()`, `is_internal()`.

---

## Inbox Endpoints

### Create Inbox Directory

Creates the physical inbox directory for an existing inbox.

**Request:**
```http
POST /api/v1/inbox/{id}/create
Authorization: Bearer <jwt_token>
Content-Type: application/json
```

**Required Role:** `write` or `admin`

**Path Parameter:**
- `id` — the MongoDB `ObjectId` of the inbox

**Request Body:**
```json
{
  "warehouse_path": "/data/warehouse/example"
}
```

**Behavior:**
1. Fetches inbox metadata via the Barbican API
2. Validates the inbox doesn't already have an `inbox_path`
3. Creates `CITADEL_INBOX_BASE_PATH/{inbox_id}/`
4. Transitions the inbox state via the Barbican API

**Response (201 Created):**
```json
{
  "inbox_id": "507f1f77bcf86cd799439011",
  "inbox_path": "/data/inboxes/507f1f77bcf86cd799439011",
  "status": "active"
}
```

**Errors:**
- `404` — inbox not found (invalid ObjectId or Barbican lookup failed)
- `409` — inbox already has a path assigned
- `403` — insufficient role

---

### Publish Inbox

Starts an asynchronous publish job that moves inbox files into the protected warehouse.

**Request:**
```http
POST /api/v1/inbox/{id}/publish
Authorization: Bearer <jwt_token>
```

**Required Role:** `write` or `admin`

**Path Parameter:**
- `id` — the MongoDB `ObjectId` of the inbox

**Behavior:**
1. Fetches inbox metadata via the Barbican API
2. Validates the inbox has an `inbox_path` (i.e., it was created)
3. Creates a `PublishInbox` job record in MongoDB (status `queued`)
4. Transitions the inbox to `publishing` via the Barbican API
5. Submits the job to the executor; updates the job to `running` with the executor id

**Response (202 Accepted):**
```json
{
  "job_id": "58a3e4c0-...",
  "inbox_id": "507f1f77bcf86cd799439011",
  "status": "running",
  "job_type": "publish"
}
```

**Errors:**
- `404` — inbox not found
- `409` — inbox must be created before publishing (no `inbox_path`)
- `500`/`503` — job insertion or Barbican transition failed
- `403` — insufficient role

> **Note**: The actual file transfer is executed by `citadel-job` (K8s/process) and reported back via the [Job Callback](#job-callback-endpoint).

---

### Delete Inbox

Starts an asynchronous job that deletes the inbox directory.

**Request:**
```http
POST /api/v1/inbox/{id}/delete
Authorization: Bearer <jwt_token>
```

**Required Role:** `write` or `admin`

**Path Parameter:**
- `id` — the MongoDB `ObjectId` of the inbox

**Behavior:**
1. Fetches inbox metadata via the Barbican API
2. Validates the inbox has an `inbox_path`
3. Creates a `DeleteInbox` job record in MongoDB (status `queued`)
4. Transitions the inbox to `deleting` via the Barbican API
5. Submits the job to the executor; updates the job to `running` with the executor id

**Response (202 Accepted):**
```json
{
  "job_id": "58a3e4c0-...",
  "inbox_id": "507f1f77bcf86cd799439011",
  "status": "running",
  "job_type": "delete"
}
```

**Errors:** same shape as [Publish Inbox](#publish-inbox)

---

## Job Endpoints

### Get Job

Retrieves detailed status for a single job.

**Request:**
```http
GET /api/v1/job/{job_id}
Authorization: Bearer <jwt_token>
```

**Required Role:** `read`, `write`, or `admin`

**Path Parameter:**
- `job_id` — the unique job identifier (UUID string)

**Response (200 OK):**
```json
{
  "job_id": "58a3e4c0-...",
  "inbox_id": "507f1f77bcf86cd799439011",
  "job_type": "publish",
  "status": "running",
  "progress": {
    "total_files": 10,
    "processed_files": 5,
    "total_bytes": 1048576,
    "processed_bytes": 524288,
    "phase": "copying",
    "last_updated": "2024-01-01T00:05:00Z"
  },
  "errors": [],
  "result": null
}
```

`status` is the lowercase form of the [`JobStatus`](state-machine.md#job-statuses) enum (`queued`, `running`, `completed`, `failed`, `cancelling`, `cancelled`, `timed_out`).

**Errors:**
- `404` — job not found
- `403` — insufficient role

---

### List Jobs

Lists jobs with optional filtering.

**Request:**
```http
GET /api/v1/jobs?status=running&inbox_id=507f...&limit=100
Authorization: Bearer <jwt_token>
```

**Required Role:** `read`, `write`, or `admin`

**Query Parameters:**
- `status` — filter by job status string (optional)
- `inbox_id` — filter by inbox ObjectId (optional)
- `limit` — maximum results (default `100`)

**Response (200 OK):**
```json
{
  "jobs": [
    {
      "job_id": "58a3e4c0-...",
      "inbox_id": "507f1f77bcf86cd799439011",
      "job_type": "publish",
      "status": "completed",
      "created_at": "2024-01-01T00:00:00Z"
    }
  ]
}
```

Jobs are sorted by `created_at` descending.

---

### Cancel Job

Cancels a running (or queued) job.

**Request:**
```http
POST /api/v1/job/{job_id}/cancel
Authorization: Bearer <jwt_token>
```

**Required Role:** `admin` only

**Path Parameter:**
- `job_id` — the unique job identifier

**Behavior:**
1. Loads the job; rejects if it is `completed`, `cancelled`, `failed`, `timed_out`, or already `cancelling`
2. Sets the job to `cancelling`
3. Calls `executor.cancel()` (K8s delete / mock no-op)
4. Sets the job to `cancelled`

**Response (200 OK):**
```json
{
  "job_id": "58a3e4c0-...",
  "status": "cancelled"
}
```

**Errors:**
- `403` — requires `admin`
- `409` — job not in a cancellable state
- `404` — job not found

---

## Job Callback Endpoint

Receives completion/failure notifications from job workers (`citadel-job`).

**Request:**
```http
POST /api/v1/job/callback
Content-Type: application/json
X-Callback-Secret: <shared-secret>
```

**No JWT required** — authentication is via callback secret + timestamp + nonce (see [Callback Security](authentication.md#callback-security)).

**Request Body:**
```json
{
  "job_id": "58a3e4c0-...",
  "status": "completed",
  "timestamp": "2024-01-01T00:00:00Z",
  "nonce": "unique-random-string",
  "result": {
    "success": true,
    "files_transferred": 10,
    "total_bytes": 1048576
  }
}
```

**Behavior:**
1. **Callback secret validation** (when configured): `X-Callback-Secret` must match the configured secret using a **constant-time comparison**; missing/mismatch → `401 invalid_callback_secret`. When no secret is configured, a warning is logged and validation falls back to timestamp + nonce only.
2. **Timestamp validation**: `age < 5 minutes` with up to 1 minute clock skew allowed
3. **Nonce validation**: nonce must not have been seen before; stored with a 5-minute expiry
4. Loads the job; ignores callbacks for already-terminal jobs
5. On `completed` + `success`: sets `Completed`, records `JobResult`; notifies catalog then LTA (wired; `NoOp` implementations by default in `main.rs`)
6. On `completed` + `!success`: sets `Failed`
7. On `failed`: sets `Failed` and records a `JobError`
8. On `timed_out`: sets `TimedOut`

**Response (200 OK):**
```json
{
  "acknowledged": true
}
```

**Errors (401 Unauthorized):**
- `invalid_callback_secret`
- `callback_timestamp_expired`
- `callback_nonce_reused`

---

## Transfer Error & Audit Event Types

The recursive transfer engine (`storage/transfer.rs`) reports a structured
set of error types and emits typed security audit events. These are the
`error_type` metric labels (`citadel_transfer_errors_total`) and the
`SecurityEventType` values logged/forwarded to the audit sink. See
[Security Considerations](security-considerations.md) and
[Recursive Transfer](recursive-transfer.md).

### `TransferError` variants

| Variant | Meaning |
|---------|---------|
| `StorageError` | Generic storage/IO wrapper (path + message) |
| `FileTransferFailed` | A file copy/verify failed, or an unspecified transfer failure |
| `IoError` | Underlying `std::io::Error` |
| `SymlinkLoop` | Symlink loop/cycle detected (`PathBuf`) |
| `ExternalSymlinkNotAllowed` | Symlink resolves outside the inbox (`{ path }`) |
| `MaxRecursionDepthExceeded` | Directory nesting deeper than limit (`u32`) |
| `MaxSymlinkDepthExceeded` | Symlink chain longer than limit (`u32`) |
| `BrokenSymlink` | Symlink target missing (`PathBuf`) |
| `PathTooLong` | Combined warehouse path exceeds limit (`PathBuf`) |
| `MultipleErrors` | One or more collected sub-threshold errors (`Vec`) |
| `Cancelled` | Cancellation mechanism (filtered from reported errors) |
| `InvalidSymlinkTarget` | Target has null/control chars or `..` (`PathBuf`) |
| `SymlinkResolutionFailed` | Chain resolution failed (`PathBuf`) |
| `SecurityViolation` | Generic security violation (message) |
| `InboxIntegrityFailed` | Pre-transfer integrity scan failed (message) |
| `InsufficientDiskSpace` | Warehouse filesystem too small (`required`, `available`) |
| `InvalidWarehousePath` | Warehouse path escapes protected base (`PathBuf`) |
| `InboxLockFailed` | chmod lock failed (message) |
| `MaxSymlinkReferencesExceeded` | Symlink-farm inode reference limit (`ino`) |
| `MaxTotalFilesExceeded` | Total file count limit (`u64`) |
| `MaxTotalSymlinksExceeded` | Total symlink count limit (`u32`) |
| `TransferFailed` | **Wrapper**: all transfer failures return this with `{ errors, audit_events }` so the audit trail reaches callers |

### `SecurityEventType` (audit events)

`SymlinkLoopDetected`, `ExternalSymlinkRejected`, `MaxRecursionDepthExceeded`,
`MaxSymlinkDepthExceeded`, `InvalidSymlinkTarget`, `InboxIntegrityCheckFailed`,
`AtomicRollbackTriggered`, `InsufficientDiskSpace`,
`WarehousePathValidationFailed`, `InboxLockFailed`,
`MaxSymlinkReferencesExceeded`, `MaxTotalFilesExceeded`,
`MaxTotalSymlinksExceeded` — each with a severity (`Info`/`Warning`/`Error`/`Critical`),
structured `details`, and optional `inbox_id`/`job_id` context.

---

## Health Endpoints

### Health

Liveness probe. Public, no auth.

```http
GET /health
```

**Response (200 OK):**
```json
{
  "status": "healthy",
  "timestamp": "2024-01-01T00:00:00Z"
}
```

### Readiness

Readiness probe; verifies MongoDB connectivity. Public, no auth.

```http
GET /ready
```

**Response (200 OK):**
```json
{
  "status": "ready",
  "mongodb": "connected"
}
```

**Response (503 Service Unavailable):**
```json
{
  "status": "not_ready",
  "mongodb": "disconnected"
}
```

---

## Error Responses

All errors follow a consistent JSON format:

```json
{
  "error": "error_code",
  "message": "Human-readable message",
  "details": {}
}
```

### HTTP Status Codes

| Code | Meaning | Example |
|------|---------|---------|
| 400 | Bad Request | Invalid JSON, invalid request body |
| 401 | Unauthorized | Missing/invalid JWT, invalid callback timestamp/nonce |
| 403 | Forbidden | Insufficient role, path traversal attempt |
| 404 | Not Found | Inbox/job not found, file not found |
| 409 | Conflict | Invalid inbox state, job not cancellable, Barbican conflict |
| 500 | Internal Server Error | Storage/database/notification errors |
| 503 | Service Unavailable | K8s API down, Barbican API down |
| 504 | Gateway Timeout | Job exceeded max duration |

### Error Code Categories

| Category | Codes |
|----------|-------|
| Auth | `auth_error`, `invalid_jwt`, `missing_roles`, `forbidden` |
| Inbox | `inbox_not_found`, `invalid_inbox_state`, `inbox_path_exists` |
| Job | `job_not_found`, `job_completed`, `job_cancellation_failed`, `job_timeout` |
| Storage | `storage_error`, `file_not_found`, `file_transfer_failed`, `checksum_mismatch`, `path_traversal_attempt`, `invalid_base_path` |
| Database | `database_error` |
| Orchestration | `kubernetes_error`, `k8s_api_error` |
| Coordination | `barbican_api_error`, `barbican_conflict`, `barbican_unavailable` |
| Notifications | `catalog_notify_failed`, `lta_notify_failed` |
| Callback | `invalid_callback_secret`, `callback_timestamp_expired`, `callback_nonce_reused` |
| Generic | `config_error`, `http_error`, `json_error`, `io_error`, `internal_error` |

> **Security note**: Citadel logs full error detail server-side but maps responses to generic codes/messages for most errors (see [`error.rs`](../src/error.rs)) to avoid leaking internal paths, database structure, or stack traces.

---

*See also:*
- [Architecture](architecture.md) - System design
- [State Machine](state-machine.md) - Job lifecycle
- [Authentication](authentication.md) - JWT and callback security
- [Configuration](configuration.md) - Environment variables
