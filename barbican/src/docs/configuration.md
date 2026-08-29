# Configuration

Complete guide to configuring Barbican via environment variables.

## Table of Contents

- [Overview](#overview)
- [Required Configuration](#required-configuration)
- [HTTP Server](#http-server)
- [MongoDB](#mongodb)
- [Keycloak Authentication](#keycloak-authentication)
- [Citadel Integration](#citadel-integration)
- [File System](#file-system)
- [Observability](#observability)
- [Security Settings](#security-settings)
- [Example Configurations](#example-configurations)

---

## Overview

Barbican is configured entirely through environment variables. The configuration is loaded at startup and validated before the service begins accepting requests.

### Configuration Loading

```rust
pub fn from_env() -> Result<Self, ConfigError> {
    let config = Config::default();
    config.validate()?;
    Ok(config)
}
```

### Validation Rules

- All required variables must be set
- Numeric values must be parseable
- URLs must be valid
- Security settings must meet minimum requirements

---

## Required Configuration

These variables **must** be set for Barbican to start:

| Variable | Description | Example |
|----------|-------------|---------|
| `BARBICAN_KEYCLOAK_CLIENT_SECRET` | Keycloak client secret for JWT validation | `super-secret-value` |

**Error on Missing Required:**
```json
{
  "error": "CONFIG_VALIDATION_FAILED",
  "message": "Missing required environment variable: BARBICAN_KEYCLOAK_CLIENT_SECRET"
}
```

---

## HTTP Server

Configuration for the Barbican HTTP server.

| Variable | Default | Description |
|----------|---------|-------------|
| `BARBICAN_BIND_ADDRESS` | `127.0.0.1:8080` | Address and port to bind HTTP server |

### Examples

**Development:**
```bash
BARBICAN_BIND_ADDRESS=127.0.0.1:8080
```

**Production (all interfaces):**
```bash
BARBICAN_BIND_ADDRESS=0.0.0.0:8080
```

**Production (specific interface):**
```bash
BARBICAN_BIND_ADDRESS=10.0.1.5:8080
```

---

## MongoDB

Configuration for MongoDB connectivity.

| Variable | Default | Description |
|----------|---------|-------------|
| `BARBICAN_MONGODB_URI` | `mongodb://localhost:27017` | MongoDB connection string |
| `BARBICAN_MONGODB_DATABASE` | `data_warehouse` | Database name |
| `BARBICAN_MONGODB_INBOX_COLLECTION` | `inboxes` | Collection name for inbox metadata |
| `BARBICAN_MONGODB_TIMEOUT_SECS` | `30` | **Reserved/unused** — parsed but not read by any code path |
| `BARBICAN_MONGODB_RETRY_MAX` | `3` | **Reserved/unused** — parsed but not read by any code path |

> The unique index on `warehouse_path` (`uniq_warehouse_path`, created
> idempotently at startup — P1.5/D3) lives in this database's `inboxes`
> collection; a duplicate insert surfaces as `409 Conflict`.

### Connection String Formats

**Single Node:**
```bash
BARBICAN_MONGODB_URI=mongodb://localhost:27017
```

**Replica Set:**
```bash
BARBICAN_MONGODB_URI=mongodb://mongo1:27017,mongo2:27017,mongo3:27017/?replicaSet=rs0
```

**With Authentication:**
```bash
BARBICAN_MONGODB_URI=mongodb://barbican:password@localhost:27017/data_warehouse?authSource=admin
```

**With TLS:**
```bash
BARBICAN_MONGODB_URI=mongodb://localhost:27017/data_warehouse?tls=true&tlsCAFile=/etc/ssl/certs/ca.pem
```

### Recommended Settings

**Production:**
```bash
BARBICAN_MONGODB_URI=mongodb://mongo1:27017,mongo2:27017,mongo3:27017/?replicaSet=rs0&readPreference=primaryPreferred
```

---

## Keycloak Authentication

Configuration for JWT authentication via Keycloak.

| Variable | Default | Description |
|----------|---------|-------------|
| `BARBICAN_KEYCLOAK_URL` | `http://localhost:8080` | Keycloak server base URL |
| `BARBICAN_KEYCLOAK_REALM` | `data-warehouse` | Keycloak realm name |
| `BARBICAN_KEYCLOAK_CLIENT_ID` | `barbican` | Client ID for audience validation |
| `BARBICAN_KEYCLOAK_CLIENT_SECRET` | *(optional)* | Only checked for **non-emptiness** at startup; JWT validation is JWKS-based, the secret and `token_url()` are not used by any code path |
| `BARBICAN_REQUIRED_ROLES` | `read,write` | Comma-separated list of accepted roles (external routes) |
| `BARBICAN_ALLOWED_ALGORITHMS` | `RS256` | Comma-separated list of allowed JWT algorithms |
| `BARBICAN_JWKS_REFRESH_SECS` | `60` | Minimum seconds between JWKS refreshes |

> There is **no `BARBICAN_INTERNAL_ROLES`** (D9): with the removal of the
> `internal` role, the state-transition endpoints are gated only by the
> generic auth layer (valid Bearer JWT, no role requirement). Trust is
> delegated to Citadel, the only caller of those endpoints.

### Derived URLs

Barbican automatically constructs these URLs from the base configuration:

**JWKS URL:**
```
{BARBICAN_KEYCLOAK_URL}/realms/{BARBICAN_KEYCLOAK_REALM}/protocol/openid-connect/certs
```

### Example Configuration

**Development:**
```bash
BARBICAN_KEYCLOAK_URL=http://localhost:8080
BARBICAN_KEYCLOAK_REALM=data-warehouse
BARBICAN_KEYCLOAK_CLIENT_ID=barbican-dev
BARBICAN_KEYCLOAK_CLIENT_SECRET=dev-secret-123
BARBICAN_REQUIRED_ROLES=read,write
BARBICAN_ALLOWED_ALGORITHMS=RS256
BARBICAN_JWKS_REFRESH_SECS=60
```

**Production:**
```bash
BARBICAN_KEYCLOAK_URL=https://auth.example.com
BARBICAN_KEYCLOAK_REALM=data-warehouse-prod
BARBICAN_KEYCLOAK_CLIENT_ID=barbican-service
BARBICAN_KEYCLOAK_CLIENT_SECRET=${SECRET_FROM_VAULT}
BARBICAN_REQUIRED_ROLES=read,write
BARBICAN_ALLOWED_ALGORITHMS=RS256,RS384,RS512
BARBICAN_JWKS_REFRESH_SECS=300
```

### Security Considerations

**Algorithm Whitelist:**
- ✅ Allow: `RS256`, `RS384`, `RS512`, `ES256`, `ES384`, `ES512`
- ❌ Never Allow: `none`, `HS256`, `HS384`, `HS512`

**Role Configuration:**
- Use separate clients for external users and internal services
- The state-transition endpoints are reachable by any valid Bearer token
  (no role gate) — keep them off the public network; Citadel is the only
  legitimate caller (see [Authentication](authentication.md))
- Regularly rotate client secrets

---

## Citadel Integration

Configuration for communication with the Citadel backend service.

| Variable | Default | Description |
|----------|---------|-------------|
| `BARBICAN_CITADEL_API_URL` | `http://citadel:8081` | Citadel API base URL |
| `BARBICAN_CITADEL_API_TIMEOUT_SECS` | `30` | HTTP request timeout |
| `BARBICAN_CITADEL_API_RETRY_MAX` | `3` | Maximum retry attempts |
| `BARBICAN_CITADEL_SERVICE_TOKEN_FILE` | *(optional)* | Path to JWT token file for authenticating to Citadel |

### Automatic Trigger Configuration

Barbican automatically triggers Citadel for these operations:
- Inbox creation (`/create/start`)
- Inbox publishing (`/publish/start`)
- Inbox deletion (`/delete/start`)

### Example Configuration

**Development:**
```bash
BARBICAN_CITADEL_API_URL=http://localhost:8081
BARBICAN_CITADEL_API_TIMEOUT_SECS=60
BARBICAN_CITADEL_API_RETRY_MAX=5
BARBICAN_CITADEL_SERVICE_TOKEN_FILE=./dev/citadel.jwt
```

**Production:**
```bash
BARBICAN_CITADEL_API_URL=http://citadel.data-warehouse.svc.cluster.local:8081
BARBICAN_CITADEL_API_TIMEOUT_SECS=30
BARBICAN_CITADEL_API_RETRY_MAX=3
BARBICAN_CITADEL_SERVICE_TOKEN_FILE=/run/secrets/citadel-service-jwt
```

### Retry Behavior

Citadel triggers use exponential backoff:

```
Attempt 1: Immediate
Attempt 2: After 1 second
Attempt 3: After 2 seconds
Attempt 4: After 4 seconds (if retry_max >= 4)
```

---

## File System

| Variable | Default | Description |
|----------|---------|-------------|
| `BARBICAN_INBOX_BASE_PATH` | `./data/inboxes` | **Reserved/unused** — parsed but not read by any code path. Barbican is a metadata-only service; the physical inbox/protected directories belong to Citadel. |

### Warehouse-path containment

Barbican has **no** `validate_path` helper (there is no such function in the
codebase). Containment of the physical warehouse path is enforced by
**Citadel**: `CITADEL_PROTECTED_BASE_PATH` + `validate_warehouse_path`
reject any target outside the protected base. Barbican's duplicate-path guard
is the unique `warehouse_path` index (see [MongoDB](#mongodb)).

### Example Configuration

**Development:**
```bash
BARBICAN_INBOX_BASE_PATH=./data/inboxes
```

**Production:**
```bash
BARBICAN_INBOX_BASE_PATH=/var/data/inboxes
```

---

## Observability

Configuration for monitoring and metrics.

| Variable | Default | Description |
|----------|---------|-------------|
| `BARBICAN_PROMETHEUS_PORT` | `9091` | Port for Prometheus metrics endpoint |

### Metrics Endpoint

```http
GET /metrics
Host: barbican:9091
```

Served from **Barbican's own registry** via
`common::start_metrics_server_with_router` (bound `0.0.0.0:{prometheus_port}`).
Two Barbican-owned instruments (P4.3):

- `barbican_inboxes_total{status}` — a **gauge** refreshed on a 30 s
  background interval (`find_many` grouped by status; `set()` semantics, so
  unused status labels report 0 rather than lingering).
- `barbican_api_requests_total{endpoint,method,status}` — a per-request
  counter recorded by middleware; `endpoint` is the matched route template
  (e.g. `/api/v1/inbox/{id}/publish/complete`), keeping cardinality bounded.

**Example Metrics:**
```
# HELP barbican_inboxes_total Number of inboxes by status (refreshed periodically)
# TYPE barbican_inboxes_total gauge
barbican_inboxes_total{status="active"} 5
barbican_inboxes_total{status="creating"} 0
barbican_inboxes_total{status="ready"} 45
barbican_inboxes_total{status="publishing"} 8
barbican_inboxes_total{status="published"} 120
barbican_inboxes_total{status="failed"} 3
barbican_inboxes_total{status="deleting"} 2
barbican_inboxes_total{status="deleted"} 89

# HELP barbican_api_requests_total HTTP requests handled by Barbican
# TYPE barbican_api_requests_total counter
barbican_api_requests_total{endpoint="/api/v1/inbox",method="POST",status="202"} 150
barbican_api_requests_total{endpoint="/api/v1/inbox/{id}/publish/complete",method="POST",status="200"} 12
```

### Logging

Barbican uses `tracing` for structured logging. Configure log level via:

```bash
RUST_LOG=info,barbican=debug
```

**Log Levels:**
- `error` - Critical failures only
- `warn` - Warnings and errors
- `info` - General operational information (default)
- `debug` - Detailed debugging information
- `trace` - Very verbose tracing

---

## Security Settings

### JWT Algorithm Whitelist

**Critical:** Only allow asymmetric algorithms:

```bash
# ✅ Good
BARBICAN_ALLOWED_ALGORITHMS=RS256
BARBICAN_ALLOWED_ALGORITHMS=RS256,RS384,RS512
BARBICAN_ALLOWED_ALGORITHMS=ES256,ES384,ES512

# ❌ Never allow these
BARBICAN_ALLOWED_ALGORITHMS=none
BARBICAN_ALLOWED_ALGORITHMS=HS256
```

### Role Configuration

**External routes** (create/get/list/delete/publish) require at least one of
`CITADEL_REQUIRED_ROLES`-style roles (`read`/`write`/`admin`):
```bash
BARBICAN_REQUIRED_ROLES=read,write
```

**State-transition endpoints** have **no role requirement** (D9): only a
valid Bearer JWT is required. `BARBICAN_INTERNAL_ROLES` does **not exist** —
do not set it.

### JWKS Cache Settings

**Balance between security and performance:**
```bash
# Development (faster refresh)
BARBICAN_JWKS_REFRESH_SECS=30

# Production (standard)
BARBICAN_JWKS_REFRESH_SECS=60

# High security (frequent key rotation)
BARBICAN_JWKS_REFRESH_SECS=30
```

---

## Example Configurations

### Development Environment

```bash
# HTTP Server
BARBICAN_BIND_ADDRESS=127.0.0.1:8080

# MongoDB
BARBICAN_MONGODB_URI=mongodb://localhost:27017
BARBICAN_MONGODB_DATABASE=data_warehouse_dev
BARBICAN_MONGODB_INBOX_COLLECTION=inboxes

# Keycloak
BARBICAN_KEYCLOAK_URL=http://localhost:8080
BARBICAN_KEYCLOAK_REALM=data-warehouse
BARBICAN_KEYCLOAK_CLIENT_ID=barbican-dev
BARBICAN_KEYCLOAK_CLIENT_SECRET=dev-secret-change-me
BARBICAN_REQUIRED_ROLES=read,write,admin
BARBICAN_ALLOWED_ALGORITHMS=RS256
BARBICAN_JWKS_REFRESH_SECS=30

# Citadel
BARBICAN_CITADEL_API_URL=http://localhost:8081
BARBICAN_CITADEL_API_TIMEOUT_SECS=60
BARBICAN_CITADEL_API_RETRY_MAX=5
BARBICAN_CITADEL_SERVICE_TOKEN_FILE=./dev/citadel.jwt

# File System
BARBICAN_INBOX_BASE_PATH=./data/inboxes

# Observability
BARBICAN_PROMETHEUS_PORT=9091

# Logging
RUST_LOG=debug
```

### Production Environment (Kubernetes)

```bash
# HTTP Server
BARBICAN_BIND_ADDRESS=0.0.0.0:8080

# MongoDB
BARBICAN_MONGODB_URI=mongodb://mongo1:27017,mongo2:27017,mongo3:27017/?replicaSet=rs0
BARBICAN_MONGODB_DATABASE=data_warehouse
BARBICAN_MONGODB_INBOX_COLLECTION=inboxes

# Keycloak
BARBICAN_KEYCLOAK_URL=https://auth.example.com
BARBICAN_KEYCLOAK_REALM=data-warehouse-prod
BARBICAN_KEYCLOAK_CLIENT_ID=barbican-service
BARBICAN_KEYCLOAK_CLIENT_SECRET=${SECRET_FROM_VAULT}
BARBICAN_REQUIRED_ROLES=read,write
BARBICAN_ALLOWED_ALGORITHMS=RS256,RS384,RS512
BARBICAN_JWKS_REFRESH_SECS=300

# Citadel
BARBICAN_CITADEL_API_URL=http://citadel.data-warehouse.svc.cluster.local:8081
BARBICAN_CITADEL_API_TIMEOUT_SECS=30
BARBICAN_CITADEL_API_RETRY_MAX=3
BARBICAN_CITADEL_SERVICE_TOKEN_FILE=/run/secrets/citadel-service-jwt

# File System
BARBICAN_INBOX_BASE_PATH=/var/data/inboxes

# Observability
BARBICAN_PROMETHEUS_PORT=9091

# Logging
RUST_LOG=info,barbican=warn
```

### Docker Compose Example

```yaml
version: '3.8'

services:
  barbican:
    image: barbican:latest
    environment:
      - BARBICAN_BIND_ADDRESS=0.0.0.0:8080
      - BARBICAN_MONGODB_URI=mongodb://mongo:27017
      - BARBICAN_KEYCLOAK_URL=http://keycloak:8080
      - BARBICAN_KEYCLOAK_REALM=data-warehouse
      - BARBICAN_KEYCLOAK_CLIENT_ID=barbican
      - BARBICAN_KEYCLOAK_CLIENT_SECRET=${KEYCLOAK_SECRET}
      - BARBICAN_CITADEL_API_URL=http://citadel:8081
      - BARBICAN_CITADEL_SERVICE_TOKEN_FILE=/run/secrets/citadel-jwt
      - BARBICAN_INBOX_BASE_PATH=/data/inboxes
      - BARBICAN_PROMETHEUS_PORT=9091
    volumes:
      - ./data/inboxes:/data/inboxes
      - ./secrets/citadel.jwt:/run/secrets/citadel-jwt:ro
    ports:
      - "8080:8080"
      - "9091:9091"
    depends_on:
      - mongo
      - keycloak
      - citadel
```

---

## Configuration Validation

Barbican validates configuration at startup:

```rust
pub fn validate(&self) -> Result<(), ConfigError> {
    // Check required secrets
    if self.keycloak_client_secret.is_empty() {
        return Err(ConfigError::MissingRequired(
            "BARBICAN_KEYCLOAK_CLIENT_SECRET".to_string(),
        ));
    }

    // Security: reject weak algorithms
    if self.allowed_algorithms.iter().any(|alg| alg == "none") {
        return Err(ConfigError::InvalidValue(
            "BARBICAN_ALLOWED_ALGORITHMS".to_string(),
            "Algorithm 'none' is not allowed".to_string(),
        ));
    }

    // Validate URLs
    if !self.keycloak_url.starts_with("http") {
        return Err(ConfigError::InvalidValue(
            "BARBICAN_KEYCLOAK_URL".to_string(),
            "Must be a valid HTTP/HTTPS URL".to_string(),
        ));
    }

    Ok(())
}
```

### Validation Errors

**Missing Required:**
```json
{
  "error": "CONFIG_VALIDATION_FAILED",
  "message": "Missing required environment variable: BARBICAN_KEYCLOAK_CLIENT_SECRET"
}
```

**Invalid Value:**
```json
{
  "error": "CONFIG_VALIDATION_FAILED",
  "message": "Invalid value for BARBICAN_ALLOWED_ALGORITHMS: Algorithm 'none' is not allowed"
}
```

---

## Hot Reloading

**Note:** Barbican does **not** support hot reloading of configuration. Changes to environment variables require a service restart.

### Safe Restart Procedure

1. Deploy new configuration to orchestration system (Kubernetes, etc.)
2. Wait for new pod to become ready
3. Drain connections from old pod
4. Terminate old pod
5. Verify service health

---

*See also:*
- [Architecture](architecture.md) - System design overview
- [Authentication](authentication.md) - JWT and Keycloak details
- [Citadel Integration](citadel-integration.md) - Backend service configuration