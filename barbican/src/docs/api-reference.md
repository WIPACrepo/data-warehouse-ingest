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
- [Error Responses](#error-response)

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
2. **Expiration**: Token must not be expired (5-second leeway)
3. **Issuer**: Must match Keycloak realm URL
4. **Audience**: Must match Barbican client ID
5. **Roles**: Must contain required role for the endpoint

### JWKS Caching

- JWKS fetched from Keycloak and cached
- Refresh interval: 60 seconds (configurable)
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
  "status": "creating",
  "warehouse_path": "/data/warehouse/example",
  "message": "Inbox creation initiated"
}
```

---

### Get Inbox

Retrieves detailed information about a specific inbox.

**Request:**
```http
GET /api/v1/inbox/{id}
Authorization: Bearer <jwt_token>
```

**Required Role:** `read`, `write`, `internal`, or `admin`

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

**Required Role:** `read`, `write`, `internal`, or `admin`

**Query Parameters:**
- `status` - Filter by status (optional)
- `limit` - Maximum results (default: 100)
- `offset` - Pagination offset (default: 0)

**Response (200 OK):**
```json
{
  "inboxes": [...],
  "total": 50,
  "limit": 100,
  "offset": 0
}
```

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

Internal endpoints follow the action pattern: `{operation}/{action}` where action is one of `start`, `progress`, `complete`, `failure`.

**Required Role:** `internal` or `admin`

### Create Operation

#### Start Create
```http
POST /api/v1/inbox/{id}/create/start
```

Transition: `Active` → `Creating`

#### Create Progress
```http
POST /api/v1/inbox/{id}/create/progress
```

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
POST /api/v1/inbox/{id}/create failure
```

Transition: `Creating` → `Failed`

Request---

## Error Responses

All errors follow a consistent JSON format:

```json
{
  "error": "ERROR_CODE",
  "message": "Human-readable description",
  "details": {}
}
```

### HTTP Status Codes

| Code | Meaning | Example |
|------|---------|---------|
| 400 | Bad Request | Invalid JSON, missing required fields |
| 401 | Unauthorized | Missing or invalid JWT |
| 403 | Forbidden | Insufficient role permissions |
| 404 | Not Found | Inbox ID doesn't exist |
| 409 | Conflict | Optimistic lock failure, duplicate inbox |
| 422 | Unprocessable Entity | Invalid state transition |
| 429 | Too Many Requests | Rate limit exceeded |
| 500 | Internal Server Error | Unexpected server error |
| 502 | Bad Gateway | Citadel API unavailable |
| 503 | Service Unavailable | MongoDB down, service starting |

### Example Error Responses

**401 Unauthorized:**
```json
{
  "error": "INVALID_TOKEN",
  "message": "JWT token signature verification failed"
}
```

**403 Forbidden:**
```json
{
  "error": "INSUFFICIENT_ROLE",
  "message": "Required role 'write' not found in token"
}
```

**409 Conflict:**
```json
{
  "error": "OPTIMISTIC_LOCK_FAILURE",
  "message": "Inbox was modified concurrently, please retry"
}
```

**422 Unprocessable Entity:**
```json
{
  "error": "INVALID_STATE_TRANSITION",
  "message": "Cannot publish inbox in state 'creating', expected 'ready'"
}
```

---

## Rate Limiting

Rate limits are applied per endpoint and per client:

| Endpoint Type | Limit | Window |
|--------------|-------|--------|
| External API | 100 requests | 1 minute per IP |
| Internal API | 1000 requests | 1 minute per service account |
| Health/Ready | No limit | - |

Rate limit headers are included in responses:
```http
X-RateLimit-Limit: 100
X-RateLimit-Remaining: 95
X-RateLimit-Reset: 1640000000
```

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