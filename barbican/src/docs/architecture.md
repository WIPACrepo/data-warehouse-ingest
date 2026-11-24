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
| Barbican → Citadel | Internal | HTTP/REST | Bearer JWT from Keycloak client-credentials (`BARBICAN_CITADEL_CLIENT_ID`/`_SECRET`); `aud` must contain `citadel`, role `write` |
| Citadel → Barbican | Internal | HTTP/REST | Bearer JWT from Keycloak client-credentials (`CITADEL_BARBICAN_CLIENT_ID`/`_SECRET`); no role gate — D9; trust delegated to Citadel |
| Barbican → MongoDB | Internal | MongoDB Wire | Native auth |
| Citadel → MongoDB | Internal | MongoDB Wire | Native auth |

---

## Module Structure

```
barbican/src/
├── main.rs                  # Application entry point, server startup, metrics server
├── lib.rs                   # Library exports, router configuration, AppState
├── config.rs                # Service env config (no auth fields; auth = common::AuthConfig, D10)
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
│   ├── ready.rs             # GET /ready (pings MongoDB via InboxStore::ping)
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
│   └── mod.rs               # create_test_state, test_auth_state; re-exports common::test_utils
│
└── tests/                   # Unit/integration tests
    ├── mod.rs               # Test module exports
    ├── main_tests.rs        # Health endpoint tests
    ├── metrics_tests.rs     # Barbican metrics tests (P4.3)
    ├── config_tests.rs      # Config defaults, overrides, malformed-number errors
    ├── citadel_client_tests.rs # CitadelClient service token: Bearer sent, 401 refresh/retry
    ├── integration_tests.rs # Handler tests
    └── handlers/            # Per-handler tests (incl. ready_tests.rs: /ready ping 200/503)
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

**Decision**: Citadel authenticates to Barbican using a service account JWT,
obtained by Keycloak client-credentials (`CITADEL_BARBICAN_CLIENT_ID`/
`_CLIENT_SECRET`) and refreshed by `common::ServiceTokenProvider`. (Earlier
Citadel sent no token, so every state-transition call was rejected.)

**Rationale**:
- Consistent authentication mechanism
- State-transition endpoints gated by the generic auth layer (no role gate, D9)
- Audit trail of Citadel operations
- No special authentication paths needed

### 7. Shared, Prefixed Auth Configuration (D10)

**Decision**: Keycloak/JWT settings are parsed by
`common::AuthConfig::from_env_prefixed("BARBICAN")`, not by Barbican's
`Config`. `main.rs` loads them before connecting to MongoDB and exits on
error; `serve(client, config, auth_config, citadel_token)` builds the
middleware state with `common::build_auth_state(&auth_config, "barbican")` and
passes it to `AppState::new(store, config, auth_state, citadel_token)`.

**Rationale**:
- Every service validates JWTs identically (one parser, one validator)
- Each service keeps its own namespace (`BARBICAN_*`), fixing the earlier bug
  where the documented `BARBICAN_*` names were ignored and the unprefixed
  `KEYCLOAK_*` names were what was actually read
- Misconfiguration fails at startup with a message naming the variable,
  instead of panicking inside `AppState::new`

See [Configuration](configuration.md#keycloak-authentication).

### 8. Refreshing Service Token for Citadel

**Decision**: Barbican obtains its Citadel Bearer token with a Keycloak
client-credentials grant (`BARBICAN_CITADEL_CLIENT_ID` + a secret from the
`BARBICAN_CITADEL_CLIENT_SECRET` environment variable) through the shared
`common::ServiceTokenProvider`, which caches the token, refreshes it 30 s
before expiry (capped at half the lifetime) and is invalidated after a 401
so the call is retried once. `main.rs` loads the source with
`ServiceTokenSource::from_env("BARBICAN", "CITADEL", &auth_config)`; `serve`
wraps it in an `Arc<ServiceTokenProvider>` passed to `CitadelClient::new`.

**Rationale**:
- The previous design read `BARBICAN_CITADEL_SERVICE_TOKEN_FILE` once at
  startup, so every trigger failed with 401 once that token expired.
- The same mechanism serves Citadel → Barbican, so both directions behave
  identically.
- A secret in an environment variable maps directly to a Kubernetes Secret
  (`env.valueFrom.secretKeyRef`); rotation is a pod restart.
- The static token file remains for development only (re-read on every
  refresh) and is rejected when `PRODUCTION=true`.

See [Configuration](configuration.md#citadel-service-token).

### 9. Strict Service Configuration

**Decision**: `Config::default()` holds literal defaults and reads no
environment; `Config::from_env()` overlays `BARBICAN_*` variables via
`Config::from_lookup`, and a malformed `BARBICAN_CITADEL_API_TIMEOUT_SECS`,
`BARBICAN_CITADEL_API_RETRY_MAX` or `BARBICAN_PROMETHEUS_PORT` is a startup
error naming the variable (`main.rs` logs it and exits 1).

**Rationale**: a typo previously fell back to the default silently; tests
can build a `Config` without being affected by the process environment.

---

## Scalability Considerations

### Horizontal Scaling

- **Stateless Design**: Barbican instances can be scaled horizontally
- **Shared MongoDB**: All instances connect to the same database
- **Citadel Communication**: HTTP-based, works across instances
- **JWT Validation**: JWKS cached locally, refreshed periodically

### Performance Optimizations

- **JWKS Caching**: Reduces Keycloak load; `BARBICAN_JWKS_REFRESH_SECS` (default/minimum 60 s)
- **Async Citadel Calls**: Non-blocking HTTP requests
- **MongoDB Indexes**: Optimized for common query patterns
- **Connection Pooling**: MongoDB and HTTP client connection reuse

### Failure Handling

- **Retry Logic**: Citadel API calls retry transport failures with exponential backoff, and retry once with a fresh service token after a 401
- **Optimistic Locking**: Handles concurrent modifications gracefully
- **Health Endpoints**: `/health` (liveness, always 200) and `/ready` (MongoDB ping: 200 `ready` / 503 `not_ready`)
- **Error Responses**: Structured JSON with appropriate HTTP status codes

---

## Security Architecture

### Authentication Layers

1. **External Clients**: JWT with user roles (read, write, admin)
2. **Citadel Service**: Bearer JWT (client-credentials service account) —
   **no role gate** (D9); trust is delegated to Citadel, the only caller of
   the state-transition endpoints
3. **Barbican → Citadel**: Bearer JWT from Barbican's own service account
   (`aud` contains `citadel`, role `write`), refreshed before expiry
4. **MongoDB**: Native authentication and TLS
5. **Inter-service**: Mutual TLS (recommended for production)

### Authorization Model

| Endpoint Pattern | Required Role(s) |
|-----------------|------------------|
| `POST /api/v1/inbox` | `write` or `admin` |
| `GET /api/v1/inbox` | `read`, `write`, or `admin` |
| `DELETE /api/v1/inbox/{id}` | `write` or `admin` |
| Internal action endpoints | (none — valid Bearer JWT only; called only by Citadel) |
| `/health`, `/ready` | None (public) |

### Security Measures

- **Algorithm Whitelist**: `BARBICAN_ALLOWED_ALGORITHMS` (default `RS256`); `HS*` and `none` rejected at startup
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