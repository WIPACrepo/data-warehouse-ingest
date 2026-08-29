# Design Plan for Barbican Project (Revised for Citadel Integration)

> ⚠️ **Historical document** (2026-08-17). This was the design written before
> the review-driven changes. It is retained for record; the **current**
> behavior is described by [README.md](README.md), [api-reference.md](api-reference.md),
> and [configuration.md](configuration.md). In particular, `internal` roles /
> `BARBICAN_INTERNAL_ROLES` and the `validate_path` helper were **removed**
> (D9) — see the current docs.

This document outlines the complete design for the `barbican` project - a frontend service that manages inbox metadata and provides the API layer for inbox lifecycle operations. Barbican owns the MongoDB inbox collection and coordinates all state changes via action-based API endpoints.

**Last Updated:** 2026-08-17  
**Based on:** Citadel PLAN.md and HANDOFF.md

---

## Overview

Barbican is a long-running HTTP service that:
1. Manages inbox metadata CRUD operations
2. Provides action-based endpoints for state transitions (create, publish, delete)
3. Receives progress updates from Citadel during job execution
4. Validates and persists all inbox state changes
5. Authenticates requests via JWT from Keycloak
6. Triggers Citadel operations automatically for inbox lifecycle events

**Key Architecture Decision:** Barbican and Citadel share the same MongoDB inbox collection. **Barbican owns all inbox metadata writes**; Citadel reads inbox metadata directly but must call Barbican's API for any state changes.

**Key Architecture Decision:** State transitions are **action-based** (e.g., `/publish/complete`, `/create/failure`) not state-based. This ensures business logic validation happens in Barbican, not in calling services.

**Key Architecture Decision:** Job tracking is **Citadel's responsibility** (separate `jobs` collection). Citadel periodically reports progress to Barbican's API, which updates inbox metadata accordingly.

**Key Architecture Decision:** All action endpoints use **unified terminology**: `start`, `progress`, `complete`, `failure`. This provides consistency across create, publish, and delete operations.

**Key Architecture Decision:** Barbican **automatically triggers Citadel** for inbox lifecycle operations (create, delete). External clients only interact with Barbican; Barbican coordinates with Citadel behind the scenes.

---

## Architecture

```
+-------------+      JWT      +-------------+
|  External   | ------------> |  Barbican   |
|  Clients    |               |  (Frontend) |
+-------------+               +------+------+
                                      |
                                      | JWT (internal role)
                                      v
                              +-------------+
                              |  Citadel    |
                              |  (Backend)  |
                              +------+------+
                                     |
        +----------------------------+----------------------------+
        |                            |                            |
        v                            v                            v
+-------------+              +-------------+              +-------------+
|  MongoDB    |              |  POSIX      |              |  K8s Job    |
|  (inboxes)  |              |  Storage    |              |  Executor   |
|  (shared)   |              |  (Citadel)  |              |  (Citadel)  |
+-------------+              +-------------+              +-------------+
```

**Flow:**
1. External client creates inbox metadata via Barbican API
2. Barbican automatically triggers Citadel create operation
3. Citadel creates physical directory, reports progress back to Barbican
4. Citadel calls Barbican's `/create/complete` or `/create/failure` endpoint
5. Similar pattern for publish and delete operations

---

## Section 1: HTTP API Design

### 1.1 External API Endpoints (Client-Facing)

| Method | Path | Description | Auth Required | Triggers Citadel |
|--------|------|-------------|---------------|------------------|
| POST | /api/v1/inbox | Create inbox metadata | Yes (write) | Yes (create) |
| GET | /api/v1/inbox/{id} | Get inbox metadata | Yes (read) | No |
| GET | /api/v1/inbox | List inboxes | Yes (read) | No |
| DELETE | /api/v1/inbox/{id} | Request inbox deletion | Yes (write) | Yes (delete) |
| GET | /health | Health check endpoint | No | No |
| GET | /ready | Readiness probe (MongoDB connectivity) | No | No |

### 1.2 Internal API Endpoints (Citadel-Only)

| Method | Path | Description | Auth Required | Caller |
|--------|------|-------------|---------------|--------|
| POST | /api/v1/inbox/{id}/create/start | Start inbox creation | Yes (internal) | Barbican -> Citadel |
| POST | /api/v1/inbox/{id}/create/progress | Report create progress | Yes (internal) | Citadel -> Barbican |
| POST | /api/v1/inbox/{id}/create/complete | Complete inbox creation | Yes (internal) | Citadel -> Barbican |
| POST | /api/v1/inbox/{id}/create/failure | Fail inbox creation | Yes (internal) | Citadel -> Barbican |
| POST | /api/v1/inbox/{id}/publish/start | Start publish operation | Yes (internal) | Barbican -> Citadel |
| POST | /api/v1/inbox/{id}/publish/progress | Report publish progress | Yes (internal) | Citadel -> Barbican |
| POST | /api/v1/inbox/{id}/publish/complete | Complete publish operation | Yes (internal) | Citadel -> Barbican |
| POST | /api/v1/inbox/{id}/publish/failure | Fail publish operation | Yes (internal) | Citadel -> Barbican |
| POST | /api/v1/inbox/{id}/delete/start | Start delete operation | Yes (internal) | Barbican -> Citadel |
| POST | /api/v1/inbox/{id}/delete/progress | Report delete progress | Yes (internal) | Citadel -> Barbican |
| POST | /api/v1/inbox/{id}/delete/complete | Complete delete operation | Yes (internal) | Citadel -> Barbican |
| POST | /api/v1/inbox/{id}/delete/failure | Fail delete operation | Yes (internal) | Citadel -> Barbican |

**Note:** The unified action pattern is: `{operation}/{action}` where action is one of `start`, `progress`, `complete`, `failure`.

### 1.3 Authentication & Authorization

**JWT Roles:**
| Role | Permissions |
|------|-------------|
| admin | Full access including internal endpoints |
| write | Create inboxes, request deletion, read access |
| read | Query inbox status only |
| internal | Access to action-based transition endpoints (Citadel) |

**Service Accounts:**
- **Citadel service account:** `read` + `internal` roles
- **External clients:** `read` and/or `write` roles based on access level

### 1.4 Key Request/Response Schemas

#### Create Inbox (External Client)

**Request:** POST /api/v1/inbox
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

**Response:** 202 Accepted (triggers async Citadel operation)
```json
{
  "inbox_id": "507f1f77bcf86cd799439011",
  "status": "creating",
  "warehouse_path": "/data/warehouse/example",
  "message": "Inbox creation initiated"
}
```

#### Get Inbox (External Client)

**Request:** GET /api/v1/inbox/{id}

**Response:** 200 OK
```json
{
  "inbox_id": "507f1f77bcf86cd799439011",
  "name": "example-inbox",
  "size": 1048576,
  "status": "creating",
  "warehouse_path": "/data/warehouse/example",
  "inbox_path": null,
  "lifetime": 86400,
  "archive_enabled": true,
  "archive_sites": ["site-a", "site-b"],
  "created_at": "2024-01-01T00:00:00Z",
  "updated_at": "2024-01-01T00:00:00Z",
  "progress": {
    "phase": "preparing",
    "percent_complete": 10
  }
}
```

#### Delete Inbox (External Client)

**Request:** DELETE /api/v1/inbox/{id}

**Response:** 202 Accepted (triggers async Citadel operation)
```json
{
  "inbox_id": "507f1f77bcf86cd799439011",
  "status": "deleting",
  "message": "Inbox deletion initiated"
}
```

#### Create Progress (Citadel -> Barbican)

**Request:** POST /api/v1/inbox/{id}/create/progress
```json
{
  "job_id": "uuid-string",
  "phase": "preparing",
  "total_files": 0,
  "processed_files": 0,
  "total_bytes": 0,
  "transferred_bytes": 0
}
```

**Response:** 200 OK
```json
{
  "inbox_id": "507f1f77bcf86cd799439011",
  "status": "creating",
  "progress": {
    "phase": "preparing",
    "percent_complete": 10
  }
}
```

#### Create Complete (Citadel -> Barbican)

**Request:** POST /api/v1/inbox/{id}/create/complete
```json
{
  "job_id": "uuid-string",
  "inbox_path": "/data/inboxes/507f1f77bcf86cd799439011"
}
```

**Response:** 200 OK
```json
{
  "inbox_id": "507f1f77bcf86cd799439011",
  "status": "ready",
  "inbox_path": "/data/inboxes/507f1f77bcf86cd799439011",
  "completed_at": "2024-01-01T00:00:00Z"
}
```

#### Create Failure (Citadel -> Barbican)

**Request:** POST /api/v1/inbox/{id}/create/failure
```json
{
  "job_id": "uuid-string",
  "error_code": "DIRECTORY_CREATE_FAILED",
  "error_message": "Permission denied creating inbox directory",
  "total_bytes": 0,
  "transferred_bytes": 0
}
```

**Response:** 200 OK
```json
{
  "inbox_id": "507f1f77bcf86cd799439011",
  "status": "failed",
  "error": "DIRECTORY_CREATE_FAILED"
}
```

#### Publish Progress (Citadel -> Barbican)

**Request:** POST /api/v1/inbox/{id}/publish/progress
```json
{
  "job_id": "uuid-string",
  "phase": "copying",
  "total_files": 10,
  "processed_files": 5,
  "total_bytes": 1048576,
  "transferred_bytes": 524288
}
```

**Response:** 200 OK
```json
{
  "inbox_id": "507f1f77bcf86cd799439011",
  "status": "publishing",
  "progress": {
    "phase": "copying",
    "percent_complete": 50
  }
}
```

#### Publish Complete (Citadel -> Barbican)

**Request:** POST /api/v1/inbox/{id}/publish/complete
```json
{
  "job_id": "uuid-string",
  "files_transferred": 10,
  "total_bytes": 1048576,
  "protected_path": "/data/protected/example"
}
```

**Response:** 200 OK
```json
{
  "inbox_id": "507f1f77bcf86cd799439011",
  "status": "published",
  "published_at": "2024-01-01T00:00:00Z"
}
```

#### Publish Failure (Citadel -> Barbican)

**Request:** POST /api/v1/inbox/{id}/publish/failure
```json
{
  "job_id": "uuid-string",
  "error_code": "TRANSFER_FAILED",
  "error_message": "Source file not found during verification",
  "files_transferred": 3,
  "total_bytes": 524288
}
```

**Response:** 200 OK
```json
{
  "inbox_id": "507f1f77bcf86cd799439011",
  "status": "failed",
  "error": "TRANSFER_FAILED"
}
```

#### Delete Progress (Citadel -> Barbican)

**Request:** POST /api/v1/inbox/{id}/delete/progress
```json
{
  "job_id": "uuid-string",
  "phase": "cleaning",
  "total_files": 10,
  "processed_files": 7,
  "total_bytes": 1048576,
  "deleted_bytes": 734003
}
```

**Response:** 200 OK
```json
{
  "inbox_id": "507f1f77bcf86cd799439011",
  "status": "deleting",
  "progress": {
    "phase": "cleaning",
    "percent_complete": 70
  }
}
```

#### Delete Complete (Citadel -> Barbican)

**Request:** POST /api/v1/inbox/{id}/delete/complete
```json
{
  "job_id": "uuid-string"
}
```

**Response:** 200 OK
```json
{
  "inbox_id": "507f1f77bcf86cd799439011",
  "status": "deleted",
  "completed_at": "2024-01-01T00:00:00Z"
}
```

#### Delete Failure (Citadel -> Barbican)

**Request:** POST /api/v1/inbox/{id}/delete/failure
```json
{
  "job_id": "uuid-string",
  "error_code": "CLEANUP_FAILED",
  "error_message": "Source files not found during cleanup",
  "total_bytes": 0,
  "deleted_bytes": 0
}
```

**Response:** 200 OK
```json
{
  "inbox_id": "507f1f77bcf86cd799439011",
  "status": "failed",
  "error": "CLEANUP_FAILED"
}
```

---

## Section 2: Inbox State Machine

### 2.1 Inbox States

```rust
pub enum InboxStatus {
    Active,           // Inbox metadata created, waiting for physical creation
    Creating,         // Physical inbox directory being created (Citadel triggered)
    Ready,            // Inbox physically created, ready for file upload
    Publishing,       // Files being transferred to protected area
    Published,        // Transfer complete, files in protected area
    Failed,           // Operation failed, requires intervention
    Deleting,         // Inbox being cleaned up (Citadel triggered)
    Deleted,          // Inbox cleaned up, metadata archived
}
```

### 2.2 Valid State Transitions (Action-Based)

| Action | Valid From States | Target State | Triggered By |
|--------|------------------|--------------|--------------|
| create (external) | - | Active | External client |
| create/start (auto) | Active | Creating | Barbican (auto-trigger) |
| create/progress | Creating | Creating | Citadel (push) |
| create/complete | Creating | Ready | Citadel (push) |
| create/failure | Creating | Failed | Citadel (push) |
| publish/start | Ready | Publishing | Barbican (auto-trigger) |
| publish/progress | Publishing | Publishing | Citadel (push) |
| publish/complete | Publishing | Published | Citadel (push) |
| publish/failure | Publishing | Failed | Citadel (push) |
| delete (external) | Ready, Failed | Deleting | External client |
| delete/start (auto) | Deleting | Deleting | Barbican (auto-trigger) |
| delete/progress | Deleting | Deleting | Citadel (push) |
| delete/complete | Deleting | Deleted | Citadel (push) |
| delete/failure | Deleting | Failed | Citadel (push) |

### 2.3 Automatic Citadel Triggers

**Barbican automatically triggers Citadel operations:**

1. **On Inbox Creation:**
   - External client POSTs to `/api/v1/inbox`
   - Barbican creates inbox metadata with status `Active`
   - Barbican immediately calls Citadel's create endpoint
   - Citadel begins physical directory creation

2. **On Inbox Deletion:**
   - External client DELETEs `/api/v1/inbox/{id}`
   - Barbican transitions inbox to `Deleting` status
   - Barbican immediately calls Citadel's delete endpoint
   - Citadel begins cleanup operation

3. **On Publish:**
   - External client POSTs to `/api/v1/inbox/{id}/publish`
   - Barbican transitions inbox to `Publishing` status
   - Barbican immediately calls Citadel's publish endpoint
   - Citadel begins file transfer operation

### 2.4 State Transition Validation

Barbican validates all state transitions:
1. **Current state must allow the action** (e.g., can't publish an inbox that's not Ready)
2. **Action must come from authorized source** (internal actions require `internal` role)
3. **Transition must be atomic** (use MongoDB optimistic locking with `version` field)
4. **Invalid transitions return 409 Conflict** with details

---

## Section 3: Configuration & Environment

### 3.1 Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| BARBICAN_BIND_ADDRESS | 127.0.0.1:8080 | HTTP server bind address |
| BARBICAN_MONGODB_URI | mongodb://localhost:27017 | MongoDB connection string |
| BARBICAN_MONGODB_DATABASE | data_warehouse | MongoDB database name |
| BARBICAN_MONGODB_INBOX_COLLECTION | inboxes | Inbox collection name |
| BARBICAN_KEYCLOAK_URL | http://localhost:8080 | Keycloak server URL |
| BARBICAN_KEYCLOAK_REALM | data-warehouse | Keycloak realm name |
| BARBICAN_KEYCLOAK_CLIENT_ID | barbican | Keycloak client ID |
| BARBICAN_KEYCLOAK_CLIENT_SECRET | (required) | Keycloak client secret |
| BARBICAN_REQUIRED_ROLES | read,write | Accepted JWT roles (comma-separated) |
| BARBICAN_INTERNAL_ROLES | internal | Roles for internal endpoints (comma-separated) |
| BARBICAN_ALLOWED_ALGORITHMS | RS256 | Allowed JWT algorithms |
| BARBICAN_JWKS_REFRESH_SECS | 60 | Min seconds between JWKS refreshes |
| BARBICAN_INBOX_BASE_PATH | ./data/inboxes | Base path for inbox directories (for validation) |
| BARBICAN_PROMETHEUS_PORT | 9091 | Port for Prometheus metrics endpoint |
| BARBICAN_CITADEL_API_URL | http://citadel:8081 | Citadel API endpoint for triggers |
| BARBICAN_CITADEL_API_TIMEOUT_SECS | 30 | Timeout for Citadel API calls |
| BARBICAN_CITADEL_API_RETRY_MAX | 3 | Max retries for Citadel API calls |

### 3.2 Retry & Timeout Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| BARBICAN_MONGODB_TIMEOUT_SECS | 30 | MongoDB operation timeout |
| BARBICAN_MONGODB_RETRY_MAX | 3 | Max retries for transient MongoDB errors |
| BARBICAN_JWKS_TIMEOUT_SECS | 10 | JWKS fetch timeout |
| BARBICAN_CITADEL_TRIGGER_RETRY_MS | 1000 | Initial backoff for Citadel trigger retries |

---

## Section 4: Authentication & Authorization

### 4.1 JWT Validation Rules

1. **JWKS Caching**: Cache JWKS, refresh every 60 seconds minimum
2. **Kid Rotation**: If new kid seen, refresh JWKS immediately (rate-limited to 1/min)
3. **Algorithm Security**:
   - Use algorithm from JWK `alg` field
   - Validate against `BARBICAN_ALLOWED_ALGORITHMS` whitelist
   - **Explicitly reject `alg: none`**
   - **Never fall back to HMAC** with public key
4. **Token Validation**:
   - Verify signature using JWKS
   - Verify expiration (5 sec leeway)
   - Verify issuer matches Keycloak realm
   - Verify audience matches client_id
   - Verify roles contain required role for endpoint
5. **Role-Based Access**:
   - `/api/v1/inbox` (POST): requires `write` or `admin`
   - `/api/v1/inbox/{id}` (GET): requires `read`, `write`, `internal`, or `admin`
   - `/api/v1/inbox/{id}` (DELETE): requires `write` or `admin`
   - `/api/v1/inbox/{id}/*/start`, `*/complete`, `*/failure`, `*/progress`: requires `internal` or `admin`
   - `/health`, `/ready`: no auth required

### 4.2 Claims Structure

```rust
pub struct Claims {
    pub sub: String,
    pub iss: String,
    pub aud: String,
    pub exp: usize,
    pub iat: usize,
    pub resource_access: Option<HashMap<String, ResourceAccess>>,
}

impl Claims {
    pub fn get_roles(&self) -> Vec<String> {
        self.resource_access
            .as_ref()
            .and_then(|ra| ra.get("barbican"))
            .map(|ra| ra.roles.clone())
            .unwrap_or_default()
    }
    
    pub fn has_role(&self, role: &str) -> bool {
        self.get_roles().contains(&role.to_string())
    }
    
    pub fn is_admin(&self) -> bool { 
        self.has_role("admin") 
    }
    
    pub fn can_write(&self) -> bool { 
        self.is_admin() || self.has_role("write") 
    }
    
    pub fn is_internal(&self) -> bool { 
        self.is_admin() || self.has_role("internal") 
    }
}
```

### 4.3 Citadel Service Account

Barbican uses a service account to call Citadel's internal endpoints:
- Stored in `BARBICAN_CITADEL_SERVICE_TOKEN_FILE` (path to JWT token file)
- Token is refreshed periodically (configurable)
- Token must have `internal` role for action endpoints

---

## Section 5: MongoDB Integration

### 5.1 Inbox Collection Schema

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
    
    // Progress tracking (updated by Citadel)
    pub current_job_id: Option<String>,
    pub progress: Option<InboxProgress>,
    pub error: Option<InboxError>,
}

pub struct InboxProgress {
    pub phase: String,  // "creating", "copying", "verifying", "deleting"
    pub total_files: u64,
    pub processed_files: u64,
    pub total_bytes: u64,
    pub transferred_bytes: u64,
    pub updated_at: DateTime<Utc>,
}

pub struct InboxError {
    pub code: String,
    pub message: String,
    pub job_id: String,
    pub occurred_at: DateTime<Utc>,
}
```

### 5.2 Indexes

```javascript
{ warehouse_path: 1 }  // unique
{ status: 1 }
{ version: 1 }  // for optimistic locking
{ current_job_id: 1 }
{ "progress.updated_at": 1 }  // for stale progress detection
```

### 5.3 Optimistic Locking

All state transitions use optimistic locking:

```rust
pub async fn transition_state(
    collection: &Collection<InboxDB>,
    inbox_id: ObjectId,
    current_version: u32,
    new_status: InboxStatus,
    updates: Document,
) -> Result<InboxDB, AppError> {
    let result = collection
        .update_one(
            doc! { 
                "_id": inbox_id,
                "version": current_version 
            },
            doc! { 
                "$set": {
                    "status": new_status,
                    "version": current_version + 1,
                    "updated_at": Utc::now(),
                }
            },
        )
        .await?;
    
    if result.matched_count == 0 {
        return Err(AppError::Conflict("Inbox state was modified concurrently".to_string()));
    }
    
    Ok(())
}
```

---

## Section 6: Error Handling

### 6.1 Error Response Format

```json
{
  "error": "error_code",
  "message": "Human readable message",
  "details": {}
}
```

### 6.2 HTTP Status Codes

| Code | Meaning |
|------|---------|
| 400 | Bad Request (validation) |
| 401 | Unauthorized (invalid or missing JWT) |
| 403 | Forbidden (insufficient role) |
| 404 | Not Found (inbox doesn't exist) |
| 409 | Conflict (state mismatch, optimistic lock failure) |
| 422 | Unprocessable Entity (invalid state transition) |
| 429 | Too Many Requests (rate limiting) |
| 500 | Internal Server Error |
| 502 | Bad Gateway (Citadel API unavailable) |
| 503 | Service Unavailable (MongoDB down) |

### 6.3 Error Types

```rust
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Validation error: {0}")]
    ValidationError(String),
    
    #[error("Inbox not found: {0}")]
    InboxNotFound(String),
    
    #[error("Inbox cannot be deleted: {0}")]
    InboxNotDeletable(String),
    
    #[error("Inbox already publishing: {0}")]
    InboxAlreadyPublishing(String),
    
    #[error("Invalid state transition: {0}")]
    InvalidStateTransition(String),
    
    #[error("Conflict: {0}")]
    Conflict(String),
    
    #[error("Database error: {0}")]
    DatabaseError(String),
    
    #[error("Internal error: {0}")]
    InternalError(String),
    
    #[error("Authentication error: {0}")]
    AuthError(String),
    
    #[error("Authorization error: {0}")]
    Forbidden(String),
    
    #[error("Citadel API error: {0}")]
    CitadelApiError(String),
}
```

---

## Section 7: Security Considerations

### 7.1 Path Traversal Prevention

All file paths must be validated:

```rust
pub fn validate_path(base: &Path, requested: &Path) -> Result<PathBuf, AppError> {
    let base_canonical = base.canonicalize()
        .map_err(|_| AppError::ValidationError("Invalid base path".to_string()))?;
    
    let full_path = base.join(requested);
    let canonical = full_path.canonicalize()
        .or_else(|_| Ok(full_path.clone()));
    
    if !canonical.starts_with(&base_canonical) {
        return Err(AppError::ValidationError("Path traversal attempt detected".to_string()));
    }
    
    Ok(canonical)
}
```

### 7.2 Input Validation

- All string inputs sanitized (no HTML/script injection)
- Numeric inputs validated for reasonable ranges
- ObjectId format validated before database queries
- Enum values validated against allowed set

### 7.3 Audit Logging

All state transitions logged with structured JSON:

```json
{
  "timestamp": "2024-01-01T00:00:00Z",
  "event_type": "inbox_publish_started",
  "inbox_id": "507f1f77bcf86cd799439011",
  "caller_identity": "citadel-service",
  "caller_roles": ["internal"],
  "source_ip": "10.0.0.5",
  "job_id": "uuid-string",
  "correlation_id": "trace-id"
}
```

### 7.4 Rate Limiting

- External endpoints: 100 requests/minute per IP
- Internal endpoints: 1000 requests/minute per service account
- Implement using `tower-governor` or similar

---

## Section 8: File Structure

```
barbican/
├── Cargo.toml
├── PLAN.md
├── Dockerfile
└── src/
    ├── main.rs              # Entry point
    ├── lib.rs               # Library exports
    ├── config.rs            # Environment configuration
    ├── error.rs             # Error types (AppError)
    ├── db.rs                # MongoDB connection helpers
    ├── models.rs            # InboxDB, InboxStatus, etc.
    ├── metrics.rs           # Prometheus metrics export
    ├── auth/
    │   ├── jwt_validator.rs # JWKS fetching, token validation
    │   ├── claims.rs        # Claims structure, role checking
    │   └── middleware.rs    # Auth middleware, role extraction
    ├── handlers/
    │   ├── mod.rs
    │   ├── create_inbox.rs  # POST /api/v1/inbox
    │   ├── get_inbox.rs     # GET /api/v1/inbox/{id}
    │   ├── list_inboxes.rs  # GET /api/v1/inbox
    │   ├── delete_inbox.rs  # DELETE /api/v1/inbox/{id}
    │   ├── transition/
    │   │   ├── mod.rs
    │   │   ├── create.rs    # /create/progress, /complete, /failure
    │   │   ├── publish.rs   # /publish/progress, /complete, /failure
    │   │   └── delete.rs    # /delete/progress, /complete, /failure
    │   ├── health.rs        # GET /health
    │   └── ready.rs         # GET /ready
    ├── clients/
    │   ├── mod.rs
    │   └── citadel.rs       # Citadel API client for triggers
    ├── middleware/
    │   ├── auth.rs          # JWT validation middleware
    │   ├── logging.rs       # Request/response logging
    │   └── rate_limit.rs    # Rate limiting middleware
    └── test_utils/
        ├── mod.rs
        ├── fixtures.rs      # Test inbox fixtures
        └── mock_store.rs    # Mock InboxStore for tests
```

---

## Section 9: Implementation Order

1. **Foundation**: config, error types (AppError), db connection, models (InboxDB with version field)
2. **Auth**: JWT validator with JWKS caching, claims structure, auth middleware
3. **Existing Handlers**: Update create_inbox, get_inbox, delete_inbox to use AppError
4. **Citadel Client**: Implement HTTP client for triggering Citadel operations
5. **Transition Handlers**: Implement create/progress, create/complete, create/failure, publish/progress, publish/complete, publish/failure, delete/progress, delete/complete, delete/failure
6. **Automatic Triggers**: Wire up Barbican to call Citadel on external create/delete/publish requests
7. **State Machine**: Add state transition validation logic to all handlers
8. **Optimistic Locking**: Implement version-based concurrency control
9. **Progress Tracking**: Add progress update logic to inbox model and handlers
10. **Health/Ready**: Implement health check and readiness probe endpoints
11. **Audit Logging**: Add structured logging for all state transitions
12. **Rate Limiting**: Add rate limiting middleware
13. **Prometheus Metrics**: Export inbox state metrics, API request metrics
14. **Testing**: Unit tests for handlers, integration tests with MongoDB

---

## Section 10: Dependencies

```toml
[dependencies]
async-trait = { workspace = true }
axum = { workspace = true, features = ["macros"] }
axum-prometheus = { workspace = true }
chrono = { workspace = true }
jsonwebtoken = "9.0"
jwks = "0.4"
mongodb = { workspace = true }
reqwest = { version = "0.12", features = ["json"] }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true, features = ["full"] }
tower = { workspace = true }
tower-http = { workspace = true, features = ["cors", "metrics", "catch-panic", "timeout", "trace"] }
tower-governor = "0.4"
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
uuid = { version = "1.0", features = ["v4", "serde"] }

[dev-dependencies]
mockall = { workspace = true }
axum-test = { workspace = true }
```

---

## Section 11: Success Criteria

- [ ] External endpoints for create, get, list, delete inboxes
- [ ] Internal action-based endpoints (start, progress, complete, failure) for create/publish/delete
- [ ] Automatic Citadel triggering on external operations
- [ ] Citadel push-based progress updates
- [ ] JWT validation with JWKS caching and algorithm whitelist
- [ ] Role-based access control (read/write/internal/admin)
- [ ] Optimistic locking with version field for concurrency
- [ ] State machine validation for all transitions
- [ ] Audit logging for all state transitions
- [ ] Rate limiting on all endpoints
- [ ] Health and readiness endpoints
- [ ] Prometheus metrics for monitoring
- [ ] Comprehensive unit tests with mock store
- [ ] Integration tests with MongoDB
- [ ] Path traversal prevention
- [ ] Input validation and sanitization
- [ ] Error responses sanitized, full details logged

---

## Section 12: Integration with Citadel

### 12.1 Barbican -> Citadel (Automatic Triggers)

1. **Inbox Creation:**
   - External POST to `/api/v1/inbox` creates metadata
   - Barbican calls Citadel's create endpoint automatically
   - Citadel creates physical directory

2. **Inbox Deletion:**
   - External DELETE to `/api/v1/inbox/{id}` marks for deletion
   - Barbican calls Citadel's delete endpoint automatically
   - Citadel performs cleanup

3. **Inbox Publishing:**
   - External POST to `/api/v1/inbox/{id}/publish` marks for publishing
   - Barbican calls Citadel's publish endpoint automatically
   - Citadel performs file transfer

### 12.2 Citadel -> Barbican (Progress Push)

Citadel pushes updates to Barbican throughout job lifecycle:
- **Progress updates**: Every 30 seconds (configurable)
- **Completion**: On successful operation
- **Failure**: On error with error details

### 12.3 Shared Configuration

- Both services use same Keycloak realm for JWT validation
- Both services use same MongoDB database (different collections)
- Citadel's `BARBICAN_API_URL` points to Barbican service endpoint
- Barbican's `CITADEL_API_URL` points to Citadel service endpoint

---

## Notes

- Barbican owns all inbox metadata writes - Citadel never writes directly to inbox collection
- State transitions are action-based, not state-based (business logic in Barbican)
- Optimistic locking prevents concurrent modification issues
- Progress updates from Citadel allow real-time monitoring of long-running operations
- Audit logging provides traceability for all state changes
- Rate limiting protects against DoS attacks
- JWT authentication ensures only authorized services can call internal endpoints
- **Webhook notifications deferred to future feature**
- **No batch operations currently planned**
- **Timeout detection is Citadel's responsibility**
