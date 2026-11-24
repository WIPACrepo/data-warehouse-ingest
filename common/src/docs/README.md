# Common Crate Documentation

Welcome to the `common` crate documentation. This directory contains guides and references for understanding and working with `common` — the shared library at the heart of the data warehouse ingestion system.

## Table of Contents

### Core Documentation

| Document | Description |
|----------|-------------|
| [Overview](overview.md) | How `common` fits into the Barbican/Citadel architecture, module structure, and ownership boundary |
| [Data Model](models.md) | Shared inbox models: `InboxDB`, `InboxStatus`, `InboxProgress`, `InboxError`, `CreateInbox` |
| [Infrastructure](infrastructure.md) | Shared utilities: config/logging, DB access, error handling, metrics, middleware |
| [Authentication](auth.md) | JWT/Keycloak auth (`auth` feature): prefixed `AuthConfig` env vars, `build_auth_state`, service-to-service tokens, decision D10 |
| [Testing](testing.md) | Test utilities (builders, fixtures, mock store) and testing conventions |

### Module Files

| File | Purpose |
|------|---------|
| [`mod.rs`](mod.rs) | Rust module documentation |
| [`STRUCTURE.md`](STRUCTURE.md) | This documentation's organization |

---

## Quick Start

**What is `common`?**

`common` is a **library crate** (not a service). It holds the types and utilities shared by both services in the data warehouse ingestion system:

- [**Barbican**](../../barbican/src/docs/README.md) — user-facing frontend that manages inbox metadata
- [**Citadel**](../../citadel/src/docs/README.md) — backend that controls protected data areas

Both services depend on `common` via the workspace (`common = { path = "common" }`), so the shared inbox model, storage abstraction, error handling, and infrastructure utilities are defined **once** and reused.

**Key Responsibilities:**

- Define the shared inbox data model (`InboxDB`, `InboxStatus`, ...)
- Define the storage abstraction (`InboxStore` trait + `MongoInboxStore` adapter)
- Provide unified error handling (`AppError`, `AppJson`)
- Provide HTTP infrastructure (routing middleware, metrics, logging, shutdown)
- Provide JWT/Keycloak authentication behind the `auth` feature (`AuthConfig`, `build_auth_state`, `auth_middleware`)
- Provide test utilities (builders, fixtures, mock store)

**Core Design Principles:**

- **Single Source of Truth**: The inbox model and status set live here so Barbican and Citadel never drift apart.
- **Ownership Boundary**: `common` is *data and plumbing only* — it contains no inbox business logic. Barbican owns inbox state transitions; Citadel owns physical file operations. `common` just makes sure both speak the same language.
- **Abstraction over Infrastructure**: Handlers depend on the `InboxStore` trait, not on MongoDB directly — which makes them testable with `MockInboxStore` (generated only under `cfg(test)` or the `test-utils` feature).
- **Consistent Error Contract**: `AppError` maps domain errors to HTTP status codes uniformly across services.

---

## Architecture Overview

```
+-------------+      JWT      +-------------+
|  External   | ------------> |  Barbican   |
|  Clients    |               |  (Frontend) |
+-------------+               +------+------+
                                    ^ |
                                    | | service tokens (both directions)
                                    | v
                              +-------------+
                              |  Citadel    |
                              |  (Backend)  |
                              +------+------+
                                     |
                  +------------------+-------------------+
                  |                                        |
                  v                                        v
        +---------------------+                 +---------------------+
        |   MongoDB           |                 |  POSIX Storage +    |
        |   (inboxes/jobs)    |                 |  K8s/mock executor  |
        +---------------------+                 +---------------------+
                  ^                                        ^
                  |              +-----------+             |
                  +------------> |  common   | <-----------+
                                 |  (shared) |
                                 +-----------+
```

Both services share the `common` crate for the inbox data model, the `InboxStore` storage abstraction, error handling, and infrastructure utilities. See [Overview](overview.md) for the full component breakdown.

---

## Module Structure

```
common/src/
├── lib.rs               # Library exports and public API surface
├── adapters.rs          # MongoInboxStore (MongoDB InboxStore implementation)
├── auth/                # JWT/Keycloak auth (feature `auth`): AuthConfig, build_auth_state, middleware,
│                        #   service-to-service tokens, auth metrics
├── config.rs            # logging_setup, shutdown_signal
├── db.rs                # db_setup, db_database, db_collection (MongoDB helpers)
├── error.rs             # AppError, AppJson, log_app_errors
├── metrics.rs           # metrics_app, router_prometheus, start_metrics_server
├── middleware.rs        # router_logging_layer (trace + timeout)
├── models.rs            # InboxDB, InboxStatus, InboxProgress, InboxError, CreateInbox
├── traits.rs            # InboxStore trait (+ MockInboxStore via mockall, test/test-utils only)
├── test_utils/          # Builders, fixtures, helpers for tests (cfg(test) or feature `test-utils`)
├── tests/               # Unit tests, mirrored per-module
└── docs/                # This documentation directory
```

---

## How Services Use `common`

### Barbican

- `use common::{CreateInbox, InboxDB, InboxStatus, InboxStore, MongoInboxStore};`
- `AuthConfig::from_env_prefixed("BARBICAN")` + `build_auth_state(&cfg, "barbican")` for JWT auth ([auth.md](auth.md))
- `ServiceTokenSource::from_env("BARBICAN", "CITADEL", &cfg)` + `ServiceTokenProvider` for its Bearer token to Citadel
- `db_collection`, `logging_setup`, `shutdown_signal`, `start_metrics_server`, `router_logging_layer`, `AppError`, `AppJson`, `log_app_errors`

### Citadel

- `use common::models::InboxDB;` — reads the shared inbox model whenever it fetches inbox metadata (via `barbican_client.get_inbox`) or persists to the shared `inboxes` collection.
- `AuthConfig::from_env_prefixed("CITADEL")` + `build_auth_state(&cfg, "citadel")` for JWT auth ([auth.md](auth.md))
- `ServiceTokenSource::from_env("CITADEL", "BARBICAN", &cfg)` + `ServiceTokenProvider` for its Bearer token to Barbican

See [Overview](overview.md) for details.

---

## Deep Dives

### The Shared Data Model

`models.md` documents every shared type, including:

- `InboxDB` — the persisted inbox document (fields, serde mapping, `_id` handling)
- `InboxStatus` — the 8-state lifecycle shared by both services
- `InboxProgress`, `InboxError` — progress and failure reporting records
- `CreateInbox` — the request payload for creating an inbox

### The Storage Abstraction

`infrastructure.md` documents:

- `InboxStore` trait — the async storage interface handlers depend on (including `ping` for readiness)
- `MongoInboxStore` — the MongoDB-backed adapter
- `AppError` / `AppJson` — the unified error contract
- `router_logging_layer`, metrics, logging, shutdown, DB helpers

### Testing

`testing.md` documents:

- `InboxBuilder` / `CreateInboxBuilder` — builder pattern for realistic fixtures
- Fixture helpers (`create_test_inbox`, `create_test_store`, ...)
- `MockInboxStore` (from `mockall::automock`, only under `cfg(test)` / feature `test-utils`)
- Per-module unit tests and the `integration-tests` feature

---

## Common Workflow (End to End)

A typical user flow exercises both services and the shared model, even though the user only talks to Barbican:

1. User calls `POST /api/v1/inbox` on **Barbican**
2. Barbican deserializes the request into `CreateInbox` (from `common`)
3. Barbican builds an `InboxDB` and inserts it via the `InboxStore` abstraction
4. Barbican triggers **Citadel** to create the physical directory
5. Citadel reads the `InboxDB` metadata (same shared model) and acts on it
6. Citadel reports progress; Barbican persists `InboxProgress` back into the same document
7. Both sides communicate state transitions via the shared `InboxStatus` vocabulary

---

## Additional Resources

- [Barbican Documentation](../../barbican/src/docs/README.md) - Frontend service docs
- [Citadel Documentation](../../citadel/src/docs/README.md) - Backend service docs
- [Data Model](models.md) - Shared inbox types
- [Infrastructure](infrastructure.md) - Shared utilities
- [Authentication](auth.md) - JWT/Keycloak configuration (D10)
- [Testing](testing.md) - Test utilities and conventions

---

*Last Updated: 2026-11-24*
