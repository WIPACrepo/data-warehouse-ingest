# data-warehouse-ingest

Data warehouse ingestion services.

**Rust Version Requirement**: 1.85+ (edition 2024)

![Architecture Diagram](whiteboard_mvp.jpg)

## Quick Navigation

| Component | Documentation | API Reference | Architecture |
|-----------|---------------|---------------|--------------|
| Barbican (Frontend) | [Docs](barbican/src/docs/README.md) | [API](barbican/src/docs/api-reference.md) | [Architecture](barbican/src/docs/architecture.md) |
| Citadel (Backend) | [Docs](citadel/src/docs/README.md) | [API](citadel/src/docs/api-reference.md) | [Architecture](citadel/src/docs/architecture.md) |
| Common (Shared Lib) | [Docs](common/src/docs/README.md) | N/A | [Overview](common/src/docs/overview.md) |

## Project Overview

This is a Rust workspace containing three crates:

- **Barbican**: User-facing frontend service managing inbox metadata.
- **Citadel**: Backend service controlling protected data areas. Ships two
  binaries — `citadel` (long-running HTTP service) and `citadel-job` (one-shot
  job executor, using Kubernetes in production).
- **Common**: Shared library with data models, storage abstractions, and
  infrastructure utilities.

Both services share the `common` crate via workspace dependencies.

## Build & Development

### Workspace Structure

```toml
[workspace]
members = ["citadel", "barbican", "common"]
resolver = "3"
```

### Rust Version

Requires **Rust 1.85+** — all crates use `edition = "2024"`, which mandates
`resolver = "3"` in the workspace root.

### Building

```bash
# Build all crates
cargo build --workspace

# Build specific crate
cargo build -p barbican
cargo build -p citadel
cargo build -p common

# Build Citadel binaries separately
cargo build -p citadel --bin citadel
cargo build -p citadel --bin citadel-job
```

### Testing

```bash
# Run unit + integration tests (uses the `test-integration` alias from .cargo/config.toml)
cargo test-integration
cargo test-integration --workspace   # runs all crates with the integration-tests feature

# Equivalent explicit form
cargo test -p citadel --features integration-tests
cargo test -p common --features integration-tests   # implies test-utils (needed for the InboxBuilder fixtures)
```

> The `common` auth unit tests pass under `cargo test -p common --features auth`.
> Auth-config tests use `AuthConfig::from_lookup` with an in-memory map, so they
> never mutate the process environment.

### Feature Flags

- `common`:
  - `auth` (enables JWT authentication: `jsonwebtoken`, `prometheus`, `once_cell`)
  - `test-utils` (enables `auth` plus the `test_utils` module: builders,
    fixtures, RSA key generation / signed test tokens, and `MockInboxStore`;
    turns on the optional `mockall`, `rsa` and `rand` dependencies)
  - `integration-tests` (implies `test-utils`; gates the MongoDB tests in
    `common/tests/` via `#![cfg(feature = "integration-tests")]`)
- `citadel`:
  - `integration-tests` (bare **marker** feature — gates `#[cfg(feature =
    "integration-tests")]` tests in `tests/`; it does not enable anything else)
- `barbican`:
  - `integration-tests` (bare marker feature, so `cargo test-integration
    --workspace` resolves for every crate).
  - Depends on `common` with `auth` (normal dep) plus an additional `common`
    **dev-dependency** with `test-utils`. Cargo unifies the feature during test
    compilation, so `cargo test -p barbican` compiles Barbican's unit tests
    (which use `common::test_utils` builders + `common::MockInboxStore`)
    **without needing an explicit `--features test-utils`** — per AGENTS.md,
    tests run under plain `cargo test`.
  - The normal dependency (`auth` only) keeps `mockall`, `rsa` and `rand`
    **out of the Barbican production graph**: `MockInboxStore` is generated
    only under `cfg(any(test, feature = "test-utils"))` and `mockall` is an
    optional dependency of `common`. Check with
    `cargo tree -p barbican --edges normal`.

### Dependencies

- **MongoDB** — required for integration tests (CI provides a `mongo:8.0` service container) and at runtime.
- **Kubernetes** — required for the Citadel job executor in production.

## Architecture

Both services are coordinated around the shared `common` crate: Barbican
exposes the user-facing inbox API, and Citadel performs the protected-area
operations behind the scenes.

See detailed architecture docs:

- [Barbican Architecture](barbican/src/docs/architecture.md)
- [Citadel Architecture](citadel/src/docs/architecture.md)
- [Common Overview](common/src/docs/overview.md)

## Barbican

A user frontend for writing files to the Citadel.

MVP: An API allowing limited operations:

| Method | Path | Description |
| ------ | ---- | ----------- |
| `POST` | `/api/v1/inbox` | Create a new inbox |
| `GET`  | `/api/v1/inbox/[id]` | Get info on inbox |
| `DELETE` | `/api/v1/inbox/[id]` | Delete inbox |
| `POST` | `/api/v1/inbox/[id]/publish` | Publish inbox to data warehouse |

### Creating an Inbox

Creating an inbox takes various parameters:

* Final data warehouse path
* Estimated size
* Expected lifetime of inbox
* Should data be archived via LTA?
  * If so, which sites to archive the data at

Additional metadata may be added here in the future.

> Note: if an existing inbox exists for the specified data warehouse path,
> the new request is rejected — the fast in-process check returns `400`, and
> the unique `warehouse_path` index (`uniq_warehouse_path`, created at
> startup) turns a concurrent race or soft-deleted duplicate into a
> `409 Conflict` (P1.5/D3).

### Using an Inbox

Once an inbox has been created, files can be uploaded to it via normal methods.

An inbox is exclusive to a single user, and will not allow group writes.

### Publishing an Inbox

When all data has been uploaded to an inbox and it is ready to be moved into
the data warehouse, the publish action will instruct the Citadel to proceed.
This will lock the inbox while publishing, which is an asynchronous operation.

Publishing takes some time to complete. The inbox status will be updated
as this progresses.

When publishing completes, the inbox is left in the `Published` state; it is
**not** deleted automatically. Removing the inbox is a separate
`DELETE /api/v1/inbox/{id}` request.

See full documentation: [Barbican Docs](barbican/src/docs/README.md)

## Citadel

Controls the protected data areas of the data warehouse. This is a backend
service that should not be called by users.

Some available operations:
1. Create a temporary inbox for new data.
2. Move data from an inbox into a protected area.
3. Delete / cleanup an inbox.
4. Inform a file catalog of the newly controlled files.
5. Inform the Long Term Archive of new data that should be archived.

Citadel provides two binaries:

- `citadel`: Long-running HTTP service.
- `citadel-job`: One-shot job executor for file transfers (uses Kubernetes in production).

See full documentation: [Citadel Docs](citadel/src/docs/README.md)

## Common Library

The `common` crate provides shared types and utilities:

- Inbox data model (`InboxDB`, `InboxStatus`, `InboxProgress`, `InboxError`)
- Storage abstraction (`InboxStore` trait)
- Error handling (`AppError`, `AppJson`)
- Infrastructure utilities (logging, metrics, shutdown signals)
- JWT/Keycloak authentication behind the `auth` feature (`AuthConfig`,
  `build_auth_state`, `auth_middleware`) — see [Authentication](common/src/docs/auth.md)

See full documentation: [Common Docs](common/src/docs/README.md)

## Configuration

Key environment variables by service:

- **Barbican**: See [Configuration](barbican/src/docs/configuration.md)
- **Citadel**: See [Configuration](citadel/src/docs/configuration.md)
- **Common**: See [Infrastructure](common/src/docs/infrastructure.md)
  (logging: `LOG_LEVEL`, `HTTP_LOG_LEVEL`, `THIRD_PARTY_LOG_LEVEL`)

### Authentication Variables

Both services load Keycloak/JWT settings through the shared parser
`common::AuthConfig::from_env_prefixed(prefix)` (decision D10), using their own
prefix `{P}` = `BARBICAN` or `CITADEL`:

| Variable | Default | Notes |
|----------|---------|-------|
| `{P}_KEYCLOAK_URL` | `http://localhost:8080` | Base URL **including** any path prefix (e.g. `https://kc.example.com/auth` for Keycloak < 17); trailing `/` trimmed |
| `{P}_KEYCLOAK_REALM` | `data-warehouse` | |
| `{P}_KEYCLOAK_CLIENT_ID` | `barbican` / `citadel` | Expected `aud`; requires a Keycloak audience mapper |
| `{P}_REQUIRED_ROLES` | `read,write` | Must be non-empty |
| `{P}_ALLOWED_ALGORITHMS` | `RS256` | `HS*` and `none` rejected |
| `{P}_JWKS_REFRESH_SECS` | `60` | Minimum 60 |
| `{P}_TOKEN_LEEWAY_MS` | `5000` | Maximum 60000 |
| `PRODUCTION` | unset | `true` rejects the localhost Keycloak URL |

Issuer is `{URL}/realms/{realm}`; JWKS is
`{URL}/realms/{realm}/protocol/openid-connect/certs`. Malformed numbers fail
startup with an error naming the variable. Full details:
[Authentication](common/src/docs/auth.md).

### Upgrading

Breaking configuration changes in this release:

- **Auth variables are now service-prefixed.** Set `BARBICAN_KEYCLOAK_URL`,
  `BARBICAN_KEYCLOAK_REALM`, `BARBICAN_KEYCLOAK_CLIENT_ID`,
  `BARBICAN_REQUIRED_ROLES`, `BARBICAN_ALLOWED_ALGORITHMS`,
  `BARBICAN_JWKS_REFRESH_SECS`, `BARBICAN_TOKEN_LEEWAY_MS` (and the same
  names with `CITADEL_` for Citadel). The unprefixed `KEYCLOAK_URL`,
  `KEYCLOAK_REALM`, `KEYCLOAK_CLIENT_ID`, `REQUIRED_ROLES`,
  `ALLOWED_ALGORITHMS`, `JWKS_REFRESH_SECS` and `TOKEN_LEEWAY_MS` are no
  longer read (`AuthConfig::from_env()` was removed).
- **`/auth/` is no longer appended to the Keycloak URL.** Legacy Keycloak
  (< 17) deployments must include it: `{P}_KEYCLOAK_URL=https://host/auth`.
- **The default audience changed** from `default-client` to the service name
  (`barbican` / `citadel`); set `{P}_KEYCLOAK_CLIENT_ID` if your client differs.
- **Removed variables:** `BARBICAN_KEYCLOAK_CLIENT_SECRET`,
  `CITADEL_KEYCLOAK_CLIENT_SECRET`, `BARBICAN_MONGODB_TIMEOUT_SECS`,
  `BARBICAN_MONGODB_RETRY_MAX`, `BARBICAN_INBOX_BASE_PATH` (they were unused).
  Barbican no longer fails at startup when `BARBICAN_KEYCLOAK_CLIENT_SECRET`
  is unset.
- **New variable:** `CITADEL_JOB_STATUS_POLL_INTERVAL_SECS` (default `30`,
  must be ≥ 1) sets how often Citadel polls Kubernetes job status.

## Testing

- Unit tests: `cargo test`
- Integration tests: `cargo test-integration` (alias defined in `.cargo/config.toml`; requires MongoDB)
- Integration tests for one crate: `cargo test -p common --features integration-tests`
  (`integration-tests` implies `test-utils` in `common`)
- Test utilities and `MockInboxStore`: available in `common` only under
  `cfg(test)` or with the `test-utils` feature

See [Testing Guide](common/src/docs/testing.md).

## CI/CD

The CI pipeline (`.github/workflows/ci.yaml`) runs:
`cargo machete`, `cargo fmt -- --check`, `cargo clippy -- --deny warnings`,
`cargo verify-project`, `cargo doc --no-deps`, `cargo build`,
`cargo test-integration`, and `cargo tarpaulin`
(with a `mongo:8.0` service container for tests).

## Health & Readiness Endpoints

| Service | Health | Readiness |
|---------|--------|-----------|
| Barbican | `GET /health` | `GET /ready` |
| Citadel | `GET /health` | `GET /ready` |

## Contributing

See [AGENTS.md](AGENTS.md) for development conventions.

## Additional Resources

- [Barbican Design Plan](barbican/src/docs/01-design-plan.md)
- [Barbican Completion Report](barbican/src/docs/02-completion-report.md)
- [Barbican State Machine](barbican/src/docs/state-machine.md)
- [Citadel State Machine](citadel/src/docs/state-machine.md)
- [Citadel Recursive Transfer](citadel/src/docs/recursive-transfer.md)
- [Citadel Security Considerations](citadel/src/docs/security-considerations.md)
- [Citadel Symlink Limitations](citadel/src/docs/symlink-limitations.md)
- [Common Data Model](common/src/docs/models.md)
- [Common Authentication (D10)](common/src/docs/auth.md)
