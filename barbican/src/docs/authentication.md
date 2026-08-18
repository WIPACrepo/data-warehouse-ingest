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
| `iss` | Must match Keycloak realm URL | Issuer validation |
| `aud` | Must match `barbican` | Audience validation |
| `exp` | Must be in future (5s leeway) | Expiration check |
| `resource_access.barbican.roles` | Must contain required role | Authorization check |

---

## Token Validation

### Validation Steps

Barbican performs the following validation checks in order:

1. **Token Format**: Verify JWT structure (header.payload.signature)
2. **Algorithm Check**: Ensure `alg` is in allowed list (default: `RS256`)
3. **Signature Verification**: Validate signature against JWKS public key
4. **Expiration Check**: Verify token not expired (5-second leeway)
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

**401 Unauthorized** - Invalid or missing token:
```json
{
  "error": "INVALID_TOKEN",
  "message": "JWT token signature verification failed"
}
```

**401 Unauthorized** - Token expired:
```json
{
  "error": "TOKEN_EXPIRED",
  "message": "JWT token has expired"
}
```

**403 Forbidden** - Insufficient role:
```json
{
  "error": "INSUFFICIENT_ROLE",
  "message": "Required role 'write' not found in token"
}
```

---

## JWKS Management

### JWKS Fetching

Barbican fetches the JSON Web Key Set (JWKS) from Keycloak:

```
GET /realms/{realm}/protocol/openid-connect/certs
Host: keycloak:8080
```

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

- **Cache Duration**: JWKS cached for 60 seconds (configurable)
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
| `write` | Create, update, and delete inboxes |
| `read` | Query inbox metadata only |
| `internal` | Access to Citadel callback endpoints |

### Endpoint Authorization

#### External Endpoints

| Endpoint | Required Roles |
|----------|----------------|
| `POST /api/v1/inbox` | `write`, `admin` |
| `GET /api/v1/inbox` | `read`, `write`, `internal`, `admin` |
| `GET /api/v1/inbox/{id}` | `read`, `write`, `internal`, `admin` |
| `DELETE /api/v1/inbox/{id}` | `write`, `admin` |
| `POST /api/v1/inbox/{id}/publish` | `write`, `admin` |

#### Internal Endpoints (Citadel Callbacks)

| Endpoint Pattern | Required Roles |
|------------------|----------------|
| `/api/v1/inbox/{id}/*/start` | `internal`, `admin` |
| `/api/v1/inbox/{id}/*/progress` | `internal`, `admin` |
| `/api/v1/inbox/{id}/*/complete` | `internal`, `admin` |
| `/api/v1/inbox/{id}/*/failure` | `internal`, `admin` |

#### Public Endpoints

| Endpoint | Required Roles |
|----------|----------------|
| `GET /health` | None (public) |
| `GET /ready` | None (public) |

### Role Hierarchy

```
admin ⊃ {write, internal, read}
write ⊃ {read}
internal ⊃ {read}
```

- `admin` can access all endpoints
- `write` can access read endpoints and write operations
- `internal` can access read endpoints and Citadel callbacks
- `read` can only query inbox metadata

### Configuration

```bash
# Comma-separated list of accepted roles
BARBICAN_REQUIRED_ROLES=read,write

# Roles allowed to access internal endpoints
BARBICAN_INTERNAL_ROLES=internal

# Admin role (has all permissions)
BARBICAN_ADMIN_ROLE=admin
```

---

## Service Accounts

### Citadel Service Account

Citadel authenticates to Barbican using a service account:

**Setup:**
1. Create service account in Keycloak
2. Assign `internal` role to service account
3. Generate JWT token (or use client credentials grant)
4. Store token in file referenced by `BARBICAN_CITADEL_SERVICE_TOKEN_FILE`

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

- **Per-IP Limits**: 100 requests/minute for external endpoints
- **Per-Service Limits**: 1000 requests/minute for internal endpoints
- **JWKS Refresh**: Max 1 refresh/minute to prevent abuse
- **Failed Auth**: Exponential backoff after repeated failures

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

### Claims Structure

```rust
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Claims {
    pub sub: String,
    pub iss: String,
    pub aud: String,
    pub exp: usize,
    pub iat: usize,
    pub resource_access: Option<HashMap<String, ResourceAccess>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ResourceAccess {
    pub roles: Vec<String>,
}

impl Claims {
    pub fn get_roles(&self) -> Vec<String> {
        self.resource_access
            .as_ref()
            .and_then(|ra| ra.get("barbican"))
            .map(|ra| ra.roles.clone())
            .unwrap_or_default()
    }
    
    pub fn has_role(&self, role: &str) -> bool {
        self.get_roles().contains(&role.to_string())
    }
    
    pub fn is_admin(&self) -> bool {
        self.has_role("admin")
    }
    
    pub fn can_write(&self) -> bool {
        self.is_admin() || self.has_role("write")
    }

}
```

### JWT Validation

```rust
use jsonwebtoken::{decode, jwk::JwkSet, Algorithm, DecodingKey, Validation};

pub async fn validate(&self, token: &str) -> Result<Claims, JwtError> {
    // Fetch JWKS (cached)
    let jwks = self.get_jwks().await?;
    
    // Decode header to get kid
    let header = jsonwebtoken::decode_header(token)?;
    
    // Find matching JWK
    let jwk = jwks.keys.iter()
        .find(|k| k.common.key_id == header.kid)
        .ok_or(JwtError::UnknownKeyId)?;
    
    // Extract public key from x509 chain
    let public_key = extract_public_key(jwk)?;
    
    // Set up validation
    let mut validation = Validation::new(Algorithm::RS256);
    validation.leeway = 5000; // 5 seconds
    validation.iss = Some(self.config.keycloak_issuer.clone());
    validation.aud = Some(self.config.keycloak_audience.clone());
    
    // Decode and validate token
    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_rsa_pem(public_key.as_bytes())?,
        &validation,
    )?;
    
    Ok(token_data.claims)
}
```

### Auth Middleware

```rust
use axum::{
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::Response,
};

pub async fn validate_auth<B>(
    State(state): State<Arc<AppState>>,
    mut request: Request<B>,
    next: Next<B>,
) -> Result<Response, StatusCode> {
    // Extract token from Authorization header
    let token = extract_token(&request)?;
    
    // Validate token
    let claims = state.jwt_validator.validate.validate(token)
        .\n        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    
    // Check required role
    let required_role = get_required_role(&request);
    if !claims.has_role(required_role) && !claims.is_admin() {
        return Err(StatusCode::FORBIDDEN);
    }
    
    // Insert claims into request extensions
    request.extensions_mut().insert(claims);
    
    Ok(next.run(request).await)
}
```

---

## Configuration Reference

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `BARBICAN_KEYCLOAK_URL` | `http://localhost:8080` | Keycloak server URL |
| `BARBICAN_KEYCLOAK_REALM` | `data-warehouse` | Keycloak realm name |
| `BARBICAN_KEYCLOAK_CLIENT_ID` | `barbican` | Client ID for audience validation |
| `BARBICAN_KEYCLOAK_CLIENT_SECRET` | *(required)* | Client secret for service account |
| `BARBICAN_REQUIRED_ROLES` | `read,write` | Accepted roles (comma-separated) |
| `BARBICAN_INTERNAL_ROLES` | `internal` | Roles for internal endpoints |
| `BARBICAN_ALLOWED_ALGORITHMS` | `RS256` | Allowed JWT algorithms |
| `BARBICAN_JWKS_REFRESH_SECS` | `60` | JWKS cache refresh interval |
| `BARBICAN_CITADEL_SERVICE_TOKEN_FILE` | *(optional)* | Path to Citadel service token |

### Example Configuration

```bash
# Keycloak
export BARBICAN_KEYCLOAK_URL="https://auth.example.com"
export BARBICAN_KEYCLOAK_REALM="data-warehouse"
export BARBICAN_KEYCLOAK_CLIENT_ID="barbican-service"
export BARBICAN_KEYCLOAK_CLIENT_SECRET="super-secret-value"

# JWT Validation
export BARBICAN_ALLOWED_ALGORITHMS="RS256,RS384"
export BARBICAN_JWKS_REFRESH_SECS="60"

# Roles
export BARBICAN_REQUIRED_ROLES="read,write"
export BARBICAN_INTERNAL_ROLES="internal,admin"

# Citadel Integration
export BARBICAN_CITADEL_SERVICE_TOKEN_FILE="/etc/barbican/citadel.jwt"
```

---

*See also:*
- [Architecture](architecture.md) - System design overview
- [API Reference](api-reference.md) - Endpoint documentation
- [Configuration](configuration.md) - Environment variables