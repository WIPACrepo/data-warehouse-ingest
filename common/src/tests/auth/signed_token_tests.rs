//! End-to-end JWT validation against the mock Keycloak (real RS256 keys).

#[cfg(all(test, feature = "test-utils"))]
mod tests {
    use crate::auth::build_auth_state;
    use crate::test_utils::start_test_keycloak;

    #[tokio::test]
    async fn test_signed_token_validates_and_carries_roles() {
        let kc = start_test_keycloak("test").await;
        let state = build_auth_state(&kc.auth_config("citadel"), "citadel");
        let token = kc.token("citadel", "citadel", &["admin"], 300);

        let claims = state.validator.validate(&token).await.expect("valid token");
        assert!(claims.is_admin("citadel"));
    }

    #[tokio::test]
    async fn test_expired_token_rejected() {
        let kc = start_test_keycloak("test").await;
        let state = build_auth_state(&kc.auth_config("citadel"), "citadel");
        let token = kc.token("citadel", "citadel", &["admin"], -600);
        assert!(state.validator.validate(&token).await.is_err());
    }

    #[tokio::test]
    async fn test_wrong_audience_rejected() {
        let kc = start_test_keycloak("test").await;
        let state = build_auth_state(&kc.auth_config("citadel"), "citadel");
        let token = kc.token("barbican", "citadel", &["admin"], 300);
        assert!(state.validator.validate(&token).await.is_err());
    }
}
