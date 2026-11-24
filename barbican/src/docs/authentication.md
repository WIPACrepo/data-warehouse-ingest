# Authentication & Authorization

Comprehensive guide to JWT authentication, role-based access control, and security in Barbican.

## Table of Contents

- [Overview](#overview)
- [JWT Authentication](#jwt-authentication)
- [Token Validation](#token-validation)
- [JWKS Management](#jwks-management)
- [Role-Based Access Control](#role-based-access-control)
- [Service Accounts](#service-accounts)
- [Security Considerations](#security-considerations)
- [Implementation Details](#implementation-details)

---

## Overview

Barbican uses **JSON Web Tokens (JWT)** for authentication and **role-based access control (RBAC)** for authorization. All API endpoints (except health checks) require valid JWT authentication.

### Authentication Flow

```
┌─────────────┐      ┌─────────────┐      ┌─────────────┐
│   Client    │      │  Keycloak   │      │   Barbican  │
└──────┬──────┘      └──────┬──────┘      └──────┬──────┘
       │                    │                    │
       │  1. Authenticate   │                    │
       │───────────────────>│                    │
       │                    │                    │
       │  2. Return JWT     │                    │
       │<───────────────────│                    │
       │                    │                    │
       │  3. API Request    │                    │
       │  (with JWT)        │                    │
       │────────────────────────────────────────>│
       │                    │                    │
       │                    │  4. Validate JWT   │
       │                    │     & Check Roles  │
       │                    │                    │
       │  5. Response       │                    │
       │<────────────────────────────────────────│
       │                    │                    │
```

---

## JWT Authentication

### Token Structure

Barbican expects JWT tokens with the following structure:

**Header:**
```json
{
  "alg": "RS256",
  "typ": "JWT",
  "kid": "key-identifier"
}
```

**Payload (Claims):**
```json
{
  "sub": "user-123",
  "iss": "http://keycloak:8080/realms/data-warehouse",
  "aud": "barbican",
  "exp": 1640000000,
  "iat": 1639996400,
  "resource_access": {
    "barbican": {
      "roles": ["read", "write"]
    }
  }
}
```

**Signature:**
- Signed with Keycloak's private RSA key
- Verified using JWKS public key

### Token Requirements

| Claim | Requirement | Description |
|-------|-------------|-------------|
| `alg` | Must be `RS256` | Algorithm whitelist enforced |
| `iss` | Must equal `{BARBICAN_KEYCLOAK_URL}/realms/{BARBICAN_KEYCLOAK_REALM}` | Issuer validation |
| `aud` | Must contain `BARBICAN_KEYCLOAK_CLIENT_ID` (default `barbican`; needs a Keycloak audience mapper) | Audience validation |
| `exp` | Must be in future (leeway `BARBICAN_TOKEN_LEEWAY_MS`, default 5000 ms) | Expiration check |
| `resource_access.barbican.roles` | Must contain required role | Authorization check |

---

## Token Validation

### Validation Steps

Barbican performs the following validation checks in order:

1. **Token Format**: Verify JWT structure (header.payload.signature)
2. **Algorithm Check**: Ensure `alg` is in allowed list (default: `RS256`)
3. **Signature Verification**: Validate signature against JWKS public key
4. **Expiration Check**: Verify token not expired (leeway `BARBICAN_TOKEN_LEEWAY_MS`, default 5 s, max 60 s)
5. **Issuer Validation**: Check `iss` claim matches Keycloak realm
6. **Audience Validation**: Check `aud` claim matches client ID
7. **Role Verification**: Ensure token has required role for endpoint

### Algorithm Security

**Critical Security Measures:**

- ✅ **Whitelist Only**: Only explicitly allowed algorithms accepted
- ✅ **Reject `none`**: Algorithm `none` is explicitly rejected
- ✅ **No HMAC Fallback**: Never fall back to HMAC with public key
- ✅ **Key ID Validation**: `kid` must match a key in JWKS

Configuration:
```bash
BARBICAN_ALLOWED_ALGORITHMS=RS256
```

### Error Responses

Auth rejections return **JSON bodies** via `common::auth_middleware` (decision
D7) — never bare status codes.

**401 Unauthorized** - Missing/invalid token:
```json
{
  "error": "auth_error",
  "message": "Missing or invalid Authorization header"
}
```

**401 Unauthorized** - JWT validation failure (redacted reason):
```json
{
  "error": "auth_error",
  "message": "Invalid token"
}
```

**403 Forbidden** - Insufficient role:
```json
{
  "error": "forbidden",
  "message": "Missing required role for service 'barbican'"
}
```

**503 Service Unavailable** - JWKS unavailable:
```json
{
  "error": "service_unavailable",
  "message": "JWT verification temporarily unavailable"
}
```

---

## JWKS Management

### JWKS Fetching

Barbican fetches the JSON Web Key Set (JWKS) from Keycloak:

```
GET {BARBICAN_KEYCLOAK_URL}/realms/{BARBICAN_KEYCLOAK_REALM}/protocol/openid-connect/certs
```

`BARBICAN_KEYCLOAK_URL` includes any path prefix; no `/auth/` segment is
added automatically (legacy Keycloak < 17: set it to `https://host/auth`).

Response:
```json
{
  "keys": [
    {
      "kid": "key-1",
      "kty": "RSA",
      "alg": "RS256",
      "use": "sig",
      "x5c": ["MIICqT..."]
    }
  ]
}
```

### Caching Strategy

To reduce load on Keycloak and improve performance:

- **Cache Duration**: JWKS cached for `BARBICAN_JWKS_REFRESH_SECS` (default and minimum 60 seconds)
- **Lazy Refresh**: Fetch new JWKS only when cache expires
- **Kid Rotation**: Immediately refresh if unknown `kid` encountered
- **Rate Limiting**: Max 1 JWKS refresh per minute

Configuration:
```bash
BARBICAN_JWKS_REFRESH_SECS=60
BARBICAN_KEYCLOAK_URL=http://keycloak:8080
BARBICAN_KEYCLOAK_REALM=data-warehouse
```

### JWKS Refresh Logic

```rust
async fn maybe_refresh_jwks(&self) -> Result<(), JwtError> {
    let last_refresh = self.last_refresh.read().await;
    
    // Check if cache is still valid
    if last_refresh.elapsed() < refresh_interval {
        return Ok(());
    }
    drop(last_refresh);
    
    // Acquire write lock and fetch new JWKS
    let mut jwks_guard = self.jwks.write().await;
    
    // Double-check after acquiring lock
    // ... fetch JWKS from Keycloak ...
}
```

---

## Role-Based Access Control

### Available Roles

| Role | Description |
|------|-------------|
| `admin` | Full access to all endpoints |
| `write` | Create, publish, delete inboxes |
| `read` | Query inbox metadata only |
| (no `internal`) | There is **no internal role** (D9) — see below |

### Endpoint Authorization

#### External Endpoints

| Endpoint | Required Roles |
|----------|----------------|
| `POST /api/v1/inbox` | `write`, `admin` |
| `GET /api/v1/inbox` | `read`, `write`, `admin` |
| `GET /api/v1/inbox/{id}` | `read`, `write`, `admin` |
| `DELETE /api/v1/inbox/{id}` | `write`, `admin` |
| `POST /api/v1/inbox/{id}/publish` | `write`, `admin` |

#### Internal Endpoints (State Transitions)

| Endpoint Pattern | Required Roles |
|------------------|----------------|
| `/api/v1/inbox/{id}/*/start` | (none — valid Bearer JWT only) |
| `/api/v1/inbox/{id}/*/progress` | (none — valid Bearer JWT only) |
| `/api/v1/inbox/{id}/*/complete` | (none — valid Bearer JWT only) |
| `/api/v1/inbox/{id}/*/failure` | (none — valid Bearer JWT only) |

#### Public Endpoints

| Endpoint | Required Roles |
|----------|----------------|
| `GET /health` | None (public) |
| `GET /ready` | None (public) |

### Role Hierarchy

```
admin ⊃ {write, read}
write ⊃ {read}
```

- `admin` can access all endpoints
- `write` can access read endpoints and write operations
- `read` can only query inbox metadata

> **Trust model (D9)**: the state-transition endpoints are reachable by any
> valid Bearer token — there is no role gate. In a well-deployed network they
> are **not publicly addressable**: Citadel is the only legitimate caller and
> authenticates to Barbican. If agent-grade mutual-auth or role gating is
> ever needed, that is a separate, dedicated task — do not assume it exists
> here.

### Configuration

```bash
# Comma-separated list of accepted roles (external routes only).
# Must name at least one role; an empty value is a startup error.
BARBICAN_REQUIRED_ROLES=read,write
```

`admin` always passes the role check (hard-coded in
`common::auth_middleware`; there is no `BARBICAN_ADMIN_ROLE` variable).
`BARBICAN_INTERNAL_ROLES` does **not** exist.

---

## Service Accounts

### Citadel Service Account

Citadel authenticates to Barbican using a service account (any valid Bearer
token — no role requirement, D9):

**Setup:**
1. Create service account in Keycloak
2. Generate JWT token (or use client credentials grant)
3. Store token in file referenced by `BARBICAN_CITADEL_SERVICE_TOKEN_FILE`

**Token File:**
```bash
BARBICAN_CITADEL_SERVICE_TOKEN_FILE=/etc/barbican/citadel-token.jwt
```

**Token Refresh:**
- Citadel is responsible for refreshing its token
- Barbican validates token on each request
- Expired tokens return 401 Unauthorized

### External Client Service Accounts

For automated clients:

1. Create service account in Keycloak
2. Assign appropriate roles (`read`, `write`)
3. Use client credentials grant to obtain JWT
4. Include JWT in `Authorization: Bearer` header

### Client Credentials Grant

```http
POST /realms/data-warehouse/protocol/openid-connect/token
Host: keycloak:8080
Content-Type: application/x-www-form-urlencoded

grant_type=client_credentials
&client_id=my-service
&client_secret=service-secret
```

Response:
```json
{
  "access_token": "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9...",
  "expires_in": 300,
  "token_type": "Bearer"
}
```

---

## Security Considerations

### Token Security

- **Short-Lived Tokens**: Recommended expiration: 5-15 minutes
- **Secure Storage**: Store tokens in memory, not logs or disk
- **HTTPS Only**: Always transmit tokens over TLS
- **Token Binding**: Bind tokens to client IP when possible

### Key Management

- **Key Rotation**: Keycloak should rotate signing keys periodically
- **JWKS Cache**: Refresh cache on key rotation (detected via new `kid`)
- **Algorithm Whitelist**: Only allow strong algorithms (RS256, RS384, RS512)
- **Reject Weak Algorithms**: Explicitly reject `none`, `HS256` with public key

### Input Validation

- **Claim Validation**: Validate all required claims present
- **Type Checking**: Ensure claims are correct types (string, array, etc.)
- **Sanitization**: Sanitize string inputs to prevent injection
- **Size Limits**: Limit token size to prevent DoS

### Rate Limiting

- **JWKS Refresh**: Max 1 refresh/minute to prevent abuse
- Edge rate limiting per IP / per service is **not implemented** (see
  [api-reference.md](api-reference.md#rate-limiting)); enforce at an edge
  proxy if needed.

### Audit Logging

All authentication events should be logged:

```json
{
  "timestamp": "2024-01-01T00:00:00Z",
  "event": "token_validated",
  "client_id": "my-service",
  "subject": "user-123",
  "roles": ["read", "write"],
  "endpoint": "POST /api/v1/inbox",
  "source_ip": "10.0.0.5",
  "result": "success"
}
```

---

## Implementation Details

Barbican has no auth code of its own: everything lives in `common::auth`
(see [`common/src/docs/auth.md`](../../../common/src/docs/auth.md)).

### Startup wiring (`barbican/src/main.rs`, `barbican/src/lib.rs`)

```rust
// main.rs — fail fast, before connecting to MongoDB
let auth_config = AuthConfig::from_env_prefixed("BARBICAN")?; // BARBICAN_KEYCLOAK_*, ...
// lib.rs::serve
let auth_state = common::build_auth_state(&auth_config, "barbican");
let state = Arc::new(AppState::new(store, config, auth_state));
```

`create_app` derives a second `AuthMiddlewareState` for the internal
`*/{start,progress,complete,failure}` routes with the same validator and an
empty `required_roles` (Bearer-only, D9).

### Claims (`common/src/auth/claims.rs`)

Role helpers take the service name, which selects
`resource_access.<service>.roles`:

| Method | Meaning |
|--------|---------|
| `get_roles(service)` | roles listed for `service` |
| `has_role(service, role)` | `role` is listed |
| `is_admin(service)` | has `admin` |
| `can_write(service)` | `admin` or `write` |
| `can_read(service)` | `admin` or `read` |
| `is_internal(service)` | `admin` or `internal` — exists but unused (no internal role, D9) |

### JWT validation (`common/src/auth/jwt_validator.rs`)

1. Decode the header; reject algorithms not in `BARBICAN_ALLOWED_ALGORITHMS`.
2. Look up the `kid` in the cached JWKS (`JwksCache`, refreshed every
   `BARBICAN_JWKS_REFRESH_SECS`).
3. Validate signature, `exp`, `iss` (`AuthConfig::issuer()`) and `aud`
   (`BARBICAN_KEYCLOAK_CLIENT_ID`). Leeway is `AuthConfig::leeway_secs()` —
   `BARBICAN_TOKEN_LEEWAY_MS` converted to whole seconds (rounded up), because
   `jsonwebtoken` expects seconds.

### Auth middleware (`common::auth_middleware`)

`auth_middleware(State<Arc<AuthMiddlewareState>>, Request, Next) -> Result<Response, AppError>`:
missing/invalid Bearer → 401 `auth_error`; JWKS unavailable → 503
`service_unavailable`; none of `required_roles` (and not `admin`) → 403
`forbidden`; otherwise `Claims` are inserted into request extensions.

---

## Configuration Reference

### Environment Variables

Auth settings are parsed by the shared
`common::AuthConfig::from_env_prefixed("BARBICAN")`
(`common/src/auth/config.rs`, decision **D10**) — not by Barbican's `Config`.
`barbican/src/main.rs` loads them before connecting to MongoDB and exits with
status 1 on any error; the resulting `AuthConfig` is turned into middleware
state with `common::build_auth_state(&auth_config, "barbican")`.

| Variable | Default | Description |
|----------|---------|-------------|
| `BARBICAN_KEYCLOAK_URL` | `http://localhost:8080` | Keycloak base URL including any path prefix (e.g. `.../auth` for Keycloak < 17); trailing `/` trimmed |
| `BARBICAN_KEYCLOAK_REALM` | `data-warehouse` | Keycloak realm name |
| `BARBICAN_KEYCLOAK_CLIENT_ID` | `barbican` | Expected `aud` (requires an audience mapper on the Keycloak client) |
| `BARBICAN_REQUIRED_ROLES` | `read,write` | Accepted roles (comma-separated, external routes); must be non-empty |
| `BARBICAN_ALLOWED_ALGORITHMS` | `RS256` | Allowed JWT algorithms; `HS*` and `none` rejected |
| `BARBICAN_JWKS_REFRESH_SECS` | `60` | JWKS cache refresh interval (minimum 60) |
| `BARBICAN_TOKEN_LEEWAY_MS` | `5000` | Clock-skew leeway for `exp`/`nbf` (maximum 60000) |
| `PRODUCTION` *(global)* | unset | `true` rejects the localhost default `BARBICAN_KEYCLOAK_URL` |
| `BARBICAN_CITADEL_SERVICE_TOKEN_FILE` | *(optional)* | Path to the Citadel service token used for Barbican → Citadel triggers (read by `Config`, not `AuthConfig`) |

Malformed numbers are a startup error naming the variable. Derived values:
issuer `{URL}/realms/{realm}`, JWKS `{URL}/realms/{realm}/protocol/openid-connect/certs`.

> **Removed:** `BARBICAN_KEYCLOAK_CLIENT_SECRET` (required at startup but never
> used) and the unprefixed `KEYCLOAK_URL`, `KEYCLOAK_REALM`,
> `KEYCLOAK_CLIENT_ID`, `REQUIRED_ROLES`, `ALLOWED_ALGORITHMS`,
> `JWKS_REFRESH_SECS`, `TOKEN_LEEWAY_MS` (previously what the auth layer
> actually read). See [Configuration](configuration.md#keycloak-authentication).

> `BARBICAN_INTERNAL_ROLES` does not exist (D9); the state-transition
> endpoints require only a valid Bearer JWT. See
> [Role-Based Access Control](#role-based-access-control).

### Example Configuration

```bash
# Keycloak
export BARBICAN_KEYCLOAK_URL="https://auth.example.com"
export BARBICAN_KEYCLOAK_REALM="data-warehouse"
export BARBICAN_KEYCLOAK_CLIENT_ID="barbican-service"
export PRODUCTION="true"

# JWT Validation
export BARBICAN_ALLOWED_ALGORITHMS="RS256,RS384"
export BARBICAN_JWKS_REFRESH_SECS="60"
export BARBICAN_TOKEN_LEEWAY_MS="5000"

# Roles
export BARBICAN_REQUIRED_ROLES="read,write"

# Citadel Integration
export BARBICAN_CITADEL_SERVICE_TOKEN_FILE="/etc/barbican/citadel.jwt"
```

---

*See also:*
- [Architecture](architecture.md) - System design overview
- [API Reference](api-reference.md) - Endpoint documentation
- [Configuration](configuration.md) - Environment variables