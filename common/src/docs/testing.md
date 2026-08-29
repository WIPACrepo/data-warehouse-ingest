# Testing

This document covers the test utilities in `common` and the testing conventions used across the workspace, so contributors can write consistent, fast tests.

## Table of Contents

- [Overview](#overview)
- [Builders](#builders)
- [Fixtures & Helpers](#fixtures--helpers)
- [The Mock Store](#the-mock-store)
- [Unit Test Conventions](#unit-test-conventions)
- [Integration Tests](#integration-tests)
- [See also](#see-also)

---

## Overview

The workspace convention (from the root `AGENTS.md`) is:

- Each crate gets its own `src/tests/` directory.
- Test utilities live in `src/tests/test_utils/`.
- Tests mirror the source structure.
- Integration tests that require services (like MongoDB) live in `tests/` and are gated behind the `integration-tests` feature flag; they run in CI but may not run in a dev environment.
- All tests run with standard `cargo test`.

In `common` specifically, test utilities live at `common/src/test_utils/` and are exposed through the crate:

```rust
// common/src/test_utils/mod.rs
pub mod builders;
pub mod fixtures;
pub mod helpers;

pub use builders::*;
pub use fixtures::*;
pub use helpers::*;

#[cfg(test)]
pub use crate::traits::MockInboxStore;
```

Because `test_utils` is a normal (non-`#[cfg(test)]`) module, its builders and fixtures are available to **downstream** test code too — e.g. Barbican and Citadel unit tests, and the integration tests in `common/tests/`. Note that `MockInboxStore` is only re-exported when the crate is compiled for tests (`#[cfg(test)]`); in other contexts it is referenced as `common::traits::MockInboxStore` behind the `mockall` dependency.

---

## Builders

`common/src/test_utils/builders.rs` provides builder-pattern fixtures so tests can create realistic objects with sensible defaults and tweak only what they need.

### `InboxBuilder`

```rust
let inbox = InboxBuilder::new()
    .with_name("Science Run")
    .with_size(2 * 1024 * 1024 * 1024) // 2 GiB
    .with_status(InboxStatus::Ready)
    .with_warehouse_path("/warehouse/runs/science")
    .with_archive_enabled(true)
    .with_archive_sites(vec!["site-a".into(), "site-b".into()])
    .with_current_job_id("job-123".into())
    .with_progress(progress)
    .build();
```

Defaults (so most tests only override 1–3 fields):

| Field | Default |
|-------|---------|
| `id` | `Some(ObjectId::new())` |
| `name` | `"Test Inbox"` |
| `size` | `100` |
| `status` | `Active` |
| `warehouse_path` | `"/test/warehouse/path"` |
| `inbox_path` | `None` |
| `lifetime` | `None` |
| `archive_enabled` | `false` |
| `archive_sites` | `[]` |
| `version` | `0` |
| `current_job_id` | `None` |
| `progress` | `None` |

`build()` sets `error: None` and `created_at`/`updated_at` to `Utc::now()`.

### `CreateInboxBuilder`

```rust
let request = CreateInboxBuilder::new()
    .with_name("Urban Image Set")
    .with_size(10 * 1024 * 1024)
    .with_warehouse_path("/warehouse/images/urban")
    .with_archive_enabled(true)
    .build();
```

Defaults mirror `InboxBuilder` (`"Test Inbox"`, `100`, `"/test/warehouse/path"`, no archive).

---

## Fixtures & Helpers

`common/src/test_utils/fixtures.rs` and `common/src/test_utils/helpers.rs` provide plain functions:

| Function | Purpose |
|----------|---------|
| `create_test_inbox(name, size)` | `InboxDB` with `Active` status, warehouse path `"/test/warehouse/{name}"`. |
| `create_test_inbox_with_id(id, name, size)` | Same, with a specific `ObjectId`. |
| `create_test_inbox_with_status(name, size, status)` | `InboxDB` with a given `InboxStatus`. |
| `create_test_inbox_no_id(name, size)` | `InboxDB` with `id: None` (for insertion tests). |
| `create_default_test_inbox()` | Default `InboxDB` ("Default Test Inbox", size 100, `/test/warehouse/default`). |
| `create_test_request(name, size)` | A `CreateInbox` payload. |
| `create_default_test_request()` | Default `CreateInbox` ("Test Request", size 50). |
| `create_test_store()` | A `MockInboxStore` ready for expectations. |
| `assert_inboxes_equal_ignoring_id_and_timestamps(a, b)` | Fieldwise equality assertion that ignores `id`/timestamps. |

These are useful both for `common`'s own tests and for Barbican/Citadel handler tests that need inbox fixtures.

---

## The Mock Store

`MockInboxStore` is generated from `#[automock]` on the `InboxStore` trait (`common/src/traits.rs`) by `mockall`. It implements the full async interface and lets tests record expectations:

```rust
let mut store = MockInboxStore::new();
store
    .expect_find_one()
    .withf(|filter| /* ... */)
    .times(1)
    .returning(|_| Ok(Some(create_default_test_inbox())));

let state = Arc::new(AppState::new(Arc::new(store), config));
```

This is how service handlers (e.g., Barbican's `get_inbox`, `publish_inbox`) are tested without a running MongoDB. Because `InboxStore` is `Send + Sync` and used through `Arc<dyn InboxStore>`, the mock slots into the same `AppState` shape as the production `MongoInboxStore`.

---

## Unit Test Conventions

Unit tests live in `common/src/tests/`, one file per source module, mirroring `src/`:

```
common/src/tests/
├── mod.rs               # declares the test submodules
├── adapters_tests.rs    # MongoInboxStore type/behavior checks
├── auth/                # auth middleware/claims/JWKS tests
├── config_tests.rs      # logging/shutdown utilities
├── db_tests.rs          # db helpers (some conditionally run)
├── error_tests.rs       # AppError/AppJson
├── metrics_tests.rs     # metrics app/router
└── models_tests.rs      # serialization round-trips
```

Examples of things tested here:

- `models_tests.rs::test_inbox_db_serialization_roundtrip` — serializing/deserializing `InboxDB` preserves fields (including status).
- `adapters_tests.rs` — verifies `MongoInboxStore::new` compiles/constructs; MongoDB-dependent tests **skip** when `DATABASE_URL` is not set rather than failing.
- `db_tests.rs` — connects only when a database is reachable.

Convention: tests that need a live service should **skip gracefully** (as the adapter/db tests do) or live in `tests/` behind the feature flag — they must never fail in an environment without the service.

---

## Integration Tests

`common/tests/db_tests.rs` contains integration tests requiring a running MongoDB instance:

```rust
#![cfg(feature = "integration-tests")]
```

- `test_db_setup_with_default_url` — verifies `db_setup()` connects.
- `test_mongo_inbox_store_insert_and_find` — creates an `InboxBuilder` inbox, inserts it via `MongoInboxStore`, finds it back, asserting round-trip against real MongoDB (using a `tests` collection, cleaning up before/after).

These are gated behind `--features integration-tests` (the `integration-tests` feature in `common/Cargo.toml`) and run in CI, but may be skipped in a local dev environment. Run them with:

```bash
cargo test --package common --features integration-tests
```

(This requires a reachable MongoDB, e.g. via `DATABASE_URL`.)

---

## See also

- [README.md](README.md) - Main index
- [models.md](models.md) - Types exercised in tests
- [infrastructure.md](infrastructure.md) - The `InboxStore` trait and `MongoInboxStore` under test
- [barbican/src/docs/development.md](../../barbican/src/docs/development.md) - Testing Barbican (which consumes these utilities)
- [citadel/src/docs/development.md](../../citadel/src/docs/development.md) - Testing Citadel
