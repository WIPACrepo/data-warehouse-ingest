use serde::{Deserialize, Serialize};

/// JWT claims structure for Keycloak tokens
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub iss: String,
    pub aud: String,
    pub exp: usize,
    pub iat: usize,
    #[serde(default)]
    pub resource_access: Option<std::collections::HashMap<String, ResourceAccess>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceAccess {
    #[serde(default)]
    pub roles: Vec<String>,
}

impl Claims {
    /// Extract roles from the resource_access claim for the barbican client
    pub fn get_roles(&self) -> Vec<String> {
        self.resource_access
            .as_ref()
            .and_then(|ra| ra.get("barbican"))
            .map(|ra| ra.roles.clone())
            .unwrap_or_default()
    }

    /// Check if the claims contain a specific role
    pub fn has_role(&self, role: &str) -> bool {
        self.get_roles().contains(&role.to_string())
    }

    /// Check if the claims have admin role
    pub fn is_admin(&self) -> bool {
        self.has_role("admin")
    }

    /// Check if the claims have write permission (admin or write role)
    pub fn can_write(&self) -> bool {
        self.is_admin() || self.has_role("write")
    }

    /// Check if the claims have internal permission (admin or internal role)
    pub fn is_internal(&self) -> bool {
        self.is_admin() || self.has_role("internal")
    }

    /// Check if the claims have read permission (admin, write, internal, or read role)
    pub fn can_read(&self) -> bool {
        self.is_admin() || self.has_role("write") || self.has_role("internal") || self.has_role("read")
    }
}