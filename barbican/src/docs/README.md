# Barbican Documentation

Welcome to the Barbican crate documentation. This directory contains comprehensive guides and references for understanding and working with Barbican.

## Table of Contents

### Core Documentation

| Document | Description |
|----------|-------------|
| [Architecture](architecture.md) | System overview, component diagram, and module structure |
| [API Reference](api-reference.md) | Complete endpoint documentation with request/response examples |
| [State Machine](state-machine.md) | Inbox lifecycle, states, and transition rules |
| [Authentication](authentication.md) | JWT validation, roles, and authorization |
| [Citadel Integration](citadel-integration.md) | Backend service coordination and triggers |
| [Configuration](configuration.md) | Environment variables and settings |
| [Development Guide](development.md) | Building, testing, and extending Barbican |

### Design Documents

| Document | Description |
|----------|-------------|
| [Design Plan](01-design-plan.md) | Complete architecture and API specification |
| [Completion Report](02-completion-report.md) | Implementation status and verification |

---

## Quick Start

**What is Barbican?**

Barbican is the user-facing frontend service in the data warehouse ingestion system. It provides a secure HTTP API for managing inbox metadata and orchestrating the lifecycle of data ingestion operations.

**Key Responsibilities:**
- Inbox metadata management (CRUD operations)
- State machine orchestration
- JWT authentication and authorization
- Automatic Citadel backend triggers
- Progress tracking for long-running operations

**Core Design Principles:**
- **Action-Based API**: State transitions via actions (e.g., `/create/complete`)
- **Shared Data Model**: Barbican and Citadel share MongoDB inbox collection
- **Automatic Triggers**: Citadel operations triggered transparently
- **Optimistic Locking**: Version-based concurrency control
- **Unified Terminology**: Consistent `start` → `progress` → `complete`/`failure` pattern

---

## Architecture Overview

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

See [Architecture](architecture.md) for detailed component breakdown.

---

## Module Structure

```
barbican/src/
├── main.rs              # Application entry point
├── lib.rs               # Library exports, AppState, create_app, serve
├── config.rs            # Service env config (auth is common::AuthConfig::from_env_prefixed("BARBICAN"), D10)
├── db.rs                # ensure_inbox_indexes (unique warehouse_path, P1.5)
├── metrics.rs           # BarbicanMetrics: inboxes_total gauge + api_requests_total counter (P4.3)
├── clients/
│   └── citadel.rs       # CitadelClient: triggers on Citadel /create|publish|delete (D9)
├── handlers/
│   ├── create_inbox.rs  # POST /api/v1/inbox (202 envelope)
│   ├── get_inbox.rs     # GET /api/v1/inbox/{id}
│   ├── list_inboxes.rs  # GET /api/v1/inbox (pagination envelope, offset)
│   ├── delete_inbox.rs  # DELETE /api/v1/inbox/{id} (202 envelope)
│   ├── publish_inbox.rs # POST /api/v1/inbox/{id}/publish (202, no body)
│   ├── health.rs        # GET /health
│   ├── ready.rs         # GET /ready (MongoDB ping; 200 ready / 503 not_ready)
│   └── transition/      # Internal action endpoints (called only by Citadel)
│       ├── create.rs    # /create/start, /progress, /complete, /failure
│       ├── publish.rs   # /publish/start, /progress, /complete, /failure
│       └── delete.rs    # /delete/start, /progress, /complete, /failure
├── middleware/
│   └── mod.rs           # Re-exports common::auth_middleware (auth lives in common::auth)
├── test_utils/          # Test utilities (cfg(test))
│   └── mod.rs           # create_test_state, test_auth_state; re-exports common::test_utils + MockInboxStore
├── tests/               # Unit tests (mirroring src/)
└── docs/                # This documentation directory
```

---

## Common Workflows

### Creating an Inbox

1. Client sends `POST /api/v1/inbox` with inbox metadata
2. Barbican creates inbox record with status `Active`
3. Barbican automatically triggers Citadel create operation
4. Citadel creates physical directory and reports progress
5. Citadel calls `/create/complete` when finished
6. Inbox status transitions to `Ready`

See [API Reference](api-reference.md) for detailed endpoint documentation.

### Publishing an Inbox

1. Client sends `POST /api/v1/inbox/{id}/publish`
2. Barbican validates inbox is in `Ready` state
3. Barbican transitions status to `Publishing`
4. Barbican triggers Citadel publish operation
5. Citadel transfers files and reports progress
6. Citadel calls `/publish/complete` when finished
7. Inbox status transitions to `Published`

See [State Machine](state-machine.md) for all valid transitions.

---

## Additional Resources

- [Design Plan](01-design-plan.md) - Historical design (pre-review)
- [Completion Report](02-completion-report.md) - Historical completion report (pre-review)
- [Common Crate Documentation](../../../common/src/docs/README.md) - Shared types and utilities
- [Citadel Documentation](../../../citadel/src/docs/README.md) - Backend service specification

---

*Last Updated: 2026-08-29 (review-driven changes)*