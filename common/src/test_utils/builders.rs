use crate::models::{CreateInbox, InboxDB};
use mongodb::bson::oid::ObjectId;

/// Builder pattern for creating test InboxDB instances
#[derive(Debug, Clone)]
pub struct InboxBuilder {
    id: Option<ObjectId>,
    name: String,
    size: u64,
    active: bool,
}

impl InboxBuilder {
    /// Creates a new InboxBuilder with default values
    pub fn new() -> Self {
        Self {
            id: Some(ObjectId::new()),
            name: "Test Inbox".to_string(),
            size: 100,
            active: true,
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

    /// Sets the inbox active status
    pub fn with_active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Sets the inbox to inactive
    pub fn inactive(mut self) -> Self {
        self.active = false;
        self
    }

    /// Builds the InboxDB instance
    pub fn build(self) -> InboxDB {
        InboxDB {
            id: self.id,
            name: self.name,
            size: self.size,
            active: self.active,
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
}

impl CreateInboxBuilder {
    /// Creates a new CreateInboxBuilder with default values
    pub fn new() -> Self {
        Self {
            name: "Test Inbox".to_string(),
            size: 100,
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

    /// Builds the CreateInbox instance
    pub fn build(self) -> CreateInbox {
        CreateInbox {
            name: self.name,
            size: self.size,
        }
    }
}

impl Default for CreateInboxBuilder {
    fn default() -> Self {
        Self::new()
    }
}