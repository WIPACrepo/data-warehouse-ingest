# Configuration

Complete guide to configuring Barbican via environment variables.

## Table of Contents

- [Overview](#overview)
- [Required Configuration](#required-configuration)
- [HTTP Server](#http-server)
- [MongoDB](#mongodb)
- [Keycloak Authentication](#keycloak-authentication)
- [Citadel Integration](#citadel-integration)
- [Warehouse-path containment](#warehouse-path-containment)
- [Observability](#observability)
- [Security Settings](#security-settings)
- [Example Configurations](#example-configurations)

---

## Overview

Barbican is configured entirely through environment variables, loaded once at
startup by three independent loaders (`barbican/src/main.rs`):

| Loader | Source | Variables |
|--------|--------|-----------|
| `Config::from_env()` | `barbican/src/config.rs` | Service settings: bind address, MongoDB, Citadel client, Prometheus port |
| `AuthConfig::from_env_prefixed("BARBICAN")` | `common/src/auth/config.rs` | Keycloak/JWT settings (`BARBICAN_KEYCLOAK_*`, `BARBICAN_REQUIRED_ROLES`, ...) — decision **D10** |
| `ServiceTokenSource::from_env("BARBICAN", "CITADEL", &auth_config)` | `common/src/auth/service_token.rs` | Bearer token for Barbican → Citadel calls (`BARBICAN_CITADEL_CLIENT_ID`/`_CLIENT_SECRET` or `_SERVICE_TOKEN_FILE`) |

`Config::default()` returns the literal built-in defaults and reads **no**
environment variables; `Config::from_env()` calls
`Config::from_lookup(|name| env::var(name).ok())`, which overlays any set
variables on those defaults (tests call `from_lookup` with a map).

### Configuration Loading

```rust
// barbican/src/main.rs (abridged) — every loader fails fast, before MongoDB.
let config = match Config::from_env() {
    Ok(c) => c,
    Err(e) => {
        error!("Failed to load configuration: {e}");
        std::process::exit(1);
    }
};
let auth_config = match AuthConfig::from_env_prefixed("BARBICAN") {
    Ok(c) => c,
    Err(e) => {
        error!("Failed to load auth configuration: {e}");
        std::process::exit(1);
    }
};
// Option<ServiceTokenSource>; None (nothing configured, non-production)
// logs a warning and sends no Authorization header to Citadel.
let citadel_token = match ServiceTokenSource::from_env("BARBICAN", "CITADEL", &auth_config) {
    Ok(source) => source,
    Err(e) => {
        error!("Invalid Citadel service-token configuration: {e}");
        std::process::exit(1);
    }
};
// ... connect to MongoDB ...
serve(client, config, auth_config, citadel_token).await
```

### Validation Rules

All three loaders validate; any violation is logged and the process exits
with status 1 before MongoDB is contacted.

- `Config::from_env()`: unset variables take their defaults, but a malformed
  `BARBICAN_CITADEL_API_TIMEOUT_SECS` (`u64`), `BARBICAN_CITADEL_API_RETRY_MAX`
  (`u32`) or `BARBICAN_PROMETHEUS_PORT` (`u16`) is an error naming the
  variable (surrounding whitespace is trimmed). These used to fall back to
  the default silently.
- `AuthConfig::from_env_prefixed`: see
  [Keycloak Authentication](#keycloak-authentication).
- `ServiceTokenSource::from_env`: see
  [Citadel service token](#citadel-service-token).

---

## Required Configuration

No variable is strictly required for local development: every setting has a
default, and without a Citadel service token Barbican starts with a warning
(its calls to Citadel are then rejected). In production (`PRODUCTION=true`):

- `BARBICAN_KEYCLOAK_URL` **must** be set to a non-default value — the
  `http://localhost:8080` default is rejected at startup;
- `BARBICAN_CITADEL_CLIENT_ID` and `BARBICAN_CITADEL_CLIENT_SECRET` **must**
  be set (static-file mode and no token are both rejected).

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

Configuration for JWT authentication via Keycloak. Parsed by the shared
`common::AuthConfig::from_env_prefixed("BARBICAN")` (decision **D10**), so
Barbican and Citadel validate JWTs identically while each service keeps its
own variable namespace. Barbican's `Config` has **no** auth fields.

| Variable | Default | Description |
|----------|---------|-------------|
| `BARBICAN_KEYCLOAK_URL` | `http://localhost:8080` | Keycloak base URL **including any path prefix** (legacy Keycloak < 17 needs `.../auth`). A trailing `/` is trimmed. Rejected when `PRODUCTION=true` and left at the default |
| `BARBICAN_KEYCLOAK_REALM` | `data-warehouse` | Keycloak realm name |
| `BARBICAN_KEYCLOAK_CLIENT_ID` | `barbican` | Expected `aud` claim. Keycloak only puts the client ID in `aud` when an **audience mapper** is configured on the client |
| `BARBICAN_REQUIRED_ROLES` | `read,write` | Comma-separated roles accepted on external routes (any one grants access; `admin` always passes). Must name at least one role |
| `BARBICAN_ALLOWED_ALGORITHMS` | `RS256` | Comma-separated JWT algorithm allow-list. `HS*` and `none` are rejected |
| `BARBICAN_JWKS_REFRESH_SECS` | `60` | JWKS cache refresh interval in seconds (minimum `60`) |
| `BARBICAN_TOKEN_LEEWAY_MS` | `5000` | Clock-skew leeway for `exp`/`nbf` in milliseconds (maximum `60000`) |
| `PRODUCTION` *(global, unprefixed)* | unset | When `true`, the localhost default `BARBICAN_KEYCLOAK_URL` is rejected, and Citadel client credentials become mandatory (see [Citadel service token](#citadel-service-token)) |

> **Removed variables.** `BARBICAN_KEYCLOAK_CLIENT_SECRET` no longer exists
> (it was required at startup but never used — validation is JWKS-based).
> The unprefixed `KEYCLOAK_URL`, `KEYCLOAK_REALM`, `KEYCLOAK_CLIENT_ID`,
> `REQUIRED_ROLES`, `ALLOWED_ALGORITHMS`, `JWKS_REFRESH_SECS` and
> `TOKEN_LEEWAY_MS` are **no longer read** by Barbican; previously these were
> what the auth layer actually consumed, so the documented `BARBICAN_*`
> names had no effect. Migrate any deployment that set the unprefixed names.

> There is **no `BARBICAN_INTERNAL_ROLES`** (D9): with the removal of the
> `internal` role, the state-transition endpoints are gated only by the
> generic auth layer (valid Bearer JWT, no role requirement). Trust is
> delegated to Citadel, the only caller of those endpoints.

### Startup validation

`AuthConfig::from_env_prefixed` returns an error (and Barbican exits with
status 1) when:

- a numeric variable (`BARBICAN_JWKS_REFRESH_SECS`, `BARBICAN_TOKEN_LEEWAY_MS`)
  is not a non-negative integer — the message names the variable;
- `BARBICAN_REQUIRED_ROLES` is empty;
- `BARBICAN_ALLOWED_ALGORITHMS` is empty or contains `HS*` / `none`;
- `BARBICAN_JWKS_REFRESH_SECS` < 60 or `BARBICAN_TOKEN_LEEWAY_MS` > 60000;
- `PRODUCTION=true` and `BARBICAN_KEYCLOAK_URL` is the localhost default.

> There is **no `BARBICAN_INTERNAL_ROLES`** (D9): with the removal of the
> `internal` role, the state-transition endpoints are gated only by the
> generic auth layer (valid Bearer JWT, no role requirement). Trust is
> delegated to Citadel, the only caller of those endpoints.

### Derived URLs

Barbican constructs these from the base configuration (no `/auth/` segment
is inserted — include it in `BARBICAN_KEYCLOAK_URL` if your Keycloak needs it):

**Expected issuer (`iss`):**
```
{BARBICAN_KEYCLOAK_URL}/realms/{BARBICAN_KEYCLOAK_REALM}
```

**JWKS URL:**
```
{BARBICAN_KEYCLOAK_URL}/realms/{BARBICAN_KEYCLOAK_REALM}/protocol/openid-connect/certs
```

Legacy Keycloak (< 17) example: `BARBICAN_KEYCLOAK_URL=https://kc.example.com/auth`.

### Example Configuration

**Development:**
```bash
BARBICAN_KEYCLOAK_URL=http://localhost:8080
BARBICAN_KEYCLOAK_REALM=data-warehouse
BARBICAN_KEYCLOAK_CLIENT_ID=barbican-dev
BARBICAN_REQUIRED_ROLES=read,write
BARBICAN_ALLOWED_ALGORITHMS=RS256
BARBICAN_JWKS_REFRESH_SECS=60
BARBICAN_TOKEN_LEEWAY_MS=5000
```

**Production:**
```bash
PRODUCTION=true
BARBICAN_KEYCLOAK_URL=https://auth.example.com
BARBICAN_KEYCLOAK_REALM=data-warehouse-prod
BARBICAN_KEYCLOAK_CLIENT_ID=barbican-service
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
- Configure an audience mapper on the Keycloak client so `aud` contains
  `BARBICAN_KEYCLOAK_CLIENT_ID`; otherwise every token fails audience validation

---

## Citadel Integration

Configuration for communication with the Citadel backend service.

| Variable | Default | Description |
|----------|---------|-------------|
| `BARBICAN_CITADEL_API_URL` | `http://citadel:8081` | Citadel API base URL |
| `BARBICAN_CITADEL_API_TIMEOUT_SECS` | `30` | HTTP request timeout |
| `BARBICAN_CITADEL_API_RETRY_MAX` | `3` | Maximum retry attempts |
| `BARBICAN_CITADEL_CLIENT_ID` | *(unset)* | Keycloak client ID for the client-credentials grant (production mode) |
| `BARBICAN_CITADEL_CLIENT_SECRET` | *(unset)* | Its client secret, read from this **environment variable** |
| `BARBICAN_CITADEL_SERVICE_TOKEN_FILE` | *(unset)* | **Development only**: path to a pre-issued JWT, re-read on every refresh |

Malformed numeric values are a startup error naming the variable.

### Citadel service token

Citadel's inbox routes are role-gated, so every Barbican → Citadel call needs
a Bearer token. It is supplied by a shared `common::ServiceTokenProvider`
(`common/src/auth/service_token.rs`) built from one of two sources:

| Mode | Variables | Behaviour |
|------|-----------|-----------|
| Client credentials (production) | `BARBICAN_CITADEL_CLIENT_ID` + `BARBICAN_CITADEL_CLIENT_SECRET` | `grant_type=client_credentials` POST to `{BARBICAN_KEYCLOAK_URL}/realms/{BARBICAN_KEYCLOAK_REALM}/protocol/openid-connect/token` |
| Static file (development) | `BARBICAN_CITADEL_SERVICE_TOKEN_FILE` | File is re-read on every refresh (an external refresher may rewrite it) |

- The token is cached and refreshed 30 s before it expires (the margin is
  capped at half the token lifetime). A static token whose `exp` cannot be
  read is re-read every 60 s.
- On a **401** from Citadel the client invalidates the cached token and
  retries once; that retry does not count against
  `BARBICAN_CITADEL_API_RETRY_MAX`.
- In Kubernetes inject the secret from a Secret via
  `env.valueFrom.secretKeyRef`. Rotating it requires a pod restart (the
  variable is read once at startup).
- Keycloak: the token must carry `citadel` in `aud` (add an audience mapper
  to the `barbican` client), and the `barbican` service account needs the
  `write` role (Citadel's inbox routes require one of
  `CITADEL_REQUIRED_ROLES`). See
  [Authentication → Service Accounts](authentication.md#service-accounts).

Startup errors (exit 1):

- client ID without secret, or secret without ID (empty values count as unset);
- both client credentials and `BARBICAN_CITADEL_SERVICE_TOKEN_FILE` set;
- `PRODUCTION=true` with static-file mode, or with nothing configured.

With nothing configured and `PRODUCTION` not `true`, Barbican logs a warning
and sends no `Authorization` header (Citadel rejects those calls).

```yaml
# Kubernetes container spec (excerpt)
env:
  - name: BARBICAN_CITADEL_CLIENT_ID
    value: barbican
  - name: BARBICAN_CITADEL_CLIENT_SECRET
    valueFrom:
      secretKeyRef:
        name: barbican-keycloak-client
        key: client-secret
```

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
BARBICAN_CITADEL_SERVICE_TOKEN_FILE=./dev/citadel.jwt   # dev-only static token
```

**Production:**
```bash
PRODUCTION=true
BARBICAN_CITADEL_API_URL=http://citadel.data-warehouse.svc.cluster.local:8081
BARBICAN_CITADEL_API_TIMEOUT_SECS=30
BARBICAN_CITADEL_API_RETRY_MAX=3
BARBICAN_CITADEL_CLIENT_ID=barbican
BARBICAN_CITADEL_CLIENT_SECRET=<from a Kubernetes Secret>
```

### Retry Behavior

Connection/timeout errors are retried with exponential backoff (an HTTP
error status is returned immediately; a 401 first gets one token refresh, see
above). A failure to obtain the service token fails the call without retry.

```
Attempt 1: Immediate
Attempt 2: After 1 second
Attempt 3: After 2 seconds
Attempt 4: After 4 seconds (if retry_max >= 4)
```

---

## Warehouse-path containment

Barbican is a metadata-only service and has no file-system configuration; the
physical inbox/protected directories belong to Citadel.

Barbican has **no** `validate_path` helper (there is no such function in the
codebase). Containment of the physical warehouse path is enforced by
**Citadel**: `CITADEL_PROTECTED_BASE_PATH` + `validate_warehouse_path`
reject any target outside the protected base. Barbican's duplicate-path guard
is the unique `warehouse_path` index (see [MongoDB](#mongodb)).

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

`BARBICAN_JWKS_REFRESH_SECS` has a hard minimum of `60`; lower values are a
startup error.
```bash
# Default / minimum
BARBICAN_JWKS_REFRESH_SECS=60

# Less frequent refresh
BARBICAN_JWKS_REFRESH_SECS=300
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
BARBICAN_REQUIRED_ROLES=read,write,admin
BARBICAN_ALLOWED_ALGORITHMS=RS256
BARBICAN_JWKS_REFRESH_SECS=60
BARBICAN_TOKEN_LEEWAY_MS=5000

# Citadel
BARBICAN_CITADEL_API_URL=http://localhost:8081
BARBICAN_CITADEL_API_TIMEOUT_SECS=60
BARBICAN_CITADEL_API_RETRY_MAX=5
BARBICAN_CITADEL_SERVICE_TOKEN_FILE=./dev/citadel.jwt   # dev-only

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

# Keycloak (PRODUCTION=true rejects the localhost default URL)
PRODUCTION=true
BARBICAN_KEYCLOAK_URL=https://auth.example.com
BARBICAN_KEYCLOAK_REALM=data-warehouse-prod
BARBICAN_KEYCLOAK_CLIENT_ID=barbican-service
BARBICAN_REQUIRED_ROLES=read,write
BARBICAN_ALLOWED_ALGORITHMS=RS256,RS384,RS512
BARBICAN_JWKS_REFRESH_SECS=300

# Citadel
BARBICAN_CITADEL_API_URL=http://citadel.data-warehouse.svc.cluster.local:8081
BARBICAN_CITADEL_API_TIMEOUT_SECS=30
BARBICAN_CITADEL_API_RETRY_MAX=3
BARBICAN_CITADEL_CLIENT_ID=barbican
BARBICAN_CITADEL_CLIENT_SECRET=...   # env.valueFrom.secretKeyRef, never a literal

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
      - BARBICAN_CITADEL_API_URL=http://citadel:8081
      - BARBICAN_CITADEL_CLIENT_ID=barbican
      - BARBICAN_CITADEL_CLIENT_SECRET=${BARBICAN_CITADEL_CLIENT_SECRET}
      - BARBICAN_PROMETHEUS_PORT=9091
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

Each loader validates its own settings: `Config::from_lookup`
(`barbican/src/config.rs`), `AuthConfig::validate_with`
(`common/src/auth/config.rs`, see [Startup validation](#startup-validation))
and `ServiceTokenSource::from_lookup` (`common/src/auth/service_token.rs`). A
failure is logged and the process exits with status 1 — it is a log line, not
an HTTP response:

```
ERROR Failed to load configuration: Invalid value for BARBICAN_PROMETHEUS_PORT: 70000
```

```
ERROR Invalid Citadel service-token configuration: Invalid configuration value: BARBICAN_CITADEL_CLIENT_ID is set but BARBICAN_CITADEL_CLIENT_SECRET is not
```

```
ERROR Failed to load auth configuration: Invalid configuration value: BARBICAN_ALLOWED_ALGORITHMS: symmetric algorithm 'HS256' not allowed
```

```
ERROR Failed to load auth configuration: Invalid configuration value: BARBICAN_JWKS_REFRESH_SECS must be a non-negative integer, got 'abc'
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