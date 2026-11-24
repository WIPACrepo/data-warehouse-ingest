//! Tests for `Config::from_lookup` (no process-environment access).

use std::collections::HashMap;

use crate::config::{Config, ConfigError};

fn load(vars: &[(&str, &str)]) -> Result<Config, ConfigError> {
    let map: HashMap<String, String> = vars
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    Config::from_lookup(|n| map.get(n).cloned())
}

#[test]
fn test_defaults() {
    let c = load(&[]).unwrap();
    assert_eq!(c.bind_address, "127.0.0.1:8080");
    assert_eq!(c.citadel_api_timeout_secs, 30);
    assert_eq!(c.citadel_api_retry_max, 3);
    assert_eq!(c.prometheus_port, 9091);
}

#[test]
fn test_overrides() {
    let c = load(&[
        ("BARBICAN_BIND_ADDRESS", "0.0.0.0:9000"),
        ("BARBICAN_CITADEL_API_URL", "http://c:1"),
        ("BARBICAN_CITADEL_API_TIMEOUT_SECS", "5"),
        ("BARBICAN_CITADEL_API_RETRY_MAX", "7"),
        ("BARBICAN_PROMETHEUS_PORT", "9999"),
    ])
    .unwrap();
    assert_eq!(c.bind_address, "0.0.0.0:9000");
    assert_eq!(c.citadel_api_url, "http://c:1");
    assert_eq!(c.citadel_api_timeout_secs, 5);
    assert_eq!(c.citadel_api_retry_max, 7);
    assert_eq!(c.prometheus_port, 9999);
}

#[test]
fn test_malformed_numbers_name_the_variable() {
    for (var, bad) in [
        ("BARBICAN_CITADEL_API_TIMEOUT_SECS", "30s"),
        ("BARBICAN_CITADEL_API_RETRY_MAX", "-1"),
        ("BARBICAN_PROMETHEUS_PORT", "70000"),
    ] {
        let err = load(&[(var, bad)]).unwrap_err();
        assert!(err.to_string().contains(var), "{var}: {err}");
    }
}

#[test]
fn test_default_ignores_environment() {
    // Default must not read env: identical to the literal defaults.
    let d = Config::default();
    assert_eq!(d.bind_address, "127.0.0.1:8080");
    assert_eq!(d.mongodb_database, "data_warehouse");
}
