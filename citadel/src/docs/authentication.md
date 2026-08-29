# Authentication & Authorization

Comprehensive guide to JWT authentication, role-based access control, and callback security in Citadel.

## Table of Contents

- [Overview](#overview)
- [JWT Authentication](#jwt-authentication)
- [Token Validation](#token-validation)
- [JWKS Management](#jwks-management)
- [Role-Based Access Control](#role-based-access-control)
- [Callback Security](#callback-security)
- [Security Considerations](#security-considerations)
- [Implementation Details](#implementation-details)

---

## Overview

Citadel uses **JSON Web Tokens (JWT)** for authentication and **role-based access control (RBAC)** for authorization. All `/api/v1/` endpoints (except the job callback, which uses its own scheme) require a valid JWT. `/health` and `/ready` are public.

### Authentication Flow

```
┌─────────────┐      ┌─────────────┐      ┌─────────────┐
│   Client    │      │  Keycloak   │      │   Citadel   │
└──────┬──────┘      └──────┬──────┘      └──────┬──────┘
       │                    │                    │
       │  1. Authenticate   │                    │
       │───────────────────>│                    │
       │                    │                    │
       │  2. Return JWT     │                    │
       │<───────────────────│                    │
       │                    │                    │
       │  3. API Request    │                    │
       │  (Bearer JWT)      │                    │
       │────────────────────────────────────────>│
       │                    │                    │
       │                    │  4. Validate JWT   │
       │                    │     (JWKS)         │
       │                    │                    │
       │  5. Response       │                    │
       │<────────────────────────────────────────│
       │                    │                    │
```

In practice, end users talk to Barbican; Citadel is typically called by Barbican (with a service account) and other internal components.

---

## JWT Authentication

### Token Structure

Citadel expects JWT tokens with the following structure:

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
  "sub": "citadel-service",
  "iss": "http://keycloak:8080/auth/realms/data-warehouse",
  "aud": "citadel",
  "exp": 1640000000,
  "iat": 1639996400,
  "resource_access": {
    "citadel": {
      "roles": ["read", "write"]
    }
  }
}
```

**Signature:**
- Signed with Keycloak's private key
- Verified using the JWKS public key

### Claims Structure (from [`auth/jwt_validator.rs`](../src/auth/jwt_validator.rs))

```rust
pub struct Claims {
    pub sub: String,
    pub iss: String,
    pub aud: String,
    pub exp: usize,
    pub iat: usize,
    pub resource_access: Option<HashMap<String, ResourceAccess>>,
}

pub struct ResourceAccess {
    pub roles: Vec<String>,
}
```

Roles are read from `resource_access.citadel.roles` via `Claims::get_roles()`.

### Token Requirements

| Claim | Requirement | Description |
|-------|-------------|-------------|
| `alg` | In `CITADEL_ALLOWED_ALGORITHMS`; `none` rejected | Algorithm whitelist |
| `iss` | Matches Keycloak realm issuer | Issuer validation |
| `aud` | Matches `CITADEL_KEYCLOAK_CLIENT_ID` | Audience validation |
| `exp` | Must be in the future (leeway applied) | Expiration check |
| `resource_access.citadel.roles` | Contains a role from `CITADEL_REQUIRED_ROLES` | Authorization gate |

---

## Token Validation

### Validation Steps (`validate_jwt`)

Citadel performs the following checks in order:

1. **Header decode**: `jsonwebtoken::decode_header` fails on malformed tokens
2. **Algorithm check**: `format!("{:?}", header.alg)` must not be `"none"` or `"None"`, and must be in `CITADEL_ALLOWED_ALGORITHMS`
3. **Key lookup**: the `kid` must match a key in the JWKS
4. **Key construction**: RSA (`n`, `e`), EC (`x`, `y`, `crv = P-256`), or EdDSA (`x`) keys are supported
5. **Validation object**: `validate_exp = true`, leeway applied, issuer and audience set
6. **Decode**: signature verified; on failure returns `InvalidJwt`

### Algorithm Security

**Critical Security Measures (in code):**

- ✅ **Whitelist Only**: `alg` must be in `CITADEL_ALLOWED_ALGORITHMS` (default `RS256`)
- ✅ **Reject `none`**: `alg: none` / `alg: None` rejected before decoding
- ✅ **No HMAC Fallback**: only asymmetric key types are used (`RSA`, `EC`, `OKP`); the public key is never reused as an HMAC secret
- ✅ **Key ID Validation**: `kid` must be present and found in JWKS

Configuration:
```bash
CITADEL_ALLOWED_ALGORITHMS=RS256
```

### Error Responses

Auth rejections return **JSON bodies** via `common::auth_middleware` (decision D7) — never bare status codes:

**401 Unauthorized** - Missing/invalid token:
```json
{
  "error": "auth_error",
  "message": "Missing or invalid Authorization header"
}
```
JWT validation failures map to the redacted reason:
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
  "message": "Missing required role for service 'citadel'"
}
```

**503 Service Unavailable** - JWKS unavailable (could not fetch/parse signing keys):
```json
{
  "error": "service_unavailable",
  "message": "JWT verification temporarily unavailable"
}
```

(The message strings are examples of the shape; the serialized `{error, message}` body is produced by `common::AppError::IntoResponse` — see [`common/src/docs/infrastructure.md`](../../../common/src/docs/infrastructure.md).)

---

## JWKS Management

### JWKS Fetching

Citadel fetches the JSON Web Key Set from Keycloak:

```
GET {CITADEL_KEYCLOAK_URL}/auth/realms/{realm}/protocol/openid-connect/certs
```

Derived in `config.rs`:
```rust
pub fn jwks_url(&self) -> String {
    format!(
        "{}/auth/realms/{}/protocol/openid-connect/certs",
        self.keycloak_url, self.keycloak_realm
    )
}
```

### Caching Strategy (`JwksCache`)

To reduce load on Keycloak:

- **Cache Duration**: JWKS cached for `CITADEL_JWKS_REFRESH_SECS` (default 60s)
- **Rate Limit**: at most one refresh per minute regardless
- **Behavior on rate-limit hit**: returns the stale cached JWKS with a warning

```bash
CITADEL_JWKS_REFRESH_SECS=60
```

> **Planned**: immediate refresh when a new/unknown `kid` is seen (rate-limited to 1/min). The current code refreshes on cache expiry only.

---

## Role-Based Access Control

### Available Roles

| Role | Description |
|------|-------------|
| `admin` | Full access, including job cancellation |
| `write` | Create, publish, delete inboxes (inbox ops) |
| `read` | Enumerate and read jobs only |
| (no `internal`) | Barbican coordination does **not** use an internal role — see [Callback Security](#callback-security-and-the-citadel-job-trust-edge) |

### Endpoint Authorization

#### Inbox Endpoints

| Endpoint | Required Roles |
|----------|----------------|
| `POST /api/v1/inbox/{id}/create` | `write`, `admin` |
| `POST /api/v1/inbox/{id}/publish` | `write`, `admin` |
| `POST /api/v1/inbox/{id}/delete` | `write`, `admin` |

#### Job Endpoints

| Endpoint | Required Roles |
|----------|----------------|
| `GET /api/v1/job/{job_id}` | `read`, `write`, `admin` |
| `GET /api/v1/jobs` | `read`, `write`, `admin` |
| `POST /api/v1/job/{job_id}/cancel` | `admin` only |

#### Callback & Health

| Endpoint | Required Roles |
|----------|----------------|
| `POST /api/v1/job/callback` | service JWT (route is inside the auth layer) **+** timestamp + nonce + `X-Callback-Secret` |
| `POST /api/v1/job/progress` | service JWT **+** timestamp + nonce + `X-Callback-Secret` (P4.1) |
| `GET /health` | None (public) |
| `GET /ready` | None (public) |

### Role Helpers (`Claims`)

```rust
impl Claims {
    pub fn get_roles(&self) -> Vec<String>;         // resource_access.citadel.roles
    pub fn has_role(&self, role: &str) -> bool;
    pub fn is_admin(&self) -> bool;                 // has_role("admin")
    pub fn can_write(&self) -> bool;                // admin || write
    pub fn can_read(&self) -> bool;                 // admin || read || write
}
```

`req.middleware` (`auth_middleware`) validates the JWT, requires at least one role from `CITADEL_REQUIRED_ROLES`, and inserts a `Claims` into request extensions for handlers. (The common `Claims::is_internal` utility method still exists but is **not used** by either service — Barbican coordination is not role-gated, see [Barbican trust model](../../../barbican/src/docs/authentication.md).)

### Configuration

```bash
# Comma-separated list of accepted roles (middleware gate)
CITADEL_REQUIRED_ROLES=read,write
```

---

## Callback Security

The job callback endpoint (`POST /api/v1/job/callback`) authenticates job workers by **callback secret** (when configured), plus **timestamp + nonce** rather than a JWT.

### Flow (from [`handlers/job_callback.rs`](../src/handlers/job_callback.rs))

1. **Callback secret validation** (when a secret is configured): the `X-Callback-Secret` header is compared in constant time; missing/mismatch → `401 invalid_callback_secret` (see [Callback Secret Validation](#callback-secret-validation-implemented))
2. **Timestamp validation**: `now - timestamp` must be `< 300s` and `> -60s` (5-minute window, 1-minute clock skew)
3. **Nonce validation & storage**: look up `{ nonce }` in the `callback_nonces`
   collection:
   - A nonce that is still **live** (expires at a future time) → reject
     `CallbackNonceReused`
   - A nonce that is **present but expired** → reused with a fresh 5-minute
     window (the value is replaced); the **TTL index** on
     `callback_nonces.expires_at` (created idempotently at startup) garbage-
     collects old rows — the replay-protection window stays semantically 5
     minutes (P2.2)
   - Otherwise store `CallbackNonceDB::new(nonce)` (5-minute expiry) and continue

This prevents **replay attacks**: a captured callback can only be replayed for a short window and only once per nonce.

### Callback Request

```json
{
  "job_id": "58a3e4c0-...",
  "status": "completed",
  "timestamp": "2024-01-01T00:00:00Z",
  "nonce": "unique-random-string",
  "result": {
    "success": true,
    "files_transferred": 10,
    "total_bytes": 1048576
  }
}
```

### Nonce Storage

```rust
pub struct CallbackNonceDB {
    pub nonce: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>, // 5 minutes from created_at
}
```

### Callback Secret Validation (implemented)

When a secret is configured (read from `CITADEL_JOB_CALLBACK_SECRET_FILE`),
the handler verifies the `X-Callback-Secret` request header using a
**constant-time byte comparison** (`ct_eq`, in `handlers/job_callback.rs`)
to prevent timing side-channels:

- Secret configured + header present and matching → accepted
- Secret configured + header missing or mismatching → rejected with
  `401 invalid_callback_secret` (and `callback_errors_total` incremented)
- No secret configured → dev-mode passthrough (a warning is logged unless
  the mock executor is in use); authentication then relies on
  timestamp + nonce only

The secret is **hot-reloaded**: `secret_reloader.rs` watches the file via
the `notify` crate and atomically swaps the in-memory value
(`Arc<RwLock<Option<String>>>`) on change — no service restart needed.
`citadel-job` reads the same secret file and attaches it as
`X-Callback-Secret` on every callback.

> **Remaining planned hardening**:
> - K8s NetworkPolicy restricting the callback endpoint to internal pods
>
> The job callback/progress **trust edge** (D9): `citadel-job` talks only to
> Citadel (`/job/callback`, `/job/progress`), never to Barbican. Coordination
> with Barbican is Citadel's outward-only responsibility; no `internal` role or
> cross-service direct trust is used.

---

## Security Considerations

### Token Security

- **Short-Lived Tokens**: recommended expiration 5-15 minutes
- **Secure Storage**: tokens in memory, not logs
- **HTTPS Only**: always transmit tokens over TLS in production

### Key Management

- **Key Rotation**: Keycloak should rotate signing keys periodically
- **JWKS Cache**: refresh cache on rotation
- **Algorithm Whitelist**: only asymmetric algorithms; reject `none` and HMAC-with-public-key

### Callback Security Hardening (implemented / remaining)

- Shared file-based secret (`CITADEL_JOB_CALLBACK_SECRET_FILE`) with
  constant-time comparison and hot-reload (implemented)
- Replay protection via per-nonce MongoDB deduplication + 5-minute
  timestamp window, with a **TTL index** on `callback_nonces.expires_at`
  creating idempotently at startup (implemented)
- K8s NetworkPolicy restricting the callback endpoint to internal pods
  (deployment-level, not yet wired)

### Input Validation

- JWT header algorithm checked before decode
- `kid` required and validated against JWKS
- Error responses sanitized (generic messages, full detail server-side)

### Rate Limiting

- **JWKS refresh**: at most 1/minute (enforced in code)
- Other rate limiting is not currently implemented in the API layer

### Audit Logging

Security-relevant events are logged with `tracing` (`info`/`warn`/`error`), including JWT validation failures, auth missing headers, and callback validation results. Structured JSON logging is recommended for production.

---

## Implementation Details

### Auth Middleware (`middleware/auth.rs`)

```rust
pub async fn auth_middleware(
    State(state): State<AppState>,
    mut req: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    // 1. Extract "Bearer <token>" from Authorization header
    let token = extract_token(&req).ok_or(StatusCode::UNAUTHORIZED)?;

    // 2. Validate JWT and get Claims
    let claims = crate::auth::validate_jwt(&token, &state.jwks_cache, &state.config)
        .await
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    // 3. Require at least one role from CITADEL_REQUIRED_ROLES
    let has_required_role = state.config.required_roles.iter()
        .any(|role| claims.has_role(role));
    if !has_required_role {
        return Err(StatusCode::FORBIDDEN);
    }

    // 4. Insert AuthState for handlers
    req.extensions_mut().insert(AuthState::new(claims));
    Ok(next.run(req).await)
}
```

### JWKS Cache Core (`auth/jwt_validator.rs`)

```rust
impl JwksCache {
    // Returns cached JWKS if fresh; otherwise rate-limits (1/min)
    // then fetches from Keycloak and stores (custom JWKS deserialization).
    pub async fn get_jwks(&self) -> Result<CustomJwks> { ... }

    // Finds the kid, builds DecodingKey (RSA/EC/EdDSA), returns (key, alg).
    pub async fn get_decoding_key(&self, kid: &str) -> Result<(DecodingKey, Algorithm)> { ... }
}
```

---

## Configuration Reference

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `CITADEL_KEYCLOAK_URL` | `http://localhost:8080` | Keycloak server URL |
| `CITADEL_KEYCLOAK_REALM` | `data-warehouse` | Keycloak realm name |
| `CITADEL_KEYCLOAK_CLIENT_ID` | `citadel` | Client ID for audience validation |
| `CITADEL_KEYCLOAK_CLIENT_SECRET` | *(optional in code)* | Client secret |
| `CITADEL_REQUIRED_ROLES` | `read,write` | Accepted roles (comma-separated) |
| `CITADEL_ALLOWED_ALGORITHMS` | `RS256` | Allowed JWT algorithms |
| `CITADEL_JWKS_REFRESH_SECS` | `60` | JWKS cache refresh interval |
| `CITADEL_JOB_CALLBACK_SECRET_FILE` | `/etc/citadel/secrets/callback-secret` | Path to shared callback secret |

### Example Configuration

```bash
# Keycloak / JWT
export CITADEL_KEYCLOAK_URL="http://localhost:8080"
export CITADEL_KEYCLOAK_REALM="data-warehouse"
export CITADEL_KEYCLOAK_CLIENT_ID="citadel"
export CITADEL_KEYCLOAK_CLIENT_SECRET="super-secret"

# JWT validation
export CITADEL_ALLOWED_ALGORITHMS="RS256"
export CITADEL_REQUIRED_ROLES="read,write"
export CITADEL_JWKS_REFRESH_SECS="60"

# Callback secret
export CITADEL_JOB_CALLBACK_SECRET_FILE="/etc/citadel/secrets/callback-secret"
```

---

*See also:*
- [API Reference](api-reference.md) - Endpoint role requirements
- [Architecture](architecture.md) - Security architecture
- [Configuration](configuration.md) - All environment variables
