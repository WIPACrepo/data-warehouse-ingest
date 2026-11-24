# Authentication (JWT / Keycloak)

This document covers the `auth` feature of `common`: how Barbican and Citadel load their Keycloak/JWT settings, how the shared middleware state is built, and why it is designed this way (decision **D10**).

## Table of Contents

- [Overview](#overview)
- [Loading Configuration](#loading-configuration)
- [Environment Variables](#environment-variables)
- [Derived URLs: Issuer and JWKS](#derived-urls-issuer-and-jwks)
- [Validation Rules](#validation-rules)
- [Building the Middleware State](#building-the-middleware-state)
- [Role Checks](#role-checks)
- [Design Decision D10](#design-decision-d10)
- [Leeway Units (fixed)](#leeway-units-fixed)
- [See also](#see-also)

---

## Overview

All JWT handling lives in `common/src/auth/` behind the `auth` Cargo feature (which pulls in `jsonwebtoken`, `prometheus` and `once_cell`). Both services enable it via `common = { workspace = true, features = ["auth"] }`.

| Module | Contents |
|--------|----------|
| `config.rs` | `AuthConfig` (parse + validate), `build_auth_state` |
| `jwks_cache.rs` | `JwksCache` — fetches and caches the realm's signing keys |
| `jwt_validator.rs` | `JwtValidator` — signature, `exp`, `iss`, `aud` and algorithm checks |
| `claims.rs` | `Claims` — `resource_access` client-role helpers (`has_role`, `is_admin`) |
| `middleware.rs` | `AuthMiddlewareState`, `auth_middleware`, `ClaimsExtractor` |
| `error.rs` | `AuthError`, `ConfigError` |

All of these are re-exported from the crate root (e.g. `common::AuthConfig`, `common::build_auth_state`).

---

## Loading Configuration

```rust
// Production: read the process environment under a service-owned prefix.
let auth_config = common::AuthConfig::from_env_prefixed("BARBICAN")?; // or "CITADEL"

// Tests: same parser, arbitrary lookup — never mutates the process env.
let auth_config = common::AuthConfig::from_lookup("CITADEL", |name| map.get(name).cloned())?;
```

- `AuthConfig::from_env_prefixed(prefix)` is a thin wrapper: `from_lookup(prefix, |n| std::env::var(n).ok())`.
- `AuthConfig::from_lookup(prefix, lookup)` is the testable core. It reads every `{prefix}_*` variable through `lookup`, applies defaults, and runs validation before returning.
- The old unprefixed `AuthConfig::from_env()` (which read `KEYCLOAK_URL`, `KEYCLOAK_REALM`, ...) has been **removed**. Unprefixed variables, and variables carrying another service's prefix, are ignored.

| Service | Prefix | Call site |
|---------|--------|-----------|
| Barbican | `BARBICAN` | `barbican/src/main.rs` |
| Citadel | `CITADEL` | `citadel/src/main.rs` |

---

## Environment Variables

`{P}` is the service prefix (`BARBICAN` or `CITADEL`).

| Variable | Default | Notes |
|----------|---------|-------|
| `{P}_KEYCLOAK_URL` | `http://localhost:8080` | Keycloak **base URL, including any path prefix** — e.g. `https://kc.example.com/auth` for legacy Keycloak (< 17). A trailing `/` is trimmed. |
| `{P}_KEYCLOAK_REALM` | `data-warehouse` | Realm name. |
| `{P}_KEYCLOAK_CLIENT_ID` | lowercased prefix (`barbican` / `citadel`) | Expected `aud` claim. Keycloak only puts the client ID into `aud` when an **audience mapper** is configured for the client. |
| `{P}_REQUIRED_ROLES` | `read,write` | Comma-separated; any one role grants access (`admin` always passes). Must name at least one role. |
| `{P}_ALLOWED_ALGORITHMS` | `RS256` | Comma-separated allow-list. `HS*` and `none` are rejected. |
| `{P}_JWKS_REFRESH_SECS` | `60` | JWKS cache refresh interval; minimum `60`. |
| `{P}_TOKEN_LEEWAY_MS` | `5000` | Clock-skew leeway for `exp`; maximum `60000`. Converted to whole seconds, rounding up; see [Leeway Units](#leeway-units-fixed). |
| `PRODUCTION` (global, unprefixed) | unset | When `true`, the default localhost Keycloak URL is rejected. |

List values are split on `,`, trimmed, and empty entries dropped. Numeric values that fail to parse produce a `ConfigError::InvalidValue` naming the full variable, e.g. `CITADEL_JWKS_REFRESH_SECS must be a non-negative integer, got 'abc'`.

There is no client-secret variable: services only *validate* tokens, they do not request them from Keycloak, so `*_KEYCLOAK_CLIENT_SECRET` was removed.

---

## Derived URLs: Issuer and JWKS

```text
issuer   = {KEYCLOAK_URL}/realms/{realm}
jwks_url = {KEYCLOAK_URL}/realms/{realm}/protocol/openid-connect/certs
```

No `/auth/` segment is hard-coded. For Keycloak 17+ (Quarkus) set the bare host; for legacy deployments include the prefix in the URL:

| `{P}_KEYCLOAK_URL` | Issuer (realm `IceCube`) |
|--------------------|--------------------------|
| `https://kc.example.com` | `https://kc.example.com/realms/IceCube` |
| `https://kc.example.com/auth/` | `https://kc.example.com/auth/realms/IceCube` |

The issuer must match the token's `iss` claim exactly, so this value must be the URL Keycloak itself advertises.

---

## Validation Rules

`from_lookup` always validates; `validate()` / `validate_with(production, prefix)` can be called on a hand-built config. A config is rejected when:

1. `{P}_REQUIRED_ROLES` is empty (checked in `from_lookup` only — see [D10](#design-decision-d10));
2. `PRODUCTION=true` and the URL is the default `http://localhost:8080`;
3. the algorithm list is empty, or contains `none` or any `HS*` algorithm;
4. `jwks_refresh_secs < 60`;
5. `leeway_ms > 60000`.

Services treat a `ConfigError` as fatal at startup.

---

## Building the Middleware State

```rust
pub fn build_auth_state(config: &AuthConfig, service_name: &str) -> Arc<AuthMiddlewareState>;
```

`build_auth_state` replaces the per-service wiring each service used to do by hand: it creates the `JwksCache` (from `jwks_url()` and `jwks_refresh_secs`), wraps it in a `JwtValidator`, and returns an `AuthMiddlewareState` carrying `config.required_roles` and `service_name`. Pass the result to `axum::middleware::from_fn_with_state(state, common::auth_middleware)`.

```rust
let auth_config = AuthConfig::from_env_prefixed("CITADEL")?;
let auth_state = common::build_auth_state(&auth_config, "citadel");
```

---

## Role Checks

`auth_middleware` validates the Bearer token, then requires that the token's client roles for `service_name` (`resource_access.{service_name}.roles`) contain any of `required_roles`, or `admin`. Rejections are JSON `AppError` bodies: `401 auth_error`, `403 forbidden`, or `503 service_unavailable` when the JWKS cannot be fetched.

An **empty** `required_roles` list disables the role gate (any valid token passes). This mode cannot come from the environment — `from_lookup` rejects an empty `{P}_REQUIRED_ROLES` — so it only exists where a service builds an `AuthMiddlewareState` in code. Today that is Barbican's internal Citadel-callback routes (`*/{start,progress,complete,failure}`, decision D9), which reuse the public validator with `required_roles: Vec::new()`.

---

## Design Decision D10

**Decision:** one shared auth-config parser in `common`, reading service-owned, prefixed environment variables.

| Aspect | Choice | Rationale |
|--------|--------|-----------|
| Parser | Single `AuthConfig::from_lookup` in `common` | Previously both services validated tokens with `common`'s unprefixed `AuthConfig::from_env()`, while each service's own config *also* parsed `{SERVICE}_KEYCLOAK_*` fields that the validator never used — so the documented variables had no effect. One parser, called with the service prefix, removes the duplicate. |
| Namespace | `{SERVICE}_*` prefix chosen by the service | Barbican and Citadel can run in one environment (or one `.env`) with different realms, clients or roles without collisions; every other service setting is already prefixed. |
| Path prefix | Part of `{P}_KEYCLOAK_URL`, not hard-coded | The old `AuthConfig` always inserted `/auth/` before `/realms/`, which breaks Keycloak 17+ (no `/auth` context path). Making the prefix part of the base URL supports both generations. |
| Testability | `from_lookup` takes a closure | Tests no longer mutate process-global env vars, which raced under parallel `cargo test`. |
| Empty roles | Rejected from env; constructible only in code | Disabling the role gate is a deliberate routing decision (D9), not something a typo such as `BARBICAN_REQUIRED_ROLES=` should be able to switch on. |
| Default audience | Lowercased prefix | Matches the Keycloak client names (`barbican`, `citadel`) without extra configuration. |

**Consequences:** deployments must set prefixed variables (see the root [README upgrade notes](../../../README.md#upgrading)); unprefixed `KEYCLOAK_*` variables are silently ignored.

---

## Leeway Units (fixed)

`jsonwebtoken::Validation::leeway` is in **seconds**. `AuthConfig::leeway_secs()` converts `{P}_TOKEN_LEEWAY_MS` to whole seconds, rounding up (5000 ms → 5 s, 1500 ms → 2 s). Before this fix the millisecond value was passed through unconverted, so the 5000 ms default allowed 5000 s (≈ 83 min) of slack on `exp`/`nbf`.

---

## See also

- [README.md](README.md) - Main index
- [infrastructure.md](infrastructure.md) - `AppError` mapping for `auth_error` / `forbidden`
- [testing.md](testing.md) - `test-utils` auth mocks (RSA key generation, signed test tokens)
- [Barbican configuration](../../barbican/src/docs/configuration.md)
- [Citadel configuration](../../citadel/src/docs/configuration.md)
