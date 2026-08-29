# Barbican Architecture

This document describes the system architecture, component relationships, and module structure of the Barbican service.

## Table of Contents

- [System Overview](#system-overview)
- [High-Level Architecture](#high-level-architecture)
- [Component Flow](#component-flow)
- [Module Structure](#module-structure)
- [Data Flow](#data-flow)
- [Key Design Decisions](#key-design-decisions)

---

## System Overview

**Barbican** is the user-facing frontend service in the data warehouse ingestion system. It serves as the API gateway for managing inbox metadata and orchestrating data ingestion operations with the Citadel backend.

### Primary Responsibilities

1. **Inbox Metadata Management**: CRUD operations for inbox records in MongoDB
2. **State Machine Orchestration**: Managing inbox lifecycle through well-defined states
3. **Authentication & Authorization**: JWT validation and role-based access control
4. **Citadel Coordination**: Automatically triggering backend operations
5. **Progress Tracking**: Receiving and persisting updates from Citadel

---

## High-Level Architecture

```
+-------------+      JWT      +-------------+
|  External   | ------------> |  Barbican   |
|  Clients    |               |  (Frontend) |
+-------------+               +------+------+
                                      |
                                      | Bearer JWT (no role gate, D9)
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

### Architecture Principles

- **Separation of Concerns**: Barbican handles metadata and API; Citadel handles physical operations
- **Shared Data Model**: Both services use the same MongoDB inbox collection
- **Action-Based API**: State transitions triggered by actions, not direct state updates
- **Automatic Orchestration**: External clients only interact with Barbican; Citadel is triggered transparently
- **Optimistic Concurrency**: Version-based locking prevents race conditions

---

## Component Flow

### Typical Inbox Creation Flow

1. **External Client** → Creates inbox metadata via Barbican API (with JWT auth)
2. **Barbican** → Validates request, creates inbox record with status `Active`
3. **Barbican** → Automatically triggers Citadel create operation (Bearer service token)
4. **Citadel** → Creates physical directory structure
5. **Citadel** → Reports progress back to Barbican via `/create/progress`
6. **Citadel** → Calls Barbican's `/create/complete` or `/create/failure` endpoint
7. **Barbican** → Updates inbox state in MongoDB
8. **External Client** → Can query inbox status at any time via `/api/v1/inbox/{id}`

### Key Interactions

| Interaction | Direction | Protocol | Auth |
|-------------|-----------|----------|------|
| Client → Barbican | External | HTTP/REST | JWT (user roles) |
| Barbican → Citadel | Internal | HTTP/REST | JWT (service account) |
| Citadel → Barbican | Internal | HTTP/REST | Bearer JWT (no role gate — D9; trust delegated to Citadel) |
| Barbican → MongoDB | Internal | MongoDB Wire | Native auth |
| Citadel → MongoDB | Internal | MongoDB Wire | Native auth |

---

## Module Structure

```
barbican/src/
├── main.rs                  # Application entry point, server startup, metrics server
├── lib.rs                   # Library exports, router configuration, AppState
├── config.rs                # Environment configuration loading and validation
├── db.rs                    # ensure_inbox_indexes (unique warehouse_path, P1.5)
├── metrics.rs               # BarbicanMetrics (inboxes_total gauge, api_requests counter, P4.3)
│
├── clients/                 # External service clients
│   ├── mod.rs               # Module exports
│   └── citadel.rs           # CitadelClient: triggers on Citadel /create|publish|delete
│
├── handlers/                # HTTP request handlers
│   ├── mod.rs               # Module exports
│   ├── create_inbox.rs      # POST /api/v1/inbox (202 envelope)
│   ├── get_inbox.rs         # GET /api/v1/inbox/{id}
│   ├── list_inboxes.rs      # GET /api/v1/inbox (pagination envelope, offset)
│   ├── delete_inbox.rs      # DELETE /api/v1/inbox/{id} (202 envelope)
│   ├── publish_inbox.rs     # POST /api/v1/inbox/{id}/publish (202, no body)
│   ├── health.rs            # GET /health
│   ├── ready.rs             # GET /ready
│   └── transition/          # Internal action endpoints (called only by Citadel)
│       ├── mod.rs           # Transition module exports + apply_versioned_update (P1.1)
│       ├── create.rs        # /create/start, /progress, /complete, /failure
│       ├── publish.rs       # /publish/start, /progress, /complete, /failure
│       └── delete.rs        # /delete/start, /progress, /complete, /failure
│
├── middleware/              # Re-exports (auth lives in common::auth)
│   └── mod.rs               # Re-exports common::auth_middleware
│
├── test_utils/              # Test utilities (cfg(test))
│   └── fixtures.rs          # Test inbox fixtures (create_test_store/fixtures/state)
│
└── tests/                   # Unit/integration tests
    ├── mod.rs               # Test module exports
    ├── main_tests.rs        # Health endpoint tests
    ├── metrics_tests.rs     # Barbican metrics tests (P4.3)
    ├── integration_tests.rs # Handler tests
    └── handlers/            # Per-handler tests
```

### Shared Library (common crate)

Barbican depends on the `common` workspace library which provides:

- **Data Models**: `InboxDB`, `InboxStatus`, `InboxProgress`, `InboxError`
- **Traits**: `InboxStore` for database abstraction
- **Implementations**: `MongoInboxStore` for MongoDB operations
- **Error Types**: `AppError` unified error handling
- **Utilities**: Logging, shutdown signals, JSON response wrappers

---

## Data Flow

### Request Processing Pipeline

```
HTTP Request
    ↓
┌─────────────────────────────────────┐
│  Axum Router                        │
│  - Route matching                   │
│  - Method validation                │
└─────────────────────────────────────┘
    ↓
┌─────────────────────────────────────┐
│  Auth Middleware                    │
│  - JWT extraction                   │
│  - Signature validation             │
│  - Role verification                │
└─────────────────────────────────────┘
    ↓
┌─────────────────────────────────────┐
│  Handler Function                   │
│  - Request parsing                  │
│  - Business logic                   │
│  - State validation                 │
└─────────────────────────────────────┘
    ↓
┌─────────────────────────────────────┐
│  InboxStore (MongoDB)               │
│  - Optimistic locking               │
│  - Atomic updates                   │
└─────────────────────────────────────┘
    ↓
HTTP Response
```

### State Transition Flow

```
External Request
    ↓
┌─────────────────────────────────────┐
│  Validate Current State             │
│  - Check state allows transition    │
│  - Verify JWT roles                 │
└─────────────────────────────────────┘
    ↓
┌─────────────────────────────────────┐
│  Update Inbox Document              │
│  - Increment version                │
│  - Set new state                  │
│  - Update timestamps                │
└─────────────────────────────────────┘
    ↓
┌─────────────────────────────────────┐
│  Trigger Citadel (if applicable)    │
│  - Async HTTP call                  │
│  - Service account JWT              │
└─────────────────────────────────────┘
    ↓
┌─────────────────────────────────────┐
│  Return Response                    │
│  - 200/202 on success               │
│  - 409 on conflict                  │
│  - 422 on invalid transition        │
└─────────────────────────────────────┘
```

---

## Key Design Decisions

### 1. Action-Based API

**Decision**: State transitions are triggered by actions (e.g., `/create/complete`) rather than direct state updates (e.g., `PUT /inbox/{id}/status`).

**Rationale**:
- Ensures business logic validation happens in Barbican
- Prevents invalid state transitions
- Provides clear audit trail of operations
- Aligns with event-driven architecture

### 2. Shared MongoDB Collection

**Decision**: Barbican and Citadel share the same inbox collection in MongoDB.

**Rationale**:
- Single source of truth for inbox state
- Citadel can read inbox metadata directly
- Barbican owns all writes (enforced by API design)
- Eliminates need for data synchronization

### 3. Automatic Citadel Triggers

**Decision**: Barbican automatically triggers Citadel operations for inbox lifecycle events.

**Rationale**:
- External clients only need to interact with one service
- Ensures consistent orchestration
- Reduces client complexity
- Centralizes business logic in Barbican

### 4. Optimistic Locking

**Decision**: All state transitions use version-based optimistic locking.

**Rationale**:
- Prevents concurrent modification conflicts
- Simpler than pessimistic locking (no distributed locks)
- Detects race conditions between Barbican and Citadel
- Returns 409 Conflict on version mismatch

### 5. Unified Action Terminology

**Decision**: All operations follow the pattern: `start` → `progress` → `complete`/`failure`.

**Rationale**:
- Consistent API design across create/publish/delete
- Easier to understand and document
- Simplifies handler implementation
- Clear semantic meaning for each action

### 6. JWT Service Account for Citadel

**Decision**: Citadel authenticates to Barbican using a service account JWT.

**Rationale**:
- Consistent authentication mechanism
- State-transition endpoints gated by the generic auth layer (no role gate, D9)
- Audit trail of Citadel operations
- No special authentication paths needed

---

## Scalability Considerations

### Horizontal Scaling

- **Stateless Design**: Barbican instances can be scaled horizontally
- **Shared MongoDB**: All instances connect to the same database
- **Citadel Communication**: HTTP-based, works across instances
- **JWT Validation**: JWKS cached locally, refreshed periodically

### Performance Optimizations

- **JWKS Caching**: Reduces Keycloak load, 60-second refresh interval
- **Async Citadel Calls**: Non-blocking HTTP requests
- **MongoDB Indexes**: Optimized for common query patterns
- **Connection Pooling**: MongoDB and HTTP client connection reuse

### Failure Handling

- **Retry Logic**: Citadel API calls use exponential backoff
- **Optimistic Locking**: Handles concurrent modifications gracefully
- **Health Endpoints**: `/health` and `/ready` for load balancer checks
- **Error Responses**: Structured JSON with appropriate HTTP status codes

---

## Security Architecture

### Authentication Layers

1. **External Clients**: JWT with user roles (read, write, admin)
2. **Citadel Service**: Bearer JWT — **no role gate** (D9); trust is
   delegated to Citadel, the only caller of the state-transition endpoints
3. **MongoDB**: Native authentication and TLS
4. **Inter-service**: Mutual TLS (recommended for production)

### Authorization Model

| Endpoint Pattern | Required Role(s) |
|-----------------|------------------|
| `POST /api/v1/inbox` | `write` or `admin` |
| `GET /api/v1/inbox` | `read`, `write`, or `admin` |
| `DELETE /api/v1/inbox/{id}` | `write` or `admin` |
| Internal action endpoints | (none — valid Bearer JWT only; called only by Citadel) |
| `/health`, `/ready` | None (public) |

### Security Measures

- **Algorithm Whitelist**: Only RS256 allowed, `alg: none` explicitly rejected
- **Token Validation**: Expiration, issuer, audience, signature
- **Input Sanitization**: All string inputs validated
- **Path Traversal Prevention**: warehouse-path containment is enforced by
  **Citadel** (`CITADEL_PROTECTED_BASE_PATH` + `validate_warehouse_path`),
  not by Barbican (D9/D11)
- **Rate Limiting**: not implemented in Barbican (edge proxy recommended);
  queue saturation is 429 at Citadel (D8)

---

*See also:*
- [API Reference](api-reference.md) - Endpoint documentation
- [Authentication](authentication.md) - JWT and role details
- [Configuration](configuration.md) - Environment variables