//! Database helpers for Citadel
//!
//! Delegates to [`common::db`] for connection setup and provides typed
//! collection helpers for Citadel-specific models.

use common::db::db_collection as common_db_collection;
use mongodb::{Client, Collection, IndexModel, options::IndexOptions};

use crate::config::CitadelConfig;
use crate::models::{CallbackNonceDB, JobDB};

/// Collection names
pub const COLLECTION_INBOXES: &str = "inboxes";
pub const COLLECTION_JOBS: &str = "jobs";
pub const COLLECTION_CALLBACK_NONCES: &str = "callback_nonces";

/// Name of the TTL index on `callback_nonces.expires_at`.
pub const TTL_EXPIRES_AT_INDEX: &str = "ttl_expires_at";

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

/// Build the TTL index spec on `callback_nonces.expires_at` (pure helper).
///
/// A TTL index with `expire_after_seconds = 0` removes documents as soon as
/// their `expires_at` passes, which keeps the nonce table bounded. Created
/// idempotently at Citadel startup.
pub fn ensure_citadel_indexes_spec() -> IndexModel {
    IndexModel::builder()
        .keys(mongodb::bson::doc! { "expires_at": 1 })
        .options(IndexOptions::builder()
            .expire_after(Some(std::time::Duration::from_secs(0)))
            .name(TTL_EXPIRES_AT_INDEX.to_string())
            .build())
        .build()
}

/// Ensure the callback-nonces collection has the TTL index (idempotent).
///
/// Errors are logged and swallowed so service startup does not fail if the
/// index already exists in a different shape (operators should recreate it
/// manually in that case).
pub async fn ensure_citadel_indexes(client: &Client, config: &CitadelConfig) {
    let coll = callback_nonces_collection(client, config);
    let spec = ensure_citadel_indexes_spec();
    match coll.create_index(spec).await {
        Ok(_) => {
            tracing::info!(
                "Ensured TTL index {} on callback_nonces.expires_at",
                TTL_EXPIRES_AT_INDEX
            );
        }
        Err(e) => {
            tracing::warn!(
                "Failed to ensure TTL index {} on callback_nonces.expires_at: {} \
                 (if the index already exists with a different shape, recreate it manually)",
                TTL_EXPIRES_AT_INDEX,
                e
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ensure_citadel_indexes_spec_is_ttl_on_expires_at() {
        let spec = ensure_citadel_indexes_spec();
        let keys = &spec.keys;
        assert_eq!(
            keys.get("expires_at"),
            Some(&mongodb::bson::Bson::Int32(1))
        );
        let options = spec.options.unwrap();
        assert_eq!(options.expire_after, Some(std::time::Duration::from_secs(0)));
        assert_eq!(options.name.as_deref(), Some(TTL_EXPIRES_AT_INDEX));
    }
}
