pub mod create;
pub mod delete;
pub mod publish;

pub use create::{create_start, create_progress, create_complete, create_failure};
pub use delete::{delete_start, delete_progress, delete_complete, delete_failure};
pub use publish::{publish_start, publish_progress, publish_complete, publish_failure};

use common::{AppError, InboxStore};
use mongodb::bson::{doc, oid::ObjectId, Document};
use std::sync::Arc;

/// Apply a version-conditional update; map zero matched docs to Conflict.
///
/// This is the optimistic-locking primitive used by every state transition
/// handler. The filter is `{_id, version}` (the version read when the inbox
/// was fetched). If no document matches, the inbox was modified concurrently
/// and the transition is rejected with a `409 Conflict` per the documented
/// contract (`state-machine.md` Optimistic Locking).
pub(crate) async fn apply_versioned_update(
    store: &Arc<dyn InboxStore>,
    id: &ObjectId,
    version: u32,
    update: &Document,
) -> Result<(), AppError> {
    let result = store
        .update_one(
            doc! { "_id": id, "version": version },
            update.clone(),
        )
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

    if result.matched_count == 0 {
        return Err(AppError::Conflict(
            "Inbox state was modified concurrently".to_string(),
        ));
    }

    Ok(())
}
