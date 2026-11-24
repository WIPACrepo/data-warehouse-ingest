# Testing

This document covers the test utilities in `common` and the testing conventions used across the workspace, so contributors can write consistent, fast tests.

## Table of Contents

- [Overview](#overview)
- [Signed test tokens and mock Keycloak](#signed-test-tokens-and-mock-keycloak)
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
// common/src/lib.rs
#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;

#[cfg(any(test, feature = "test-utils"))]
pub use traits::MockInboxStore;

// common/src/test_utils/mod.rs
#[cfg(feature = "test-utils")]
pub mod auth_mock;     // mock JWKS, runtime RSA test key, signed tokens, mock Keycloak
pub mod builders;
pub mod fixtures;
pub mod helpers;
```

`test_utils` and `MockInboxStore` exist only when `common` is compiled for its own unit tests (`cfg(test)`) or with the **`test-utils`** feature. Downstream crates get them by enabling that feature — Barbican, for example, declares `common` a second time as a dev-dependency with `features = ["test-utils"]`, so plain `cargo test -p barbican` sees `common::test_utils` and `common::MockInboxStore` while its production build (normal dependency, `auth` only) does not. `mockall`, `rsa` (with its `getrandom` feature) and `base64` are optional dependencies enabled only by `test-utils`, which keeps them out of production dependency graphs. (`rand` is no longer a dependency of `common`.)

| Feature | Enables |
|---------|---------|
| *(none)* | Models, `InboxStore`, errors, DB, logging, HTTP metrics; checked in CI with `cargo check -p common --no-default-features` |
| `auth` | JWT validation, service tokens, auth metrics (`jsonwebtoken`, `prometheus`, `once_cell`) |
| `test-utils` | `auth` + `test_utils` module + `MockInboxStore` (`mockall`) + `auth_mock` (`rsa`, `base64`) |
| `integration-tests` | `test-utils` + the MongoDB tests in `common/tests/` |

### Running `common`'s tests

`cargo test -p common` compiles and runs on its own (CI runs it, after `cargo check -p common --no-default-features`, because workspace feature unification would otherwise hide feature-gating mistakes). With `-p common` no feature is enabled, so the `tests/auth/` module (gated on `auth`) is skipped; run it with:

```bash
cargo test -p common --features auth        # auth unit tests
cargo test -p common --features test-utils  # + signed-token tests against the mock Keycloak
```

A workspace-wide `cargo test` enables `auth`/`test-utils` through the services' dependencies, so all of them run there.

---

## Signed test tokens and mock Keycloak

`common/src/test_utils/auth_mock.rs` (feature `test-utils`) signs real RS256 JWTs so tests exercise the production validator end to end:

- **Signing key**: a 2048-bit RSA key **generated at runtime** on first use and shared by the whole test process (`OnceLock`); no private key is stored in the repository. Generation adds a few seconds to each test binary that uses it. `kid` = `TEST_RSA_KID`.
- `test_signing_jwks()` — JWKS containing the test public key.
- `sign_test_jwt(&claims)` — sign arbitrary claims.
- `test_claims(issuer, aud, service, roles, ttl_secs)` — standard claims with `resource_access.<service>.roles`; a negative `ttl_secs` gives an expired token.
- `start_test_keycloak(realm).await -> TestKeycloak` — an axum server on an ephemeral port serving `GET /realms/{realm}/protocol/openid-connect/certs` and a client-credentials `POST .../token`. The token endpoint accepts only `client_secret == TEST_CLIENT_SECRET` (otherwise `401`) and returns a signed token with `sub`/`azp` = `client_id` and `aud` = `"peer"`.
  - Fields: `base_url`, `realm`, `token_requests` (count of token requests), `token_ttl_secs` (adjustable `expires_in`, default 300), `handle`.
  - Methods: `issuer()`, `auth_config(aud)` (an `AuthConfig` pointed at the mock), `token(aud, service, roles, ttl_secs)`.

The older `mock_jwks()` / `generate_test_token()` helpers (a fixed token with a fake signature) remain.

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

`MockInboxStore` is generated by `mockall` from `#[cfg_attr(any(test, feature = "test-utils"), mockall::automock)]` on the `InboxStore` trait (`common/src/traits.rs`), so it is only available to tests or with the `test-utils` feature. It implements the full async interface (including `ping`, which Barbican's `/ready` tests stub via `expect_ping`) and lets tests record expectations:

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
├── auth/                # (feature `auth`) config (via from_lookup), middleware, claims, JWKS,
│                        #   service_token_tests, signed_token_tests (test-utils)
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

`integration-tests` implies `test-utils` (see `common/Cargo.toml`), so the `InboxBuilder` used by these tests is available without listing both features.

- `test_db_setup_with_default_url` — verifies `db_setup()` connects.
- `test_mongo_inbox_store_insert_and_find` — creates an `InboxBuilder` inbox, inserts it via `MongoInboxStore`, finds it back, asserting round-trip against real MongoDB (using a `tests` collection, cleaning up before/after).

These are gated behind `--features integration-tests` (the `integration-tests` feature in `common/Cargo.toml`) and run in CI, but may be skipped in a local dev environment. Run them with:

```bash
cargo test --package common --features integration-tests
# or, for every crate at once (alias in .cargo/config.toml):
cargo test-integration --workspace
```

(This requires a reachable MongoDB, e.g. via `DATABASE_URL`.)

---

## See also

- [README.md](README.md) - Main index
- [models.md](models.md) - Types exercised in tests
- [infrastructure.md](infrastructure.md) - The `InboxStore` trait and `MongoInboxStore` under test
- [auth.md](auth.md) - Auth configuration; tests use `AuthConfig::from_lookup`
- [barbican/src/docs/development.md](../../barbican/src/docs/development.md) - Testing Barbican (which consumes these utilities)
- [citadel/src/docs/development.md](../../citadel/src/docs/development.md) - Testing Citadel
