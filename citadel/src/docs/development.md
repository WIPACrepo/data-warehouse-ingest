# Development Guide

Guide for building, testing, and extending the Citadel service.

## Table of Contents

- [Prerequisites](#prerequisites)
- [Building the Project](#building-the-project)
- [Running Locally](#running-locally)
- [ctadel-job Worker](#cadel-job-worker)
- [Testing](#testing)
- [Code Style](#code-style)
- [Adding New Features](#adding-new-features)
- [Debugging](#debugging)
- [Common Issues](#common-issues)
- [Deployment](#deployment)

---

## Prerequisites

### Required Tools

- **Rust 1.85+ / edition 2024** (see workspace [`Cargo.toml`](../../Cargo.toml))
- **Cargo** (included with Rust)
- **MongoDB** (for a live server / integration tests)
- **Keycloak** (for JWT authentication testing)
- **Git** (version control)

### Optional Tools

- **Docker** (containerized development)
- **kubectl** (Kubernetes development — only needed for the K8s executor)
- **cargo-watch** (automatic recompilation)
- **rust-analyzer** (IDE support)

### Development Environment Setup

```bash
# Clone the repository
git clone https://github.com/your/data-warehouse-ingest.git
cd data-warehouse-ingest

# Rust toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Verify
cargo --version
rustc --version
```

---

## Building the Project

### Workspace Structure

```
data-warehouse-ingest/
├── Cargo.toml          # Workspace configuration
├── citadel/            # Citadel crate (this crate)
│   ├── Cargo.toml
│   └── src/
├── barbican/           # Barbican crate (frontend)
│   ├── Cargo.toml
│   └── src/
└── common/             # Shared library
    ├── Cargo.toml
    └── src/
```

### Build Commands

```bash
# Build the whole workspace
cargo build

# Build only citadel (both binaries)
cargo build --package citadel

# Build a specific binary
cargo build --package citadel --bin citadel
cargo build --package citadel --bin citadel-job

# Check for compilation errors (fast)
cargo check --package citadel

# Generate Rustdoc
cargo doc --package citadel --no-deps
```

### Build Output

The crate produces two binaries:

```
target/
├── debug/
│   ├── citadel          # HTTP service
│   └── citadel-job      # Transfer worker
└── release/
    ├── citadel
    └── citadel-job
```

---

## Running Locally

### Environment Setup

Set `CITADEL_JOB_EXECUTOR=mock` so no Kubernetes cluster is required:

```bash
export CITADEL_BIND_ADDRESS=127.0.0.1:8081
export CITADEL_PROMETHEUS_PORT=9090
export CITADEL_INBOX_BASE_PATH=./data/inboxes
export CITADEL_PROTECTED_BASE_PATH=./data/protected
export CITADEL_MONGODB_URI=mongodb://localhost:27017
export CITADEL_MONGODB_DATABASE=data_warehouse
export CITADEL_KEYCLOAK_URL=http://localhost:8080
export CITADEL_KEYCLOAK_REALM=data-warehouse
export CITADEL_KEYCLOAK_CLIENT_ID=citadel
export CITADEL_KEYCLOAK_CLIENT_SECRET=dev-secret
export CITADEL_JOB_EXECUTOR=mock
export CITADEL_BARBICAN_API_URL=http://localhost:8080
export CITADEL_JOB_CALLBACK_SECRET_FILE=./dev/callback-secret
export RUST_LOG=info,citadel=debug
```

### Run the Service

```bash
# From the workspace root
cargo run --package citadel --bin citadel
```

The service is now listening on `http://127.0.0.1:8081`.

- `GET /health` → liveness
- `GET /ready` → readiness (requires MongoDB)
- `GET /metrics` → on the metrics port (default 9090)

### Verify Endpoints

```bash
# Health (public)
curl http://127.0.0.1:8081/health

# Ready (checks MongoDB)
curl http://127.0.0.1:8081/ready

# Metrics
curl http://127.0.0.1:9090/metrics

# A protected endpoint without a token → 401
curl -i http://127.0.0.1:8081/api/v1/jobs
```

### Auto-Reload During Development

```bash
cargo install cargo-watch
cargo watch -x 'run --package citadel --bin citadel'
```

---

## `citadel-job` Worker

`citadel-job` performs the actual file transfer and reports back. It can be exercised directly for storage testing:

```bash
# Create a test inbox
mkdir -p /tmp/citadel-test/inbox
echo "hello" > /tmp/citadel-test/inbox/a.txt

# Run the worker (JOB_ID, JOB_TYPE, INBOX_ID, INBOX_PATH, WAREHOUSE_PATH required)
JOB_ID=test-1 \
JOB_TYPE=publish \
INBOX_ID=507f1f77bcf86cd799439011 \
INBOX_PATH=/tmp/citadel-test/inbox \
WAREHOUSE_PATH=/tmp/citadel-test/protected/example \
CITADEL_CALLBACK_URL=http://127.0.0.1:8081/api/v1/job/callback \
CITADEL_CALLBACK_SECRET_FILE=./dev/callback-secret \
CITADEL_TRANSFER_PARALLELISM=4 \
CITADEL_TRANSFER_CHUNK_SIZE_MB=16 \
  cargo run --package citadel --bin citadel-job
```

The worker:
1. Loads config from the environment
2. Runs `storage::transfer::transfer_files` (lock → parallel copy+verify → delete sources)
3. Sends a callback with the result
4. Exits `0` on success / `1` on failure

---

## Testing

### Test Structure

Per the workspace [`AGENTS.md`](../../AGENTS.md):
- Each crate gets its own `src/tests/` directory
- Test utilities live in `src/tests/test_utils/`
- Tests mirror the source structure
- Integration tests requiring services (e.g., MongoDB) live in `tests/` and are gated behind the `integration-tests` feature

### Running Tests

```bash
# All tests in the crate
cargo test --package citadel

# Lib tests only
cargo test --package citadel --lib

# Integration tests (requires MongoDB, Keycloak, etc.)
cargo test --package citadel --features integration-tests

# With output
cargo test --package citadel -- --nocapture
```

### Current State

- `mockall` and `axum-test` are available as dev-dependencies for unit tests
- No `src/tests/` or `src/tests/test_utils/` directories exist yet; the `integration-tests` feature is declared but no gated tests are present
- The storage transfer engine is a good target for unit tests (temp dirs, checksum verification, partial-failure cleanup)

---

## Code Style

### Formatting & Linting

```bash
cargo fmt --package citadel
cargo fmt --package citadel -- --check
cargo clippy --package citadel -- -D warnings
```

### Style Guidelines

1. **Use `tracing` for logging**:
   ```rust
   tracing::info!(job_id = %job.job_id, "Submitting job");
   ```
2. **Handle errors with `thiserror`**: `CitadelError` in [`src/error.rs`](../src/error.rs)
3. **Use async/await consistently** (tokio)
4. **Document public items** with `///` doc comments
5. **Return `crate::error::Result<T>`** from fallible operations
6. **Write tests for all public functions**

### Channel / Concurrency

- Use `tokio::sync::{Semaphore, RwLock}` for shared state
- Prefer `Arc<T>` in `AppState` (it derives `Clone`)
- Avoid blocking I/O on the async runtime

---

## Adding New Features

### Example: New Job Type

**1. Add the variant** in [`src/jobs/types.rs`](../src/jobs/types.rs):

```rust
pub enum JobType {
    PublishInbox { inbox_id, notify_catalog, notify_lta },
    DeleteInbox { inbox_id },
    // New: MyNewOperation { inbox_id }
}
```

**2. Wire a queue permit** in [`src/jobs/queue.rs`](../src/jobs/queue.rs) if it needs its own rate limit.

**3. Add a handler** in [`src/handlers/`](../src/handlers/) and register the route in [`src/main.rs`](../src/main.rs).

**4. Handle it in the callback** in [`src/handlers/job_callback.rs`](../src/handlers/job_callback.rs) if it has terminal semantics.

**5. Write tests** (new `src/tests/` module).

**6. Update docs**: [`api-reference.md`](api-reference.md), [`state-machine.md`](state-machine.md), and [`README.md`](README.md) table of contents.

### Example: New Configuration

Add the field to `CitadelConfig` in [`src/config.rs`](../src/config.rs), a default, and an env override in `from_env`. Then add it to the [Configuration](configuration.md) reference.

---

## Debugging

### Logging

```bash
# Set log level
RUST_LOG=debug cargo run --package citadel --bin citadel

# Filter by module
RUST_LOG=citadel::handlers=debug,citadel::auth=trace cargo run --bin citadel
```

### Tracing Requests

Use `tracing::instrument` on handlers to add spans:

```rust
use tracing::instrument;

#[instrument(skip(state))]
pub async fn create_inbox(
    State(state): State<AppState>,
    // ...
) -> Result<...> {
    // ...
}
```

### Common Debugging Targets

- **JWT 401s** → Verify Keycloak is reachable, `CITADEL_KEYCLOAK_*` is right, and JWKS URL resolves
- **MongoDB connection** → Check `mongod` is running; `GET /ready` reports `disconnected`
- **K8s executor not working** → Use `CITADEL_JOB_EXECUTOR=mock` locally; the K8s executor is a stub
- **Port already in use** → Change `CITADEL_BIND_ADDRESS` / `CITADEL_PROMETHEUS_PORT`

---

## Common Issues

### JWT Validation Failures

**Problem**: "Invalid JWT" (401)
**Solutions**:
1. Verify Keycloak is running and reachable
2. Check `CITADEL_KEYCLOAK_URL`, `_REALM`, `_CLIENT_ID`
3. Ensure JWKS URL is: `{url}/auth/realms/{realm}/protocol/openid-connect/certs`
4. Verify the token isn't expired and has `aud` == `CITADEL_KEYCLOAK_CLIENT_ID`

### MongoDB Connection Errors

**Problem**: "/ready" returns `mongodb: disconnected`
**Solutions**:
1. `systemctl status mongod`
2. Verify `CITADEL_MONGODB_URI`
3. Check network: `telnet localhost 27017`

### Kubernetes Executor Stub Errors

**Problem**: `KubernetesExecutor not fully implemented`
**Solutions**:
- For local development use `CITADEL_JOB_EXECUTOR=mock`
- This is a known stub — see [Handoff Notes](../HANDOFF.md)

### Compilation Errors

```bash
cargo update
cargo clean
cargo check --package citadel
```

---

## Deployment

### Docker

The repository [`Dockerfile`](../../Dockerfile) builds both `citadel` and `citadel-job` into one image (see [`PLAN.md`](../PLAN.md) Section 11 for the equivalent Dockerfile):

```bash
docker build -t citadel:latest .
```

The image tag doubles as the version for both binaries. Set `CITADEL_JOB_IMAGE` to the same tag the deployment uses.

### Kubernetes (planned)

- The service is intended to run as a Deployment with `/health` and `/ready` probes
- Jobs are executed as Kubernetes Jobs in `CITADEL_KUBE_NAMESPACE`
- Secrets (Keycloak client secret, callback secret) are mounted from Kubernetes Secrets
- Callback endpoint is intended to be restricted by a NetworkPolicy (per PLAN.md Section 7.7)

### Health Checks

```bash
curl http://127.0.0.1:8081/health     # liveness
curl http://127.0.0.1:8081/ready      # readiness (MongoDB)
curl http://127.0.0.1:9090/metrics    # Prometheus
```

---

## Contributing

### Pull Request Process

1. **Fork** the repository
2. **Create a feature branch**: `git checkout -b feature/my-feature`
3. **Make changes** with tests
4. **Run linter**: `cargo clippy --package citadel`
5. **Format**: `cargo fmt`
6. **Run tests**: `cargo test --package citadel`
7. **Update documentation** as noted above
8. **Submit PR** with a clear description

### Commit Message Format

```
type(scope): subject

body (optional)
```

**Types**: `feat`, `fix`, `docs`, `style`, `refactor`, `test`, `chore`

**Example:**
```
docs(docs): add citadel src/docs documentation

Add the full citadel/src/docs/ set (README, architecture, api-reference,
state-machine, authentication, barbican-integration, configuration,
development) and link it from lib.rs via the docs module.
```

---

*See also:*
- [Architecture](architecture.md) - System design
- [Configuration](configuration.md) - Environment variables
- [Design Plan](../PLAN.md) - Complete specification
- [Handoff Notes](../HANDOFF.md) - Implementation status and next steps
