//! Tests for service-to-service tokens (`auth::service_token`).

#[cfg(test)]
mod config_tests {
    use crate::auth::config::AuthConfig;
    use crate::auth::service_token::{SecretString, ServiceTokenSource};
    use std::collections::HashMap;

    fn auth() -> AuthConfig {
        AuthConfig::from_lookup("CITADEL", |n| match n {
            "CITADEL_KEYCLOAK_URL" => Some("https://kc.example.com".into()),
            "CITADEL_KEYCLOAK_REALM" => Some("r".into()),
            _ => None,
        })
        .unwrap()
    }

    fn parse(vars: &[(&str, &str)]) -> Result<Option<ServiceTokenSource>, String> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        ServiceTokenSource::from_lookup("CITADEL", "BARBICAN", &auth(), |n| map.get(n).cloned())
            .map_err(|e| e.to_string())
    }

    #[test]
    fn test_client_credentials_from_env_vars() {
        let src = parse(&[
            ("CITADEL_BARBICAN_CLIENT_ID", "citadel"),
            ("CITADEL_BARBICAN_CLIENT_SECRET", "s3cret"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(
            src,
            ServiceTokenSource::ClientCredentials {
                token_url: "https://kc.example.com/realms/r/protocol/openid-connect/token".into(),
                client_id: "citadel".into(),
                client_secret: SecretString::new("s3cret"),
            }
        );
    }

    #[test]
    fn test_nothing_configured_is_none_outside_production() {
        assert_eq!(parse(&[]).unwrap(), None);
    }

    #[test]
    fn test_nothing_configured_is_error_in_production() {
        let err = parse(&[("PRODUCTION", "true")]).unwrap_err();
        assert!(err.contains("CITADEL_BARBICAN_CLIENT_ID"), "{err}");
    }

    #[test]
    fn test_id_without_secret_is_error() {
        let err = parse(&[("CITADEL_BARBICAN_CLIENT_ID", "citadel")]).unwrap_err();
        assert!(err.contains("CITADEL_BARBICAN_CLIENT_SECRET"), "{err}");
    }

    #[test]
    fn test_secret_without_id_is_error() {
        let err = parse(&[("CITADEL_BARBICAN_CLIENT_SECRET", "x")]).unwrap_err();
        assert!(err.contains("CITADEL_BARBICAN_CLIENT_ID"), "{err}");
    }

    #[test]
    fn test_both_modes_is_error() {
        let err = parse(&[
            ("CITADEL_BARBICAN_CLIENT_ID", "citadel"),
            ("CITADEL_BARBICAN_CLIENT_SECRET", "x"),
            ("CITADEL_BARBICAN_SERVICE_TOKEN_FILE", "/tmp/t"),
        ])
        .unwrap_err();
        assert!(err.contains("not both"), "{err}");
    }

    #[test]
    fn test_static_file_allowed_in_dev_rejected_in_production() {
        let file = [("CITADEL_BARBICAN_SERVICE_TOKEN_FILE", "/tmp/t")];
        assert!(matches!(
            parse(&file).unwrap(),
            Some(ServiceTokenSource::StaticFile(_))
        ));
        let mut prod = file.to_vec();
        prod.push(("PRODUCTION", "true"));
        assert!(parse(&prod).unwrap_err().contains("development only"));
    }

    #[test]
    fn test_secret_is_redacted_in_debug() {
        let src = parse(&[
            ("CITADEL_BARBICAN_CLIENT_ID", "citadel"),
            ("CITADEL_BARBICAN_CLIENT_SECRET", "s3cret-value"),
        ])
        .unwrap()
        .unwrap();
        let provider = crate::auth::ServiceTokenProvider::new(src);
        let shown = format!("{provider:?}");
        assert!(!shown.contains("s3cret-value"), "{shown}");
        assert!(shown.contains("[REDACTED]"), "{shown}");
    }
}

#[cfg(all(test, feature = "test-utils"))]
mod provider_tests {
    use crate::auth::service_token::{SecretString, ServiceTokenProvider, ServiceTokenSource};
    use crate::test_utils::{
        TEST_CLIENT_SECRET, TestKeycloak, sign_test_jwt, start_test_keycloak, test_claims,
    };
    use std::sync::atomic::Ordering;

    fn credentials(kc: &TestKeycloak, secret: &str) -> ServiceTokenSource {
        ServiceTokenSource::ClientCredentials {
            token_url: kc.auth_config("x").token_url(),
            client_id: "citadel".into(),
            client_secret: SecretString::new(secret),
        }
    }

    #[tokio::test]
    async fn test_token_is_cached() {
        let kc = start_test_keycloak("test").await;
        let p = ServiceTokenProvider::new(credentials(&kc, TEST_CLIENT_SECRET));
        let a = p.bearer().await.unwrap();
        let b = p.bearer().await.unwrap();
        assert_eq!(a, b);
        assert_eq!(kc.token_requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_invalidate_forces_refresh() {
        let kc = start_test_keycloak("test").await;
        let p = ServiceTokenProvider::new(credentials(&kc, TEST_CLIENT_SECRET));
        let a = p.bearer().await.unwrap();
        p.invalidate().await;
        let b = p.bearer().await.unwrap();
        assert_ne!(a, b);
        assert_eq!(kc.token_requests.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_short_lived_token_is_refreshed() {
        let kc = start_test_keycloak("test").await;
        // 2 s lifetime → refresh after 1 s (half-life cap on the margin).
        kc.token_ttl_secs.store(2, Ordering::SeqCst);
        let p = ServiceTokenProvider::new(credentials(&kc, TEST_CLIENT_SECRET));
        p.bearer().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        p.bearer().await.unwrap();
        assert_eq!(kc.token_requests.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_bad_secret_is_error() {
        let kc = start_test_keycloak("test").await;
        let p = ServiceTokenProvider::new(credentials(&kc, "wrong"));
        assert!(p.bearer().await.is_err());
    }

    #[tokio::test]
    async fn test_static_file_reread_after_invalidate() {
        let dir = tempdir();
        let path = dir.join("token");
        let t1 = sign_test_jwt(&test_claims("iss", "aud", "svc", &[], 300));
        std::fs::write(&path, &t1).unwrap();
        let p = ServiceTokenProvider::new(ServiceTokenSource::StaticFile(path.clone()));
        assert_eq!(p.bearer().await.unwrap(), t1);

        let t2 = sign_test_jwt(&test_claims("iss", "aud2", "svc", &[], 300));
        std::fs::write(&path, &t2).unwrap();
        // Still cached until expiry or invalidation.
        assert_eq!(p.bearer().await.unwrap(), t1);
        p.invalidate().await;
        assert_eq!(p.bearer().await.unwrap(), t2);
        std::fs::remove_dir_all(dir).ok();
    }

    fn tempdir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("svc-token-{}", uuid_like()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn uuid_like() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }
}
