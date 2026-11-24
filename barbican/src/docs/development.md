# Development Guide

Guide for building, testing, and extending the Barbican service.

## Table of Contents

- [Prerequisites](#prerequisites)
- [Building the Project](#building-the-project)
- [Running Locally](#running-locally)
- [Testing](#testing)
- [Code Style](#code-style)
- [Adding New Features](#adding-new-features)
- [Debugging](#debugging)
- [Common Issues](#common-issues)
- [Deployment](#deployment)

---

## Prerequisites

### Required Tools

- **Rust 1.70+ (2024 edition)
- **Cargo** (included with Rust)
- **MongoDB** (for integration tests)
- **Keycloak** (for authentication testing)
- **Git** (version control)

### Optional Tools

- **Docker** (containerized development)
- **kubectl** (Kubernetes development)
- **cargo-watch** (automatic recompilation)
- **rust-analyzer** (IDE support)

### Development Environment Setup

```bash
# Clone the repository
git clone https://github.com/your/data-warehouse-ingest.git
cd data-warehouse-ingest

## Install# Install Rust toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install cargo-watch for development
cargo install cargo-watch

# Verify installation
cargo --version
rustc --version
```

---

## Building the Project

### Workspace Structure

Barbican is part of a Rust workspace:

```
data-warehouse-ingest/
├── Cargo.toml          # Workspace configuration
├── barbican/           # Barbican crate
│   ├── Cargo.toml
│   └── src/
├── citadel/            # Citadel crate
│   ├── Cargo.toml
│   └── src/
└── common/             # Shared library
    ├── Cargo.toml
    └── src/
```

### Build Commands

```bash
# Build entire workspace
cargo build

# Build only barbican
cargo build --package barbican

# Build in release mode
cargo build --release --package barbican

# Check for compilation errors (faster than build)
cargo check --package barbican

# Generate documentation
cargo doc --package barbican --open
```

### Build Output

```
target/
├── debug/          # Debug builds
│   └── barbican    # Debug binary
└── release/        # Release builds
    └── barbican    # Optimized binary
```

---

## Running Locally

### Environment Setup

Create a `.env` file in the project root:

```bash
# .env file
BARBICAN_BIND_ADDRESS=127.0.0.1:8080
BARBICAN_MONGODB_URI=mongodb://localhost:27017
BARBICAN_MONGODB_DATABASE=data_warehouse_dev
BARBICAN_KEYCLOAK_URL=http://localhost:8080
BARBICAN_KEYCLOAK_REALM=data-warehouse
BARBICAN_KEYCLOAK_CLIENT_ID=barbican-dev
BARBICAN_REQUIRED_ROLES=read,write
BARBICAN_CITADEL_API_URL=http://localhost:8081
BARBICAN_PROMETHEUS_PORT=9091
RUST_LOG=debug
```

Auth variables (`BARBICAN_KEYCLOAK_*`, `BARBICAN_REQUIRED_ROLES`, ...) are
read by `common::AuthConfig::from_env_prefixed("BARBICAN")`; an invalid value
stops startup before MongoDB is contacted. There is no client secret
variable. See [Configuration](configuration.md).

### Run with Cargo

```bash
# Load environment and run
source .env && cargo run --package barbican

# Or use direnv for automatic loading
direnv allow
cargo run --package barbican
```

### Run with Docker

```bash
# Build Docker image
docker build -t barbican:dev ./barbican

# Run container
docker run -p 8080:8080 -p 9091:9091 \
  --env-file .env \
  barbican:dev
```

### Auto-Reload During Development

```bash
# Install cargo-watch
cargo install cargo-watch

# Run with auto-reload on file changes
cargo watch -x 'run --package barbican'
```

---

## Testing

### Test Structure

```
barbican/src/
├── tests/              # Integration tests
│   ├── mod.rs
│   ├── main_tests.rs
│   └── handlers/
│       ├── create_inbox_tests.rs
│       ├── get_inbox_tests.rs
│       ├── ready_tests.rs
│       └── ...
└── test_utils/         # Test utilities (cfg(test))
    └── mod.rs          # create_test_state, test_auth_state
```

### Running Tests

```bash
# Run all tests
cargo test --package barbican

# Run only unit tests (fast, no MongoDB required)
cargo test --package barbican --lib

# Run integration tests (requires MongoDB)
cargo test --package barbican --features integration-tests

# Run specific test
cargo test --package barbican test_create_inbox

# Run tests with output
cargo test --package barbican -- --nocapture

# Run tests with coverage (requires cargo-tarpaulin)
cargo tarpaulin --package barbican --out html
```

### Test Utilities

`crate::test_utils` is `#[cfg(test)]` (in-crate tests only). It re-exports
`common::test_utils::*` and `common::MockInboxStore`, and adds:

- `create_test_state(store: Arc<MockInboxStore>) -> Arc<AppState>` — default
  `Config`, no Citadel client, `test_auth_state()`, fresh metrics.
- `test_auth_state() -> Arc<common::AuthMiddlewareState>` — built with
  `common::build_auth_state` from a literal `AuthConfig` (unreachable
  localhost Keycloak, roles `read`/`write`); no environment variables read.

**Mock Store:**
```rust
use crate::test_utils::create_test_state;
use common::MockInboxStore;
use common::test_utils::create_test_inbox_with_id;

#[tokio::test]
async fn test_handler_with_mock() {
    let inbox_id = ObjectId::new();
    let inbox = create_test_inbox_with_id(inbox_id, "test-inbox", 100);

    let mut mock_store = MockInboxStore::new();
    mock_store
        .expect_find_one()
        .returning(move |_| Ok(Some(inbox.clone())));

    let state = create_test_state(Arc::new(mock_store));
    let app = Router::new()
        .route("/api/v1/inbox/{id}", get(get_inbox))
        .with_state(state);

    // Drive `app` with tower::ServiceExt::oneshot
}
```

**Production wiring** (outside tests), for reference:
```rust
let auth_state = common::build_auth_state(&auth_config, "barbican");
let state = Arc::new(AppState::new(store, config, auth_state));
```

**Test Fixtures** (`common::test_utils`):
```rust
use common::test_utils::create_test_inbox_with_status;

#[test]
fn test_with_fixture() {
    let inbox = create_test_inbox_with_status("test-inbox", 100, InboxStatus::Ready);
    assert_eq!(inbox.status, InboxStatus::Ready);
}
```

### Writing Tests

**Unit Test Example:**
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::create_test_state;
    
    #[test]
    fn test_validate_inbox_name() {
        assert!(validate_name("valid-name").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name("name with spaces").is_err());
    }
    
    #[tokio::test]
    async fn test_create_inbox_handler() {
        let mock_store = MockInboxStore::new();
        let state = create_test_state(Arc::new(mock_store));
        
        let request = CreateInboxRequest {
            name: "test-inbox".to_string(),
            size: 1024,
            warehouse_path: "/data/test".to_string(),
            ..Default::default()
        };
        
        let response = create_inbox(State(state), Json(request)).await;
        
        assert!(response.is_ok());
    }
}
```

---

## Code Style

### Formatting

```bash
# Format all code
cargo fmt

# Check formatting without changes
cargo fmt -- --check
```

### Linting

```bash
# Run clippy linter
cargo clippy --package barbican -- -D warnings

# Auto-fix linter warnings
cargo clippy --package barbican --fix
```

### Style Guidelines

1. **Use `tracing` for logging**:
   ```rust
   tracing::info!(inbox_id = %inbox._id, "Inbox created successfully");
   ```

2. **Handle errors with `thiserror`**:
   ```rust
   #[derive(Debug, thiserror::Error)]
   pub enum AppError {
       #[error("Inbox not found: {0}")]
       InboxNotFound(String),
   }
   ```

3. **Use async/await consistently**:
   ```rust
   pub async fn find_inbox(&self, id: ObjectId) -> Result<Option<InboxDB>> {
       // ...
   }
   ```

4. **Document public APIs**:
   ```rust
   /// Creates a new inbox and triggers Citadel for physical creation.
   ///
   /// # Arguments
   /// * `state` - Application state with store and Citadel client
   /// * `input` - Inbox creation request
   ///
   /// # Returns
   /// * `202 Accepted` on success
   /// * `400 Bad Request` on validation error
   pub async fn create_inbox(...) { ... }
   ```

5. **Write tests for all public functions**:
   ```rust
   #[cfg(test)]
   mod tests {
       #[test]
       fn test_my_function() { ...       // ...
   }
   ```

---

## Adding New Features

### New Endpoint Example

**Step 1: Create Handler** (`src/handlers/my_feature.rs`):
```rust
use axum::{
    extract::{Json, Path, State},
    http::StatusCode,
};
use std::sync::Arc;
use crate::AppState;
use common::{AppError, AppJson, InboxDB};
use mongodb::bson::doc;

#[derive(Debug, serde::Deserialize)]
pub struct MyFeatureRequest {
    pub data: String,
}

pub async fn my_feature_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<ObjectId>,
    Json(input): Json<MyFeatureRequest>,
) -> Result<AppJson<InboxDB>, AppError> {
    // Validate input
    if input.data.is_empty() {
        return Err(AppError::ValidationError("Data cannot be empty".to_string()));
    }
    
    // Find inbox
    let mut inbox = state.store
        .find_one(doc! { "_id": id })
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?
        .ok_or_else(|| AppError::InboxNotFound(id.to_string()))?;
    
    // Perform operation
    inbox.updated_at = chrono::Utc::now();
    
    // Update database
    state.store
        .update_one(doc! { "_id": id }, doc! { "$set": { "updated_at": inbox.updated_at } })
        .await?;
    
    Ok(AppJson::new(inbox))
}
```

**Step 2: Export Handler** (`src/handlers/mod.rs`):
```rust
pub mod my_feature;
pub use my_feature::my_feature_handler;
```

**Step 3: Add Route** (`src/lib.rs`):
```rust
pub fn create_app(state: Arc<AppState>) -> Router {
    let api_routes = Router::new()
        // ... existing routes ...
        .route("/api/v1/inbox/{id}/my-feature", post(handlers::my_feature_handler))
        .with_state(state.clone());
    
    // ... rest of setup ...
}
```

**Step 4: Add Authorization** (`src/middleware/auth.rs`):
```rust
fn get_required_role(request: &RequestParts) -> Option<&'static str> {
    match (request.method(), request.uri().path()) {
        // ... existing rules ...
        (&Method::POST, "/api/v1/inbox/{id}/my-feature") => Some("write"),
        _ => None,
    }
}
```

**Step 5: Write Tests** (`src/tests/handlers/my_feature.rs`):
```rust
#[tokio::test]
async fn test_my_feature_handler() {
    let mock_store = MockInboxStore::new();
    // ... setup mocks ...
    
    let response = my_feature_handler(State(state), Path(id), Json(request)).await;
    
    assert!(response.is_ok());
}
```

**Step 6: Update Documentation**:
- Add to `docs/api-reference.md`
- Update `docs/architecture.md` if needed
- Add to `README.md` table of contents

---

## Debugging

### Logging

```bash
# Set log level
RUST_LOG=debug cargo run --package barbican

# Filter by module
RUST_LOG=barbican::handlers=debug,barbican::auth=trace cargo run

# Structured JSON logging
RUST_LOG=info RUST_LOG_FORMAT=json cargo run
```

### Debugging with IDE

**VS Code with rust-analyzer:**
1. Install rust-analyzer extension
2. Create `.vscode/launch.json`:
   ```json
   {
       "version": "0.2.0",
       "configurations": [
           {
               "type": "lldb",
               "request": "launch",
               "name": "Debug Barbican",
               "cargo": {
                   "args": ["build", "--package=barbican", "--bin=barbican"],
                   "filter": {
                       "kind": "bin"
                   }
               },
               "env": {
                   "RUST_LOG": "debug"
               }
           }
       ]
   }
   ```

### Tracing Requests

Add tracing spans to handlers:

```rust
use tracing::{info, instrument, warn};

#[instrument(skip(state, input), fields(inbox_id))]
pub async fn create_inbox(
    State(state): State<Arc<AppState>>,
    Json(input): Json<CreateInboxRequest>,
) -> Result<AppJson<CreateInboxResponse>, AppError> {
    info!("Creating inbox");
    
    // ... handler logic ...
    
    if let Some(ref inbox_id) = response.inbox_id {
        tracing::Span::current().record("inbox_id", inbox_id);
    }
    
    Ok(AppJson::new(response))
}
```

---

## Common Issues

### JWT Validation Failures

**Problem:** "Invalid token signature"

**Solutions:**
1. Verify Keycloak is running and accessible
2. Check `BARBICAN_KEYCLOAK_URL` includes any path prefix your Keycloak needs
   (e.g. `.../auth` for Keycloak < 17) — JWKS URL is
   `{BARBICAN_KEYCLOAK_URL}/realms/{realm}/protocol/openid-connect/certs`
3. Check the token's `iss` equals `{BARBICAN_KEYCLOAK_URL}/realms/{realm}` and
   its `aud` contains `BARBICAN_KEYCLOAK_CLIENT_ID` (needs an audience mapper)
4. Verify token hasn't expired (leeway `BARBICAN_TOKEN_LEEWAY_MS`)
5. Make sure you set the `BARBICAN_`-prefixed names — unprefixed
   `KEYCLOAK_URL` etc. are no longer read

### MongoDB Connection Errors

**Problem:** "Failed to connect to MongoDB"

**Solutions:**
1. Check MongoDB is running: `systemctl status mongod`
2. Verify connection string format
3. Check network connectivity: `telnet localhost 27017`
4. Ensure database and collection exist

### Port Already in Use

**Problem:** "Address already in use"

**Solutions:**
```bash
# Find process using port
lsof -i :8080

# Kill process
kill -9 <PID>

# Or use different port
BARBICAN_BIND_ADDRESS=127.0.0.1:8081
```

### Compilation Errors

**Problem:** Various compilation errors

**Solutions:**
```bash
# Update dependencies
cargo update

# Clean and rebuild
cargo clean
cargo build --package barbican

# Check Rust version
rustc --version  # Should be 1.70+
```

---

## Deployment

### Docker Build

```dockerfile
# Dockerfile
FROM rust:1.70 as builder
WORKDIR /app
COPY . .
RUN cargo build --release --package barbican

FROM debian:bullseye-slim
RUN apt-get update && apt-get install -y ca-certificates
COPY --from=builder /app/target/release/barbican /usr/local/bin/
EXPOSE 8080 9091
CMD ["barbican"]
```

```bash
docker build -t barbican:latest .
docker run -p 8080:8080 barbican:latest
```

### Kubernetes Deployment

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: barbican
spec:
  replicas: 3
  selector:
    matchLabels:
      app: barbican
  template:
    metadata:
      labels:
        app: barbican
    spec:
      containers:
      - name: barbican
        image: barbican:latest
        ports:
        - containerPort: 8080
        - containerPort: 9091
          name: metrics
        env:
        - name: BARBICAN_MONGODB_URI
          valueFrom:
            secretKeyRef:
              name: barbican-secrets
              key: mongodb-uri
        readinessProbe:
          httpGet:
            path: /ready
            port: 8080
          initialDelaySeconds: 5
          periodSeconds: 10
        livenessProbe:
          httpGet:
            path: /health
            port: 8080
          initialDelaySeconds: 15
          periodSeconds: 20
```

### Health Checks

```bash
# Health endpoint (no auth required)
curl http://localhost:8080/health

# Readiness endpoint (pings MongoDB):
#   200 {"status":"ready","mongodb":"connected"}
#   503 {"status":"not_ready","mongodb":"disconnected"}
curl http://localhost:8080/ready

# Metrics endpoint
curl http://localhost:9091/metrics
```

---

## Contributing

### Pull Request Process

1. **Fork the repository**
2. **Create feature branch**: `git checkout -b feature/my-feature`
3. **Make changes** with tests
4. **Run linter**: `cargo clippy --package barbican`
5. **Format code**: `cargo fmt`
6. **Run tests**: `cargo test --package barbican`
7. **Update documentation**
8. **Submit PR** with clear description

### Commit Message Format

```
type(scope): subject

body (optional)

footer (optional)
```

**Types:**
- `feat`: New feature
- `fix`: Bug fix
- `docs`: Documentation changes
- `style`: Code style changes (formatting, etc.)
- `refactor`: Code refactoring
- `test`: Test additions or modifications
- `chore`: Build process or auxiliary tool changes

**Example:**
```
feat(handlers): add inbox archive endpoint

Implemented POST /api/v1/inbox/{id}/archive to allow users to
archive inboxes without deleting them.

Closes #123
```

---

*See also:*
- [Architecture](architecture.md) - System design
- [API Reference](api-reference.md) - Endpoint documentation
- [Configuration](configuration.md) - Environment variables