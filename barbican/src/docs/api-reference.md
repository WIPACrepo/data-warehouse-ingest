# API Reference

Complete documentation for all Barbican HTTP API endpoints.

## Table of Contents

- [Overview](#overview)
- [Authentication](#authentication)
- [External API Endpoints](#external-api-endpoints)
  - [Create Inbox](#create-inbox)
  - [Get Inbox](#get-inbox)
  - [List Inboxes](#list-inboxes)
  - [Delete Inbox](#delete-inbox)
  - [Publish Inbox](#publish-inbox)
- [Internal API Endpoints](#internal-api-endpoints)
  - [Create Operation](#create-operation)
  - [Publish Operation](#publish-operation)
  - [Delete Operation](#delete-operation)
- [Health Endpoints](#health-endpoints)
- [Error Responses](#error-responses)

---

## Overview

Barbican provides a RESTful HTTP API for managing inbox metadata and coordinating data ingestion operations. The API is divided into:

- **External Endpoints**: Client-facing operations for inbox management
- **Internal Endpoints**: Action-based callbacks from Citadel
- **Health Endpoints**: Service health and readiness probes

All endpoints return JSON responses with appropriate HTTP status codes.

### Base URL

```
http://<barbican-host>:8080
```

### API Version

All endpoints are versioned under `/api/v1/`.

---

## Authentication

### JWT Token Requirements

Most endpoints require JWT authentication via the `Authorization` header:

```http
Authorization: Bearer <jwt_token>
```

### Token Validation

Barbican validates JWT tokens with the following checks:

1. **Signature**: Verified against Keycloak JWKS
2. **Expiration**: Token must not be expired (leeway `BARBICAN_TOKEN_LEEWAY_MS`, default 5000 ms)
3. **Issuer**: Must equal `{BARBICAN_KEYCLOAK_URL}/realms/{BARBICAN_KEYCLOAK_REALM}`
4. **Audience**: `aud` must contain `BARBICAN_KEYCLOAK_CLIENT_ID` (default `barbican`)
5. **Roles**: Must contain required role for the endpoint

### JWKS Caching

- JWKS fetched from Keycloak and cached
- Refresh interval: `BARBICAN_JWKS_REFRESH_SECS` (default and minimum 60 seconds)
- Immediate refresh on unknown key ID (rate-limited)

---

## External API Endpoints

### Create Inbox

Creates a new inbox metadata record and automatically triggers Citadel to create the physical directory.

**Request:**
```http
POST /api/v1/inbox
Authorization: Bearer <jwt_token>
Content-Type: application/json
```

**Required Role:** `write` or `admin`

**Request Body:**
```json
{
  "name": "example-inbox",
  "size": 1048576,
  "warehouse_path": "/data/warehouse/example",
  "lifetime": 86400,
  "archive_enabled": true,
  "archive_sites": ["site-a", "site-b"]
}
```

**Response (202 Accepted):**
```json
{
  "inbox_id": "507f1f77bcf86cd799439011",
  "status": "active",
  "message": "Inbox creation initiated"
}
```

The record is inserted in the `active` state and the Citadel `create` trigger
fires asynchronously (fire-and-forget). A duplicate `warehouse_path` is
rejected: the fast in-process check returns `400`, and the unique index on
`warehouse_path` turns a concurrent race (or a soft-deleted duplicate) into a
`409 Conflict` (P1.5/D3).

---

### Get Inbox

Retrieves detailed information about a specific inbox.

**Request:**
```http
GET /api/v1/inbox/{id}
Authorization: Bearer <jwt_token>
```

**Required Role:** `read`, `write`, or `admin`

**Response (200 OK):**
```json
{
  "inbox_id": "507f1f77bcf86cd799439011",
  "name": "example-inbox",
  "size": 1048576,
  "status": "ready",
  "warehouse_path": "/data/warehouse/example",
  "inbox_path": "/data/inboxes/507f1f77bcf86cd799439011",
  "created_at": "2024-01-01T00:00:00Z",
  "updated_at": "2024-01-01T00:00:00Z"
}
```

---

### List Inboxes

Retrieves a list of all inboxes with optional filtering.

**Request:**
```http
GET /api/v1/inbox
Authorization: Bearer <jwt_token>
```

**Required Role:** `read`, `write`, or `admin`

**Query Parameters:**
- `status` - Filter by status (optional)
- `limit` - Maximum results (default: 100)
- `offset` - Pagination offset (default: 0)

**Response (200 OK) — pagination envelope:**
```json
{
  "inboxes": [ ... ],
  "total": 50,
  "limit": 100,
  "offset": 0,
  "has_more": false
}
```

`total` is the count matching the same `status` filter (`count_documents`);
`has_more = offset + len(inboxes) < total`. `offset` replaces the previously
documented `skip` parameter (P1.4).

---

### Delete Inbox

Requests deletion of an inbox and triggers Citadel cleanup.

**Request:**
```http
DELETE /api/v1/inbox/{id}
Authorization: Bearer <jwt_token>
```

**Required Role:** `write` or `admin`

**Response (202 Accepted):**
```json
{
  "inbox_id": "507f1f77bcf86cd799439011",
  "status": "deleting",
  "message": "Inbox deletion initiated"
}
```

---

### Publish Inbox

Initiates transfer of inbox contents to the protected warehouse.

**Request:**
```http
POST /api/v1/inbox/{id}/publish
Authorization: Bearer <jwt_token>
```

**Required Role:** `write` or `admin`

**No request body** — the publish operation takes no payload (the request
must simply be `POST`ed; `Content-Type` is not required).

**Response (202 Accepted):**
```json
{
  "inbox_id": "507f1f77bcf86cd799439011",
  "status": "publishing",
  "message": "Inbox publish operation initiated"
}
```

---

## Internal API Endpoints

Internal endpoints follow the action pattern: `{operation}/{action}` where
action is one of `start`, `progress`, `complete`, `failure`. They are invoked
**only by Citadel** (through `Citadel`'s `BarbicanClient`) as part of the D9
coordination model — Barbican's external routes never call them.

**Authentication:** the generic auth layer (valid Bearer JWT); there is **no
`internal` role gate** — trust is delegated to Citadel, which is the only
caller in a well-deployed network. (An agent-grade mutual-auth boundary is a
separate, dedicated task; see [Authentication](authentication.md).)

All actions validate the expected prior state and use optimistic locking
(`{_id, version}`), so a stale/duplicate call returns `409 Conflict`.

### Create Operation

#### Start Create
```http
POST /api/v1/inbox/{id}/create/start
```

Transition: `Active` → `Creating`

Request Body:
```json
{ "job_id": "uuid-string" }
```

#### Create Progress
```http
POST /api/v1/inbox/{id}/create/progress
```

Transition: `Creating` → `Creating` (progress field updated)

Request Body:
```json
{
  "job_id": "uuid-string",
  "phase": "preparing",
  "total_files": 100,
  "processed_files": 10,
  "total_bytes": 1048576,
  "transferred_bytes": 104857
}
```

#### Create Complete
```http
POST /api/v1/inbox/{id}/create/complete
```

Transition: `Creating` → `Ready`

Request Body:
```json
{
  "job_id": "uuid-string",
  "inbox_path": "/data/inboxes/507f1f77bcf86cd799439011"
}
```

#### Create Failure
```http
POST /api/v1/inbox/{id}/create/failure
```

Transition: `Creating` → `Failed`

Request Body:
```json
{
  "job_id": "uuid-string",
  "error_code": "citadel_trigger_failed",
  "error_message": "Citadel create trigger failed",
  "total_bytes": 0,
  "transferred_bytes": 0
}
```

---

### Publish Operation

#### Start Publish
```http
POST /api/v1/inbox/{id}/publish/start
```

Transition: `Ready` → `Publishing`

Request Body:
```json
{ "job_id": "uuid-string" }
```

#### Publish Progress
```http
POST /api/v1/inbox/{id}/publish/progress
```

Transition: `Publishing` → `Publishing` (progress field updated)

Request Body:
```json
{
  "job_id": "uuid-string",
  "phase": "transferring",
  "total_files": 100,
  "processed_files": 42,
  "total_bytes": 1048576,
  "transferred_bytes": 524288
}
```

#### Publish Complete
```http
POST /api/v1/inbox/{id}/publish/complete
```

Transition: `Publishing` → `Published`

Request Body:
```json
{
  "job_id": "uuid-string",
  "files_transferred": 420,
  "total_bytes": 1048576,
  "protected_path": "/data/protected/507f1f77bcf86cd799439011"
}
```

> `protected_path` is **accepted but unused** (D11): Barbican neither
> validates nor stores it — it exists only for shape-compat with Citadel's
> `BarbicanClient`. The final warehouse path is Citadel's
> `CITADEL_PROTECTED_BASE_PATH` decision.

#### Publish Failure
```http
POST /api/v1/inbox/{id}/publish/failure
```

Transition: `Publishing` → `Failed`

Request Body:
```json
{
  "job_id": "uuid-string",
  "error_code": "publish_failed",
  "error_message": "Publish job reported failure",
  "files_transferred": 10,
  "total_bytes": 1048576
}
```

---

### Delete Operation

#### Start Delete
```http
POST /api/v1/inbox/{id}/delete/start
```

Transition: `Ready`/`Failed` → `Deleting`

Request Body:
```json
{ "job_id": "uuid-string" }
```

#### Delete Progress
```http
POST /api/v1/inbox/{id}/delete/progress
```

Transition: `Deleting` → `Deleting` (progress field updated)

Request Body:
```json
{
  "job_id": "uuid-string",
  "phase": "deleting",
  "total_bytes": 1048576,
  "deleted_bytes": 524288
}
```

Optional `total_files`/`processed_files` (default 0) are also accepted.
Stored as the inbox's `progress` (`InboxProgress`) with `transferred_bytes`
set to 0.
Citadel's `/jobs/{id}/progress` forwarder maps the worker's generic
bytes-processed counter (`transferred_bytes`) to `deleted_bytes`. The
`citadel-job` worker's delete path does **not** currently report progress,
so in practice this endpoint is not called yet.

#### Delete Complete
```http
POST /api/v1/inbox/{id}/delete/complete
```

Transition: `Deleting` → `Deleted`

Request Body:
```json
{ "job_id": "uuid-string" }
```

#### Delete Failure
```http
POST /api/v1/inbox/{id}/delete/failure
```

Transition: `Deleting` → `Failed`

Request Body:
```json
{
  "job_id": "uuid-string",
  "error_code": "delete_failed",
  "error_message": "Delete job reported failure",
  "total_bytes": 0,
  "deleted_bytes": 0
}
```

---

## Health Endpoints

Both are public (no `Authorization` header).

### Health
```http
GET /health
```

Liveness only — always `200 OK` while the process is serving:
```json
{ "status": "healthy" }
```

### Ready
```http
GET /ready
```

Readiness — pings MongoDB (`InboxStore::ping()`, a `{ping: 1}` command
against the `admin` database).

`200 OK`:
```json
{ "status": "ready", "mongodb": "connected" }
```

`503 Service Unavailable` (ping failed; logged at `warn`):
```json
{ "status": "not_ready", "mongodb": "disconnected" }
```

---

## Error Responses

All errors follow a consistent JSON format:

```json
{
  "error": "conflict",
  "message": "Inbox state was modified concurrently"
}
```

`error` is a lowercase snake_case code; `message` is human-readable. There is
no `details` field. The body is produced by `common::AppError::IntoResponse`
(see [common infrastructure](../../../common/src/docs/infrastructure.md)).

### HTTP Status Codes

| Code | Meaning | `error` code |
|------|---------|--------------|
| 400 | Bad Request | `invalid_json`, `validation_error`, `invalid_publish_parameters` |
| 401 | Unauthorized | `auth_error`, `invalid_callback_secret` |
| 403 | Forbidden | `forbidden` |
| 404 | Not Found | `inbox_not_found` |
| 409 | Conflict | `conflict` (optimistic lock / duplicate warehouse path) |
| 422 | Unprocessable Entity | `invalid_state_transition` |
| 429 | Too Many Requests | `too_many_requests` (queue saturation at Citadel) |
| 500 | Internal Server Error | `internal_error`, `database_error`, etc. |
| 502 | Bad Gateway | `bad_gateway` (Citadel API unavailable) |
| 503 | Service Unavailable | `service_unavailable` (MongoDB down, JWKS unavailable) |

### Example Error Responses

**401 Unauthorized** (missing/invalid token, emitted by `common::auth_middleware`):
```json
{
  "error": "auth_error",
  "message": "Missing or invalid Authorization header"
}
```

**403 Forbidden:**
```json
{
  "error": "forbidden",
  "message": "Missing required role for service 'barbican'"
}
```

**409 Conflict:**
```json
{
  "error": "conflict",
  "message": "Inbox state was modified concurrently"
}
```

**422 Unprocessable Entity:**
```json
{
  "error": "invalid_state_transition",
  "message": "Cannot publish inbox in state 'creating', expected 'ready'"
}
```

**429 Too Many Requests:**
```json
{
  "error": "too_many_requests",
  "message": "Queue rate limit reached; try again later"
}
```

**503 Service Unavailable:**
```json
{
  "error": "service_unavailable",
  "message": "JWT verification temporarily unavailable"
}
```

---

## Rate Limiting

- **Queue saturation (Citadel, enforced)**: publish/delete submissions beyond
  `CITADEL_QUEUE_PUBLISH_RATE_LIMIT` / `CITADEL_QUEUE_DELETE_RATE_LIMIT`
  return `429 Too Many Requests` (`too_many_requests`) from the Citadel
  handlers (calls into the inbox's `publish`/`delete` endpoints).
- **Edge rate limiting (NOT implemented)**: no per-IP/per-client throttling
  (`tower-governor` is not wired) and no `X-RateLimit-*` headers are emitted.
  If an edge proxy is used, enforce limits there.

---

## Pagination

List endpoints support pagination with query parameters:

```http
GET /api/v1/inbox?limit=50&offset=100
```

Response includes pagination metadata:
```json
{
  "inboxes": [...],
  "total": 250,
  "limit": 50,
  "offset": 100,
  "has_more": true
}
```

---

*See also:*
- [Architecture](architecture.md) - System design overview
- [Authentication](authentication.md) - JWT and role details
- [State Machine](state-machine.md) - Inbox lifecycle