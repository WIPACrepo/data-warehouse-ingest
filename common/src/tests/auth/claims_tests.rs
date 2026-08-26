//! Tests for Claims structure

#[cfg(test)]
mod tests {
    use crate::auth::claims::{Claims, ResourceAccess};
    use std::collections::HashMap;

    fn create_test_claims(service: &str, roles: Vec<&str>) -> Claims {
        let mut resource_access = HashMap::new();
        resource_access.insert(
            service.to_string(),
            ResourceAccess {
                roles: roles.into_iter().map(|s| s.to_string()).collect(),
            },
        );

        Claims {
            sub: "test-user".to_string(),
            iss: "http://localhost:8080/auth/realms/data-warehouse".to_string(),
            aud: "test-client".to_string(),
            exp: 9999999999,
            iat: 0,
            resource_access: Some(resource_access),
        }
    }

    #[test]
    fn test_get_roles_returns_correct_roles() {
        let claims = create_test_claims("barbican", vec!["read", "write"]);
        assert_eq!(claims.get_roles("barbican"), vec!["read".to_string(), "write".to_string()]);
    }

    #[test]
    fn test_get_roles_returns_empty_for_unknown_service() {
        let claims = create_test_claims("barbican", vec!["read"]);
        assert!(claims.get_roles("unknown").is_empty());
    }

    #[test]
    fn test_has_role_returns_true_for_existing_role() {
        let claims = create_test_claims("barbican", vec!["read", "write"]);
        assert!(claims.has_role("barbican", "read"));
        assert!(claims.has_role("barbican", "write"));
    }

    #[test]
    fn test_has_role_returns_false_for_nonexistent_role() {
        let claims = create_test_claims("barbican", vec!["read"]);
        assert!(!claims.has_role("barbican", "write"));
    }

    #[test]
    fn test_is_admin_returns_true_for_admin_role() {
        let claims = create_test_claims("barbican", vec!["admin"]);
        assert!(claims.is_admin("barbican"));
    }

    #[test]
    fn test_is_admin_returns_false_without_admin_role() {
        let claims = create_test_claims("barbican", vec!["read", "write"]);
        assert!(!claims.is_admin("barbican"));
    }

    #[test]
    fn test_can_write_with_admin_role() {
        let claims = create_test_claims("barbican", vec!["admin"]);
        assert!(claims.can_write("barbican"));
    }

    #[test]
    fn test_can_write_with_explicit_write_role() {
        let claims = create_test_claims("barbican", vec!["write"]);
        assert!(claims.can_write("barbican"));
    }

    #[test]
    fn test_can_write_without_write_permission() {
        let claims = create_test_claims("barbican", vec!["read"]);
        assert!(!claims.can_write("barbican"));
    }

    #[test]
    fn test_can_read_with_admin_role() {
        let claims = create_test_claims("barbican", vec!["admin"]);
        assert!(claims.can_read("barbican"));
    }

    #[test]
    fn test_can_read_with_explicit_read_role() {
        let claims = create_test_claims("barbican", vec!["read"]);
        assert!(claims.can_read("barbican"));
    }

    #[test]
    fn test_can_read_without_read_permission() {
        let claims = create_test_claims("barbican", vec!["write"]);
        assert!(!claims.can_read("barbican"));
    }

    #[test]
    fn test_is_internal_with_admin_role() {
        let claims = create_test_claims("barbican", vec!["admin"]);
        assert!(claims.is_internal("barbican"));
    }

    #[test]
    fn test_is_internal_with_explicit_internal_role() {
        let claims = create_test_claims("barbican", vec!["internal"]);
        assert!(claims.is_internal("barbican"));
    }

    #[test]
    fn test_is_internal_without_internal_permission() {
        let claims = create_test_claims("barbican", vec!["read"]);
        assert!(!claims.is_internal("barbican"));
    }

    #[test]
    fn test_service_specific_roles() {
        let mut resource_access = HashMap::new();
        resource_access.insert(
            "barbican".to_string(),
            ResourceAccess {
                roles: vec!["read".to_string()],
            },
        );
        resource_access.insert(
            "citadel".to_string(),
            ResourceAccess {
                roles: vec!["write".to_string()],
            },
        );

        let claims = Claims {
            sub: "test-user".to_string(),
            iss: "http://localhost:8080/auth/realms/data-warehouse".to_string(),
            aud: "test-client".to_string(),
            exp: 9999999999,
            iat: 0,
            resource_access: Some(resource_access),
        };

        assert!(claims.has_role("barbican", "read"));
        assert!(!claims.has_role("barbican", "write"));
        assert!(claims.has_role("citadel", "write"));
        assert!(!claims.has_role("citadel", "read"));
    }

    #[test]
    fn test_claims_with_no_resource_access() {
        let claims = Claims {
            sub: "test-user".to_string(),
            iss: "http://localhost:8080/auth/realms/data-warehouse".to_string(),
            aud: "test-client".to_string(),
            exp: 9999999999,
            iat: 0,
            resource_access: None,
        };

        assert!(claims.get_roles("barbican").is_empty());
        assert!(!claims.has_role("barbican", "read"));
        assert!(!claims.is_admin("barbican"));
        assert!(!claims.can_write("barbican"));
        assert!(!claims.can_read("barbican"));
        assert!(!claims.is_internal("barbican"));
    }
}