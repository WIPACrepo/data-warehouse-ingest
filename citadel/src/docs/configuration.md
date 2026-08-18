# Configuration

Complete guide to configuring Citadel via environment variables.

## Table of Contents

- [Overview](#overview)
- [HTTP Server](#http-server)
- [Storage Paths](#storage-paths)
- [MongoDB](#mongodb)
- [Keycloak Authentication](#keycloak-authentication)
- [Job Execution](#job-execution)
- [Transfer Settings](#transfer-settings)
- [Callback Authentication](#callback-authentication)
- [Barbican API Coordination](#barbican-api-coordination)
- [Polling & Progress](#polling--progress)
- [Job Queue Limits](#job-queue-limits)
- [Integration Retries](#integration-retries)
- [Example Configurations](#example-configurations)
- [Environment Variables Quick Reference](#environment-variables-quick-reference)

---

## Overview

Citadel is configured entirely through environment variables, loaded in [`src/config.rs`](../src/config.rs) by `CitadelConfig::from_env()`. Each variable falls back to a sensible default, so a bare `cargo run --bin citadel` works with defaults (though `CITADEL_JOB_EXECUTOR=kubernetes` will try to connect to a cluster — use `mock` for local development).

### Configuration Loading

```rust
pub fn from_env() -> Result<Self, ConfigError> {
    let mut config = Self::default();
    // ... override each field if the env var is present ...
    Ok(config)
}
```

> **Note**: Unlike Barbican, Citadel's `from_env` does not validate required values in code; missing `CITADEL_KEYCLOAK_CLIENT_SECRET` simply leaves the default empty string. Set it in production.

---

## HTTP Server

| Variable | Default | Description |
|----------|---------|-------------|
| `CITADEL_BIND_ADDRESS` | `127.0.0.1:8081` | Main HTTP API bind address |
| `CITADEL_PROMETHEUS_PORT` | `9090` | Port for the Prometheus metrics endpoint (separate server) |

**Examples:**
```bash
CITADEL_BIND_ADDRESS=127.0.0.1:8081        # local dev
CITADEL_BIND_ADDRESS=0.0.0.0:8081          # container (all interfaces)
CITADEL_PROMETHEUS_PORT=9090
```

---

## Storage Paths

| Variable | Default | Description |
|----------|---------|-------------|
| `CITADEL_INBOX_BASE_PATH` | `./data/inboxes` | Base path for inbox directories |
| `CITADEL_PROTECTED_BASE_PATH` | `./data/protected` | Base path for the protected warehouse |

Inbox directories are created as `{INBOX_BASE_PATH}/{inbox_id}`; published data lands at `{PROTECTED_BASE_PATH}/{warehouse_path}`.

**Examples:**
```bash
CITADEL_INBOX_BASE_PATH=./data/inboxes      # dev
CITADEL_PROTECTED_BASE_PATH=/var/data/protected
```

---

## MongoDB

| Variable | Default | Description |
|----------|---------|-------------|
| `CITADEL_MONGODB_URI` | `mongodb://localhost:27017` | MongoDB connection string |
| `CITADEL_MONGODB_DATABASE` | `data_warehouse` | Database name |

Citadel uses three collections (names are constants in [`src/db.rs`](../src/db.rs)):

| Collection | Owner | Purpose |
|------------|-------|---------|
| `inboxes` | Barbican (shared) | Read-only access for Citadel |
| `jobs` | Citadel | Job queue storage |
| `callback_nonces` | Citadel | Replay-attack prevention |

```bash
CITADEL_MONGODB_URI=mongodb://localhost:27017
CITADEL_MONGODB_DATABASE=data_warehouse
```

---

## Keycloak Authentication

| Variable | Default | Description |
|----------|---------|-------------|
| `CITADEL_KEYCLOAK_URL` | `http://localhost:8080` | Keycloak server URL |
| `CITADEL_KEYCLOAK_REALM` | `data-warehouse` | Keycloak realm name |
| `CITADEL_KEYCLOAK_CLIENT_ID` | `citadel` | Client ID for audience validation |
| `CITADEL_KEYCLOAK_CLIENT_SECRET` | *(empty)* | Client secret |
| `CITADEL_REQUIRED_ROLES` | `read,write` | Accepted roles (comma-separated) |
| `CITADEL_ALLOWED_ALGORITHMS` | `RS256` | Allowed JWT algorithms (comma-separated) |
| `CITADEL_JWKS_REFRESH_SECS` | `60` | JWKS cache refresh interval |

**Derived URLs** (constructed in `config.rs`):

```
JWKS:    {CITADEL_KEYCLOAK_URL}/auth/realms/{CITADEL_KEYCLOAK_REALM}/protocol/openid-connect/certs
Issuer:  {CITADEL_KEYCLOAK_URL}/auth/realms/{CITADEL_KEYCLOAK_REALM}
```

```bash
CITADEL_KEYCLOAK_URL=http://localhost:8080
CITADEL_KEYCLOAK_REALM=data-warehouse
CITADEL_KEYCLOAK_CLIENT_ID=citadel
CITADEL_KEYCLOAK_CLIENT_SECRET=dev-secret
CITADEL_REQUIRED_ROLES=read,write
CITADEL_ALLOWED_ALGORITHMS=RS256
CITADEL_JWKS_REFRESH_SECS=60
```

> **Security**: never set `CITADEL_ALLOWED_ALGORITHMS=none`; only asymmetric algorithms (RS256, RS384, RS512, ES256, ES384, EdDSA) are supported by the validator.

---

## Job Execution

| Variable | Default | Description |
|----------|---------|-------------|
| `CITADEL_JOB_EXECUTOR` | `kubernetes` | `kubernetes` or `mock` |
| `CITADEL_KUBE_CONFIG_PATH` | *(unset)* | Path to Kubernetes config file |
| `CITADEL_KUBE_NAMESPACE` | `citadel-jobs` | Namespace for K8s jobs |
| `CITADEL_JOB_IMAGE` | `citadel:latest` | Container image for K8s jobs |
| `CITADEL_JOB_BINARY` | `citadel-job` | Binary name within the image |
| `CITADEL_JOB_MAX_DURATION_SECS` | `86400` | Maximum job duration (24h); timeout enforcement planned |

```bash
# Local development: no cluster needed
CITADEL_JOB_EXECUTOR=mock

# Production
CITADEL_JOB_EXECUTOR=kubernetes
CITADEL_KUBE_CONFIG_PATH=/etc/kubernetes/config
CITADEL_KUBE_NAMESPACE=citadel-jobs
CITADEL_JOB_IMAGE=registry.example.com/citadel:latest
CITADEL_JOB_BINARY=citadel-job
CITADEL_JOB_MAX_DURATION_SECS=86400
```

> **Status**: the `KubernetesExecutor` is a stub returning "not implemented"; the `MockExecutor` returns immediately in the current code (`kubernetes.rs` spec creation also unimplemented). See [Handoff Notes](../HANDOFF.md).

---

## Transfer Settings

| Variable | Default | Description |
|----------|---------|-------------|
| `CITADEL_TRANSFER_PARALLELISM` | `4` | Max concurrent file transfers per job |
| `CITADEL_TRANSFER_CHUNK_SIZE_MB` | `16` | Chunk size (MB) for streaming copies |

**Memory bound**: `CITADEL_TRANSFER_CHUNK_SIZE_MB × CITADEL_TRANSFER_PARALLELISM` (e.g. 16 MB × 4 = 64 MB max per job).

```bash
CITADEL_TRANSFER_PARALLELISM=4
CITADEL_TRANSFER_CHUNK_SIZE_MB=16
```

---

## Callback Authentication

| Variable | Default | Description |
|----------|---------|-------------|
| `CITADEL_JOB_CALLBACK_SECRET_FILE` | `/etc/citadel/secrets/callback-secret` | Path to the shared callback secret file |

The secret is read by `citadel-job` and included in callbacks. Server-side secret *validation* is a planned TODO in the handler; **timestamp + nonce** validation is active today.

```bash
CITADEL_JOB_CALLBACK_SECRET_FILE=/etc/citadel/secrets/callback-secret
```

---

## Barbican API Coordination

| Variable | Default | Description |
|----------|---------|-------------|
| `CITADEL_BARBICAN_API_URL` | `http://barbican:8080` | Barbican API base URL |
| `CITADEL_BARBICAN_API_RETRY_MAX` | `5` | Max retries for Barbican API calls |
| `CITADEL_BARBICAN_API_RETRY_BACKOFF_MS` | `1000` | Initial backoff (doubles, max 30s) |
| `CITADEL_BARBICAN_API_TIMEOUT_SECS` | `10` | Per-request timeout |

```bash
CITADEL_BARBICAN_API_URL=http://barbican:8080
CITADEL_BARBICAN_API_RETRY_MAX=5
CITADEL_BARBICAN_API_RETRY_BACKOFF_MS=1000
CITADEL_BARBICAN_API_TIMEOUT_SECS=10
```

---

## Polling & Progress

| Variable | Default | Description |
|----------|---------|-------------|
| `CITADEL_JOB_METADATA_POLL_INTERVAL_SECS` | `15` | Inbox metadata polling interval during transfer (planned usage) |
| `CITADEL_PROGRESS_UPDATE_INTERVAL_SECS` | `30` | Minimum interval between progress updates in MongoDB |

```bash
CITADEL_JOB_METADATA_POLL_INTERVAL_SECS=15
CITADEL_PROGRESS_UPDATE_INTERVAL_SECS=30
```

> **Status**: config fields exist; metadata polling during transfer and periodic progress updates are **planned** (see [Handoff Notes](../HANDOFF.md)).

---

## Job Queue Limits

| Variable | Default | Description |
|----------|---------|-------------|
| `CITADEL_QUEUE_PUBLISH_RATE_LIMIT` | `10` | Max concurrent publish jobs |
| `CITADEL_QUEUE_DELETE_RATE_LIMIT` | `20` | Max concurrent delete jobs |
| `CITADEL_QUEUE_PUBLISH_CPU_LIMIT` | `"2"` | K8s CPU limit for publish jobs (planned usage) |
| `CITADEL_QUEUE_PUBLISH_MEMORY_LIMIT` | `"4Gi"` | K8s memory limit for publish jobs (planned usage) |
| `CITADEL_QUEUE_DELETE_CPU_LIMIT` | `"1"` | K8s CPU limit for delete jobs (planned usage) |
| `CITADEL_QUEUE_DELETE_MEMORY_LIMIT` | `"2Gi"` | K8s memory limit for delete jobs (planned usage) |

```bash
CITADEL_QUEUE_PUBLISH_RATE_LIMIT=10
CITADEL_QUEUE_DELETE_RATE_LIMIT=20
```

---

## Integration Retries

Configuration for the (planned) catalog and LTA notification retries.

| Variable | Default | Description |
|----------|---------|-------------|
| `CITADEL_CATALOG_NOTIFY_RETRY_MAX` | `3` | Max retries for catalog notifications |
| `CITADEL_CATALOG_NOTIFY_RETRY_BACKOFF_MS` | `5000` | Backoff between catalog notify retries |
| `CITADEL_CATALOG_NOTIFY_TIMEOUT_SECS` | `30` | Timeout for catalog notify requests |
| `CITADEL_LTA_NOTIFY_RETRY_MAX` | `3` | Max retries for LTA notifications |
| `CITADEL_LTA_NOTIFY_RETRY_BACKOFF_MS` | `5000` | Backoff between LTA notify retries |
| `CITADEL_LTA_NOTIFY_TIMEOUT_SECS` | `30` | Timeout for LTA notify requests |

```bash
CITADEL_CATALOG_NOTIFY_RETRY_MAX=3
CITADEL_CATALOG_NOTIFY_RETRY_BACKOFF_MS=5000
CITADEL_CATALOG_NOTIFY_TIMEOUT_SECS=30
CITADEL_LTA_NOTIFY_RETRY_MAX=3
CITADEL_LTA_NOTIFY_RETRY_BACKOFF_MS=5000
CITADEL_LTA_NOTIFY_TIMEOUT_SECS=30
```

---

## Example Configurations

### Development Environment

```bash
# HTTP Server
CITADEL_BIND_ADDRESS=127.0.0.1:8081
CITADEL_PROMETHEUS_PORT=9090

# Storage
CITADEL_INBOX_BASE_PATH=./data/inboxes
CITADEL_PROTECTED_BASE_PATH=./data/protected

# MongoDB
CITADEL_MONGODB_URI=mongodb://localhost:27017
CITADEL_MONGODB_DATABASE=data_warehouse

# Keycloak
CITADEL_KEYCLOAK_URL=http://localhost:8080
CITADEL_KEYCLOAK_REALM=data-warehouse
CITADEL_KEYCLOAK_CLIENT_ID=citadel
CITADEL_KEYCLOAK_CLIENT_SECRET=dev-secret
CITADEL_REQUIRED_ROLES=read,write
CITADEL_ALLOWED_ALGORITHMS=RS256

# Job executor (mock = no cluster)
CITADEL_JOB_EXECUTOR=mock

# Barbican
CITADEL_BARBICAN_API_URL=http://localhost:8080

# Transfer
CITADEL_TRANSFER_PARALLELISM=4
CITADEL_TRANSFER_CHUNK_SIZE_MB=16

# Callback
CITADEL_JOB_CALLBACK_SECRET_FILE=./dev/callback-secret

# Logging
RUST_LOG=debug
```

### Production Environment (Kubernetes)

```bash
# HTTP Server
CITADEL_BIND_ADDRESS=0.0.0.0:8081
CITADEL_PROMETHEUS_PORT=9090

# Storage (PersistentVolume paths)
CITADEL_INBOX_BASE_PATH=/data/inboxes
CITADEL_PROTECTED_BASE_PATH=/data/protected

# MongoDB
CITADEL_MONGODB_URI=mongodb://mongo-0:27017,mongo-1:27017,mongo-2:27017/?replicaSet=rs0
CITADEL_MONGODB_DATABASE=data_warehouse

# Keycloak
CITADEL_KEYCLOAK_URL=https://auth.example.com
CITADEL_KEYCLOAK_REALM=data-warehouse-prod
CITADEL_KEYCLOAK_CLIENT_ID=citadel-service
CITADEL_KEYCLOAK_CLIENT_SECRET=${CITADEL_SECRET_FROM_VAULT}
CITADEL_REQUIRED_ROLES=read,write
CITADEL_ALLOWED_ALGORITHMS=RS256,RS384

# Job executor
CITADEL_JOB_EXECUTOR=kubernetes
CITADEL_KUBE_NAMESPACE=citadel-jobs
CITADEL_JOB_IMAGE=registry.example.com/citadel:latest
CITADEL_JOB_MAX_DURATION_SECS=86400

# Barbican
CITADEL_BARBICAN_API_URL=http://barbican.data-warehouse.svc.cluster.local:8080
CITADEL_BARBICAN_API_RETRY_MAX=5
CITADEL_BARBICAN_API_TIMEOUT_SECS=5

# Transfer
CITADEL_TRANSFER_PARALLELISM=8
CITADEL_TRANSFER_CHUNK_SIZE_MB=16

# Callback secret (Kubernetes Secret mounted as file)
CITADEL_JOB_CALLBACK_SECRET_FILE=/etc/citadel/secrets/callback-secret

# Logging
RUST_LOG=info,citadel=info
```

---

## Environment Variables Quick Reference

| Variable | Default |
|----------|---------|
| `CITADEL_BIND_ADDRESS` | `127.0.0.1:8081` |
| `CITADEL_PROMETHEUS_PORT` | `9090` |
| `CITADEL_INBOX_BASE_PATH` | `./data/inboxes` |
| `CITADEL_PROTECTED_BASE_PATH` | `./data/protected` |
| `CITADEL_MONGODB_URI` | `mongodb://localhost:27017` |
| `CITADEL_MONGODB_DATABASE` | `data_warehouse` |
| `CITADEL_KEYCLOAK_URL` | `http://localhost:8080` |
| `CITADEL_KEYCLOAK_REALM` | `data-warehouse` |
| `CITADEL_KEYCLOAK_CLIENT_ID` | `citadel` |
| `CITADEL_KEYCLOAK_CLIENT_SECRET` | *(empty)* |
| `CITADEL_REQUIRED_ROLES` | `read,write` |
| `CITADEL_ALLOWED_ALGORITHMS` | `RS256` |
| `CITADEL_JWKS_REFRESH_SECS` | `60` |
| `CITADEL_JOB_EXECUTOR` | `kubernetes` |
| `CITADEL_KUBE_CONFIG_PATH` | *(unset)* |
| `CITADEL_KUBE_NAMESPACE` | `citadel-jobs` |
| `CITADEL_JOB_IMAGE` | `citadel:latest` |
| `CITADEL_JOB_BINARY` | `citadel-job` |
| `CITADEL_JOB_MAX_DURATION_SECS` | `86400` |
| `CITADEL_TRANSFER_PARALLELISM` | `4` |
| `CITADEL_TRANSFER_CHUNK_SIZE_MB` | `16` |
| `CITADEL_JOB_CALLBACK_SECRET_FILE` | `/etc/citadel/secrets/callback-secret` |
| `CITADEL_BARBICAN_API_URL` | `http://barbican:8080` |
| `CITADEL_BARBICAN_API_RETRY_MAX` | `5` |
| `CITADEL_BARBICAN_API_RETRY_BACKOFF_MS` | `1000` |
| `CITADEL_BARBICAN_API_TIMEOUT_SECS` | `10` |
| `CITADEL_JOB_METADATA_POLL_INTERVAL_SECS` | `15` |
| `CITADEL_PROGRESS_UPDATE_INTERVAL_SECS` | `30` |
| `CITADEL_QUEUE_PUBLISH_RATE_LIMIT` | `10` |
| `CITADEL_QUEUE_DELETE_RATE_LIMIT` | `20` |
| `CITADEL_QUEUE_PUBLISH_CPU_LIMIT` | `"2"` |
| `CITADEL_QUEUE_PUBLISH_MEMORY_LIMIT` | `"4Gi"` |
| `CITADEL_QUEUE_DELETE_CPU_LIMIT` | `"1"` |
| `CITADEL_QUEUE_DELETE_MEMORY_LIMIT` | `"2Gi"` |
| `CITADEL_CATALOG_NOTIFY_RETRY_MAX` | `3` |
| `CITADEL_CATALOG_NOTIFY_RETRY_BACKOFF_MS` | `5000` |
| `CITADEL_CATALOG_NOTIFY_TIMEOUT_SECS` | `30` |
| `CITADEL_LTA_NOTIFY_RETRY_MAX` | `3` |
| `CITADEL_LTA_NOTIFY_RETRY_BACKOFF_MS` | `5000` |
| `CITADEL_LTA_NOTIFY_TIMEOUT_SECS` | `30` |

---

## Validation & Hot Reload

### Validation

The current `from_env()` does not fail on missing/invalid values beyond parse errors for numeric fields (unparseable integers return `ConfigError::InvalidValue`). Required-secret validation is **planned** to match Barbican's behavior.

### Hot Reload

Like Barbican, Citadel does **not** hot-reload main configuration. The planned exception is the **callback secret**: `PLAN.md` specifies watching `CITADEL_JOB_CALLBACK_SECRET_FILE` with the `notify` crate and reloading atomically (dependency present; watcher not yet wired).

---

*See also:*
- [Architecture](architecture.md) - System design
- [Authentication](authentication.md) - Keycloak/JWT settings
- [Barbican Integration](barbican-integration.md) - Barbican coordination settings
- [Development Guide](development.md) - Building and running locally
- [Design Plan](../PLAN.md) Section 2 - Full configuration spec
