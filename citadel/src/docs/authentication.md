# Authentication & Authorization

Comprehensive guide to JWT authentication, role-based access control, job-route (callback) security, and the outbound Barbican service token in Citadel.

## Table of Contents

- [Overview](#overview)
- [JWT Authentication](#jwt-authentication)
- [Token Validation](#token-validation)
- [JWKS Management](#jwks-management)
- [Role-Based Access Control](#role-based-access-control)
- [Job Route Security](#job-route-security)
- [Outbound: Citadel → Barbican Service Token](#outbound-citadel--barbican-service-token)
- [Security Considerations](#security-considerations)
- [Implementation Details](#implementation-details)

---

## Overview

Citadel uses **JSON Web Tokens (JWT)** for authentication and **role-based access control (RBAC)** for authorization. Routes fall into three groups, wired in `citadel::create_app` ([`lib.rs`](../lib.rs)):

| Group | Routes | Credential |
|-------|--------|------------|
| Public | `/health`, `/ready` | none |
| User | `/api/v1/inbox/{id}/{create,publish,delete}`, `/api/v1/job/{job_id}`, `/api/v1/jobs`, `/api/v1/job/{job_id}/cancel` | Bearer JWT with one of `CITADEL_REQUIRED_ROLES` (`common::auth_middleware`) |
| Job | `/api/v1/job/callback`, `/api/v1/job/progress` | Bearer JWT with Citadel `admin`, **or** no `Authorization` header + `X-Callback-Secret` + `timestamp` + `nonce` ([`middleware/job_auth.rs`](../middleware/job_auth.rs), D11) |

Outbound, Citadel authenticates to Barbican with a **service token** (client-credentials grant) — see [Outbound: Citadel → Barbican](#outbound-citadel--barbican-service-token).

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
  "iss": "http://keycloak:8080/realms/data-warehouse",
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

### Claims Structure (from [`auth/jwt_validator.rs`](../../../common/src/auth/jwt_validator.rs))

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

Roles are read from `resource_access.citadel.roles` via `Claims::get_roles("citadel")`.

### Token Requirements

| Claim | Requirement | Description |
|-------|-------------|-------------|
| `alg` | In `CITADEL_ALLOWED_ALGORITHMS`; `none` rejected | Algorithm whitelist |
| `iss` | Equals `{CITADEL_KEYCLOAK_URL}/realms/{CITADEL_KEYCLOAK_REALM}` | Issuer validation |
| `aud` | Matches `CITADEL_KEYCLOAK_CLIENT_ID` (requires a Keycloak audience mapper on the client) | Audience validation |
| `exp` | Must be in the future (`CITADEL_TOKEN_LEEWAY_MS` leeway applied) | Expiration check |
| `resource_access.citadel.roles` | Contains a role from `CITADEL_REQUIRED_ROLES`, or `admin` | Authorization gate |

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
GET {CITADEL_KEYCLOAK_URL}/realms/{CITADEL_KEYCLOAK_REALM}/protocol/openid-connect/certs
```

Derived by `AuthConfig::jwks_url()` in [`common/src/auth/config.rs`](../../../common/src/auth/config.rs) (D10). `/auth` is **not** hard-coded: for legacy Keycloak (< 17) include it in the base URL, e.g. `CITADEL_KEYCLOAK_URL=https://host/auth`.

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
| (no `internal`) | Barbican coordination does **not** use an internal role — see [Job Route Security](#job-route-security) |

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
| `POST /api/v1/job/callback` | Bearer JWT with `admin`, **or** (no `Authorization` header) `X-Callback-Secret` + `timestamp` + `nonce` — see [Job Route Security](#job-route-security) |
| `POST /api/v1/job/progress` | same as the callback (P4.1) |
| `GET /health` | None (public) |
| `GET /ready` | None (public) |

### Role Helpers (`Claims`)

Defined in [`common/src/auth/claims.rs`](../../../common/src/auth/claims.rs); every helper takes the service name (`"citadel"`) whose `resource_access` entry to inspect:

```rust
impl Claims {
    pub fn get_roles(&self, service: &str) -> Vec<String>;       // resource_access.{service}.roles
    pub fn has_role(&self, service: &str, role: &str) -> bool;
    pub fn is_admin(&self, service: &str) -> bool;               // has_role(service, "admin")
    pub fn can_write(&self, service: &str) -> bool;              // admin || write
    pub fn can_read(&self, service: &str) -> bool;               // admin || read  (write alone does NOT imply read)
    pub fn is_internal(&self, service: &str) -> bool;            // admin || internal — exists, unused (D9)
}
```

`common::auth_middleware` (re-exported by `middleware/auth.rs`) validates the JWT, requires at least one role from `CITADEL_REQUIRED_ROLES` (or `admin`), and inserts a `Claims` into request extensions for handlers. (The common `Claims::is_internal` utility method still exists but is **not used** by either service — Barbican coordination is not role-gated, see [Barbican trust model](../../../barbican/src/docs/authentication.md).)

### Configuration

```bash
# Comma-separated list of accepted roles (middleware gate); must be non-empty
CITADEL_REQUIRED_ROLES=read,write
```

---

## Job Route Security

`POST /api/v1/job/callback` and `POST /api/v1/job/progress` are **not** behind the user JWT layer (D11). Before D11 they were, so every `citadel-job` callback (which carries no JWT) was rejected with `401`. They now accept exactly one of two credentials, decided by the `job_caller_auth` middleware ([`middleware/job_auth.rs`](../middleware/job_auth.rs)), which inserts a `JobCaller` into request extensions; the handlers then call `authenticate_job_caller` ([`handlers/job_callback.rs`](../handlers/job_callback.rs)).

| Request | Outcome |
|---------|---------|
| `Authorization` header not `Bearer <token>` | `401 auth_error` |
| Bearer JWT invalid / expired | `401 auth_error` (JWKS unavailable → `503 service_unavailable`) |
| Valid JWT **without** Citadel `admin` (`resource_access.citadel.roles`) | `403 forbidden` |
| Valid JWT **with** `admin` | `JobCaller::Admin` — secret, `timestamp`, `nonce` **not** required; logged with the JWT `sub` |
| No `Authorization` header | `JobCaller::Job` — `timestamp` + `nonce` required (missing → `401 auth_error`), then the secret/timestamp/nonce flow below |

- **No fall-through**: a present `Authorization` header is authoritative. An invalid or non-admin token is rejected even if a valid `X-Callback-Secret` is also sent; the two credentials never combine.
- **Admin replay protection** is the JWT signature + `exp`; no nonce is stored. Use the admin path for manual operator actions (e.g. finishing a stuck job). For error entries the handler uses the request `timestamp` if supplied, else the current time.
- `timestamp` and `nonce` are optional JSON fields (`Option`) in `JobCallbackRequest` / `JobProgressRequest`; only the secret path enforces them.

### Worker (secret) path — flow

From `validate_callback_credentials` in [`handlers/job_callback.rs`](../handlers/job_callback.rs):

1. **Callback secret validation** (when a secret is configured): the `X-Callback-Secret` header is compared in constant time; missing/mismatch → `401 invalid_callback_secret` (see [Callback Secret Validation](#callback-secret-validation-implemented)). Note: when no secret is configured, the worker path relies on timestamp + nonce only
2. **Timestamp validation**: `now - timestamp` must be `< 300s` and `> -60s` (5-minute window, 1-minute clock skew). `citadel-job` stamps a fresh timestamp and UUID nonce on **every** send attempt (`build_callback_request` / `build_progress_request` in `job_main.rs`), so the window bounds network delay only, not job duration
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

## Outbound: Citadel → Barbican Service Token

Barbican's internal routes (`/api/v1/inbox/{id}/{create,publish,delete}/{start,progress,complete,failure}`) require a valid Bearer JWT whose `aud` includes `barbican`; no role is required (D9). Before D11 Citadel sent **no** `Authorization` header, so every Citadel → Barbican call was rejected with `401`.

`main.rs` builds a `common::ServiceTokenSource::from_env("CITADEL", "BARBICAN", &auth_config)` and attaches a `common::ServiceTokenProvider` via `BarbicanClient::with_token_provider` ([`integrations/barbican.rs`](../integrations/barbican.rs)):

| Mode | Variables | Notes |
|------|-----------|-------|
| Client credentials (production) | `CITADEL_BARBICAN_CLIENT_ID`, `CITADEL_BARBICAN_CLIENT_SECRET` | Keycloak `client_credentials` grant at `{CITADEL_KEYCLOAK_URL}/realms/{CITADEL_KEYCLOAK_REALM}/protocol/openid-connect/token`. The secret is read from the **environment variable** (in Kubernetes inject it from a Secret via `env.valueFrom.secretKeyRef`); rotating it requires a pod restart |
| Static token file (development only) | `CITADEL_BARBICAN_SERVICE_TOKEN_FILE` | Pre-issued token, re-read on every refresh |
| None | — | Outside production: startup warning, requests carry no `Authorization` header (a JWT-protected Barbican rejects them) |

- **Caching**: the token is cached and refreshed before expiry (30 s margin, capped at half the token lifetime); concurrent callers share one fetch.
- **Startup errors**: ID without secret (or vice versa); client credentials and token file both set; with `PRODUCTION=true`, the static-file mode or nothing configured.
- **401 handling**: on a `401` from Barbican the client invalidates the cached token and retries **once** with a fresh one. This inner retry does not count toward `post_action` retries or the circuit breaker. A persistent `401` becomes `502 bad_gateway` naming `CITADEL_BARBICAN_CLIENT_ID/SECRET` and the audience mapper. Failure to obtain a token → `503 service_unavailable`.
- **Keycloak setup**: the `citadel` client needs service accounts enabled and an **audience mapper** adding `barbican` to its access tokens' `aud`.

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

### Job Route & Callback Hardening (implemented / remaining)

- Job routes: a present `Authorization` header is authoritative (no fall-through to the secret path); only Citadel `admin` JWTs are accepted (D11)

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

### Auth Middleware (`common/src/auth/middleware.rs`)

`citadel/src/middleware/auth.rs` re-exports `common::auth_middleware`; `create_app` applies it to the **user** route group only (the job routes use `middleware::job_auth::job_caller_auth`). Its state is the `Arc<AuthMiddlewareState>` built at startup:

```rust
// main.rs (D10)
let auth_config = AuthConfig::from_env_prefixed("CITADEL")?;
let auth_state = build_auth_state(&auth_config, "citadel"); // JwksCache -> JwtValidator -> state
```

Per request the middleware:

1. Extracts `Bearer <token>` from `Authorization` (missing → `401 auth_error`)
2. Validates the JWT with `JwtValidator` (JWKS unavailable → `503 service_unavailable`; other failures → `401 auth_error`)
3. Requires `has_role("citadel", r) || is_admin("citadel")` for some `r` in `required_roles` (else `403 forbidden`)
4. Inserts the `Claims` into request extensions

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
| `CITADEL_KEYCLOAK_URL` | `http://localhost:8080` | Keycloak base URL incl. any path prefix (`/auth` for Keycloak < 17); trailing `/` trimmed; default rejected when `PRODUCTION=true` |
| `CITADEL_KEYCLOAK_REALM` | `data-warehouse` | Keycloak realm name |
| `CITADEL_KEYCLOAK_CLIENT_ID` | `citadel` | Expected `aud` (needs a Keycloak audience mapper) |
| `CITADEL_REQUIRED_ROLES` | `read,write` | Accepted roles (comma-separated, non-empty) |
| `CITADEL_ALLOWED_ALGORITHMS` | `RS256` | Allowed JWT algorithms; `HS*`/`none` rejected at startup |
| `CITADEL_JWKS_REFRESH_SECS` | `60` | JWKS cache refresh interval (minimum 60) |
| `CITADEL_TOKEN_LEEWAY_MS` | `5000` | `exp`/`nbf` clock-skew leeway (maximum 60000) |
| `CITADEL_JOB_CALLBACK_SECRET_FILE` | `/etc/citadel/secrets/callback-secret` | Path to shared callback secret |
| `CITADEL_BARBICAN_CLIENT_ID` | (none) | Client ID for the Citadel → Barbican service token |
| `CITADEL_BARBICAN_CLIENT_SECRET` | (none) | Client secret (env var; from a Kubernetes Secret) |
| `CITADEL_BARBICAN_SERVICE_TOKEN_FILE` | (none) | Dev-only static token file (rejected with `PRODUCTION=true`) |

Auth variables are parsed by `common::AuthConfig::from_env_prefixed("CITADEL")` (D10); malformed numbers abort startup naming the variable. `CITADEL_KEYCLOAK_CLIENT_SECRET` and the unprefixed `KEYCLOAK_*` names are no longer read. See [Configuration](configuration.md#keycloak-authentication).

### Example Configuration

```bash
# Keycloak / JWT
export CITADEL_KEYCLOAK_URL="http://localhost:8080"
export CITADEL_KEYCLOAK_REALM="data-warehouse"
export CITADEL_KEYCLOAK_CLIENT_ID="citadel"

# JWT validation
export CITADEL_ALLOWED_ALGORITHMS="RS256"
export CITADEL_REQUIRED_ROLES="read,write"
export CITADEL_JWKS_REFRESH_SECS="60"
export CITADEL_TOKEN_LEEWAY_MS="5000"

# Callback secret
export CITADEL_JOB_CALLBACK_SECRET_FILE="/etc/citadel/secrets/callback-secret"
```

---

*See also:*
- [API Reference](api-reference.md) - Endpoint role requirements
- [Architecture](architecture.md) - Security architecture
- [Configuration](configuration.md) - All environment variables
