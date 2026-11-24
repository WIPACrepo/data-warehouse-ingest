//! Prometheus metrics for JWT authentication and authorization.
//!
//! Lives under the `auth` feature because it needs the optional `prometheus`
//! and `once_cell` crates. `crate::metrics` re-exports everything here when
//! `auth` is enabled, so existing `common::metrics::*` paths keep working.

use once_cell::sync::Lazy;
use prometheus::{Counter, CounterVec, Gauge, Histogram, HistogramOpts, Opts, Registry};

// Global registry for auth metrics
pub static AUTH_REGISTRY: Lazy<Registry> = Lazy::new(Registry::new);

// JWKS operations
pub static JWKS_FETCH_TOTAL: Lazy<Counter> =
    Lazy::new(|| Counter::new("auth_jwks_fetch_total", "Total JWKS fetch attempts").unwrap());
pub static JWKS_FETCH_FAILURES: Lazy<Counter> = Lazy::new(|| {
    Counter::new(
        "auth_jwks_fetch_failures_total",
        "Total JWKS fetch failures",
    )
    .unwrap()
});
pub static JWKS_CACHE_HITS: Lazy<Counter> =
    Lazy::new(|| Counter::new("auth_jwks_cache_hits_total", "Total JWKS cache hits").unwrap());
pub static JWKS_CACHE_STALENESS: Lazy<Gauge> = Lazy::new(|| {
    Gauge::new(
        "auth_jwks_cache_staleness_seconds",
        "Age of cached JWKS in seconds",
    )
    .unwrap()
});
pub static JWKS_REFRESH_RATE_LIMITED: Lazy<Counter> = Lazy::new(|| {
    Counter::new(
        "auth_jwks_refresh_rate_limited_total",
        "Times JWKS refresh was rate-limited",
    )
    .unwrap()
});

// Token validation
pub static TOKEN_VALIDATION_TOTAL: Lazy<CounterVec> = Lazy::new(|| {
    CounterVec::new(
        Opts::new("auth_token_validation_total", "Total token validations"),
        &["status"],
    )
    .unwrap()
    // status: "success", "expired", "invalid_signature", "invalid_issuer",
    //         "invalid_audience", "unknown_kid", "invalid_header", "algorithm_rejected"
});
pub static TOKEN_VALIDATION_DURATION: Lazy<Histogram> = Lazy::new(|| {
    // Buckets: 1ms, 5ms, 10ms, 25ms, 50ms, 100ms, 250ms, 500ms, 1s, 2.5s, 5s, 10s
    Histogram::with_opts(
        HistogramOpts::new(
            "auth_token_validation_duration_seconds",
            "Token validation latency",
        )
        .buckets(vec![
            0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
        ]),
    )
    .unwrap()
});

// Algorithm security
pub static ALGORITHM_REJECTED_TOTAL: Lazy<CounterVec> = Lazy::new(|| {
    CounterVec::new(
        Opts::new("auth_algorithm_rejected_total", "Rejected JWT algorithms"),
        &["algorithm", "reason"],
    )
    .unwrap()
    // reason: "symmetric_not_allowed", "none_not_allowed", "not_in_whitelist"
});

// Authorization
pub static AUTHZ_CHECK_TOTAL: Lazy<CounterVec> = Lazy::new(|| {
    CounterVec::new(
        Opts::new("auth_authz_check_total", "Total authorization checks"),
        &["result", "service"],
    )
    .unwrap()
    // result: "allowed", "denied"
});
pub static AUTHZ_DENIED_BY_ROLE: Lazy<CounterVec> = Lazy::new(|| {
    CounterVec::new(
        Opts::new(
            "auth_authz_denied_by_role_total",
            "Authorization denials by role",
        ),
        &["missing_role", "service"],
    )
    .unwrap()
});

// Configuration and infrastructure
pub static CONFIG_VALIDATION_FAILURES: Lazy<CounterVec> = Lazy::new(|| {
    CounterVec::new(
        Opts::new(
            "auth_config_validation_failures_total",
            "Configuration validation failures",
        ),
        &["config_key", "reason"],
    )
    .unwrap()
});

/// Initialize all auth metrics by registering them with the global registry
pub fn init_auth_metrics() -> Result<(), prometheus::Error> {
    AUTH_REGISTRY.register(Box::new(JWKS_FETCH_TOTAL.clone()))?;
    AUTH_REGISTRY.register(Box::new(JWKS_FETCH_FAILURES.clone()))?;
    AUTH_REGISTRY.register(Box::new(JWKS_CACHE_HITS.clone()))?;
    AUTH_REGISTRY.register(Box::new(JWKS_CACHE_STALENESS.clone()))?;
    AUTH_REGISTRY.register(Box::new(JWKS_REFRESH_RATE_LIMITED.clone()))?;
    AUTH_REGISTRY.register(Box::new(TOKEN_VALIDATION_TOTAL.clone()))?;
    AUTH_REGISTRY.register(Box::new(TOKEN_VALIDATION_DURATION.clone()))?;
    AUTH_REGISTRY.register(Box::new(ALGORITHM_REJECTED_TOTAL.clone()))?;
    AUTH_REGISTRY.register(Box::new(AUTHZ_CHECK_TOTAL.clone()))?;
    AUTH_REGISTRY.register(Box::new(AUTHZ_DENIED_BY_ROLE.clone()))?;
    AUTH_REGISTRY.register(Box::new(CONFIG_VALIDATION_FAILURES.clone()))?;
    Ok(())
}
