//! JWT Claims structure (generic, service-agnostic)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// JWT claims structure (generic, works for any service)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub iss: String,
    pub aud: String,
    pub exp: usize,
    pub iat: usize,
    pub resource_access: Option<HashMap<String, ResourceAccess>>,
}

/// Resource access information from JWT claims
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceAccess {
    pub roles: Vec<String>,
}

impl Claims {
    /// Extract roles for a specific service (e.g., "barbican" or "citadel")
    pub fn get_roles(&self, service: &str) -> Vec<String> {
        self.resource_access
            .as_ref()
            .and_then(|ra| ra.get(service))
            .map(|ra| ra.roles.clone())
            .unwrap_or_default()
    }

    /// Check if claims contain a specific role for a service
    /// Iterates the resource-access entries without cloning (no allocation).
    pub fn has_role(&self, service: &str, role: &str) -> bool {
        self.resource_access
            .as_ref()
            .and_then(|ra| ra.get(service))
            .map(|ra| ra.roles.iter().any(|r| r == role))
            .unwrap_or(false)
    }

    /// Check if claims have admin role for a service
    /// ADMIN GRANTS ALL PERMISSIONS (read, write, internal)
    pub fn is_admin(&self, service: &str) -> bool {
        self.has_role(service, "admin")
    }

    /// Check if claims have explicit write permission for a service
    pub fn can_write(&self, service: &str) -> bool {
        self.is_admin(service) || self.has_role(service, "write")
    }

    /// Check if claims have explicit read permission for a service
    pub fn can_read(&self, service: &str) -> bool {
        self.is_admin(service) || self.has_role(service, "read")
    }

    /// Check if claims have internal permission for a service
    pub fn is_internal(&self, service: &str) -> bool {
        self.is_admin(service) || self.has_role(service, "internal")
    }
}