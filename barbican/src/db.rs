//! Database helpers for Barbican
//!
//! Provides typed collection access plus idempotent index management for the
//! shared `inboxes` collection.

use mongodb::{Collection, IndexModel, options::IndexOptions};

use common::InboxDB;

/// Name of the unique `warehouse_path` index.
pub const UNIQ_WAREHOUSE_PATH_INDEX: &str = "uniq_warehouse_path";

/// Build the unique index spec on `warehouse_path` (idempotent).
///
/// A pure helper so it can be unit-tested without touching MongoDB; the
/// caller performs the actual `create_index_one` I/O.
pub fn ensure_inbox_indexes_spec() -> IndexModel {
    IndexModel::builder()
        .keys(mongodb::bson::doc! { "warehouse_path": 1 })
        .options(
            IndexOptions::builder()
                .unique(true)
                .name(UNIQ_WAREHOUSE_PATH_INDEX.to_string())
                .build(),
        )
        .build()
}

/// Ensure the `inboxes` collection has the unique `warehouse_path` index.
///
/// Idempotent (`create_index_one` is a no-op if the index already exists).
/// Errors are logged and swallowed so a pre-existing index with a different
/// shape does not prevent service startup (operators should recreate the
/// index manually if the shape ever differs from the spec).
pub async fn ensure_inbox_indexes(collection: &Collection<InboxDB>) {
    let spec = ensure_inbox_indexes_spec();
    match collection.create_index(spec).await {
        Ok(_) => {
            tracing::info!(
                "Ensured unique index {} on warehouse_path",
                UNIQ_WAREHOUSE_PATH_INDEX
            );
        }
        Err(e) => {
            tracing::warn!(
                "Failed to ensure unique index {} on warehouse_path: {} \
                 (if the index already exists with a different shape, recreate it manually)",
                UNIQ_WAREHOUSE_PATH_INDEX,
                e
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ensure_inbox_indexes_spec_is_unique_on_warehouse_path() {
        let spec = ensure_inbox_indexes_spec();
        let keys = &spec.keys;
        assert_eq!(
            keys.get("warehouse_path"),
            Some(&mongodb::bson::Bson::Int32(1))
        );
        let options = spec.options.unwrap();
        assert_eq!(options.unique, Some(true));
        assert_eq!(options.name.as_deref(), Some(UNIQ_WAREHOUSE_PATH_INDEX));
    }
}
