use crate::models::{CreateInbox, InboxDB, InboxStatus};
use chrono::Utc;
use mongodb::bson::oid::ObjectId;

/// Builder pattern for creating test InboxDB instances
#[derive(Debug, Clone)]
pub struct InboxBuilder {
    id: Option<ObjectId>,
    name: String,
    size: u64,
    status: InboxStatus,
    warehouse_path: String,
    lifetime: Option<u64>,
    archive_enabled: bool,
    archive_sites: Vec<String>,
}

impl InboxBuilder {
    /// Creates a new InboxBuilder with default values
    pub fn new() -> Self {
        Self {
            id: Some(ObjectId::new()),
            name: "Test Inbox".to_string(),
            size: 100,
            status: InboxStatus::Active,
            warehouse_path: "/test/warehouse/path".to_string(),
            lifetime: None,
            archive_enabled: false,
            archive_sites: vec![],
        }
    }

    /// Sets the inbox ID
    pub fn with_id(mut self, id: ObjectId) -> Self {
        self.id = Some(id);
        self
    }

    /// Sets the inbox to have no ID (for insertion tests)
    pub fn without_id(mut self) -> Self {
        self.id = None;
        self
    }

    /// Sets the inbox name
    pub fn with_name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    /// Sets the inbox size
    pub fn with_size(mut self, size: u64) -> Self {
        self.size = size;
        self
    }

    /// Sets the inbox status
    pub fn with_status(mut self, status: InboxStatus) -> Self {
        self.status = status;
        self
    }

    /// Sets the warehouse path
    pub fn with_warehouse_path(mut self, path: &str) -> Self {
        self.warehouse_path = path.to_string();
        self
    }

    /// Sets the lifetime
    pub fn with_lifetime(mut self, lifetime: u64) -> Self {
        self.lifetime = Some(lifetime);
        self
    }

    /// Sets archive enabled
    pub fn with_archive_enabled(mut self, enabled: bool) -> Self {
        self.archive_enabled = enabled;
        self
    }

    /// Sets archive sites
    pub fn with_archive_sites(mut self, sites: Vec<String>) -> Self {
        self.archive_sites = sites;
        self
    }

    /// Builds the InboxDB instance
    pub fn build(self) -> InboxDB {
        let now = Utc::now();
        InboxDB {
            id: self.id,
            name: self.name,
            size: self.size,
            status: self.status,
            warehouse_path: self.warehouse_path,
            lifetime: self.lifetime,
            archive_enabled: self.archive_enabled,
            archive_sites: self.archive_sites,
            created_at: now,
            updated_at: now,
        }
    }
}

impl Default for InboxBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder pattern for creating test CreateInbox instances
#[derive(Debug, Clone)]
pub struct CreateInboxBuilder {
    name: String,
    size: u64,
    warehouse_path: String,
    lifetime: Option<u64>,
    archive_enabled: bool,
    archive_sites: Vec<String>,
}

impl CreateInboxBuilder {
    /// Creates a new CreateInboxBuilder with default values
    pub fn new() -> Self {
        Self {
            name: "Test Inbox".to_string(),
            size: 100,
            warehouse_path: "/test/warehouse/path".to_string(),
            lifetime: None,
            archive_enabled: false,
            archive_sites: vec![],
        }
    }

    /// Sets the inbox name
    pub fn with_name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    /// Sets the inbox size
    pub fn with_size(mut self, size: u64) -> Self {
        self.size = size;
        self
    }

    /// Sets the warehouse path
    pub fn with_warehouse_path(mut self, path: &str) -> Self {
        self.warehouse_path = path.to_string();
        self
    }

    /// Sets the lifetime
    pub fn with_lifetime(mut self, lifetime: u64) -> Self {
        self.lifetime = Some(lifetime);
        self
    }

    /// Sets archive enabled
    pub fn with_archive_enabled(mut self, enabled: bool) -> Self {
        self.archive_enabled = enabled;
        self
    }

    /// Sets archive sites
    pub fn with_archive_sites(mut self, sites: Vec<String>) -> Self {
        self.archive_sites = sites;
        self
    }

    /// Builds the CreateInbox instance
    pub fn build(self) -> CreateInbox {
        CreateInbox {
            name: self.name,
            size: self.size,
            warehouse_path: self.warehouse_path,
            lifetime: self.lifetime,
            archive_enabled: self.archive_enabled,
            archive_sites: self.archive_sites,
        }
    }
}

impl Default for CreateInboxBuilder {
    fn default() -> Self {
        Self::new()
    }
}
