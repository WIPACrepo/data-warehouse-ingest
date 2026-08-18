//! Database helpers for Citadel
//!
//! Delegates to [`common::db`] for connection setup and provides typed
//! collection helpers for Citadel-specific models.

use common::db::db_collection as common_db_collection;
use mongodb::{Client, Collection};

use crate::config::CitadelConfig;
use crate::models::{CallbackNonceDB, JobDB};

/// Collection names
pub const COLLECTION_INBOXES: &str = "inboxes";
pub const COLLECTION_JOBS: &str = "jobs";
pub const COLLECTION_CALLBACK_NONCES: &str = "callback_nonces";

/// Get the inboxes collection (shared with Barbican, read-only for Citadel)
pub fn inboxes_collection(
    client: &Client,
    config: &CitadelConfig,
) -> Collection<common::InboxDB> {
    common_db_collection(client, &config.mongodb_database, COLLECTION_INBOXES)
}

/// Get the jobs collection (Citadel-owned)
pub fn jobs_collection(
    client: &Client,
    config: &CitadelConfig,
) -> Collection<JobDB> {
    common_db_collection(client, &config.mongodb_database, COLLECTION_JOBS)
}

/// Get the callback nonces collection (Citadel-owned)
pub fn callback_nonces_collection(
    client: &Client,
    config: &CitadelConfig,
) -> Collection<CallbackNonceDB> {
    common_db_collection(client, &config.mongodb_database, COLLECTION_CALLBACK_NONCES)
}