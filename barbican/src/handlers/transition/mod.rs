pub mod create;
pub mod delete;
pub mod publish;

pub use create::{create_start, create_progress, create_complete, create_failure};
pub use delete::{delete_start, delete_progress, delete_complete, delete_failure};
pub use publish::{publish_start, publish_progress, publish_complete, publish_failure};

use common::{AppError, InboxError, InboxStatus, InboxStore};
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

/// Fallback applied when a fire-and-forget Citadel trigger fails (D2).
///
/// Transitions an inbox that is stuck in a trigger-owned in-progress state
/// (`Active`/`Creating` for create, `Publishing`/`Deleting` for
/// publish/delete) to `Failed` with the given error code, so it never sits in
/// that state indefinitely when Citadel is down. A benign `409`
/// (the inbox already transitioned, e.g. a retry succeeded) is logged and
/// swallowed.
pub(crate) async fn apply_trigger_failure(
    store: &Arc<dyn InboxStore>,
    id: &ObjectId,
    error_code: &str,
    error_message: &str,
) -> Result<(), AppError> {
    let mut inbox = store
        .find_one(doc! { "_id": id })
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?
        .ok_or_else(|| AppError::InboxNotFound(id.to_string()))?;

    // Only the states owned by a trigger may be failed by this path. If the
    // inbox already moved on (Ready/Published/Deleted/Failed) leave it alone.
    let in_progress = matches!(
        inbox.status,
        InboxStatus::Active
            | InboxStatus::Creating
            | InboxStatus::Publishing
            | InboxStatus::Deleting
    );
    if !in_progress {
        return Ok(());
    }

    let now = chrono::Utc::now();
    let error = InboxError {
        code: error_code.to_string(),
        message: error_message.to_string(),
        job_id: inbox.current_job_id.clone().unwrap_or_default(),
        occurred_at: now,
    };

    inbox.status = InboxStatus::Failed;
    inbox.error = Some(error.clone());
    inbox.progress = None;
    inbox.updated_at = now;

    let update = doc! {
        "$set": {
            "status": "failed",
            "error": mongodb::bson::to_document(&error).unwrap(),
            "current_job_id": inbox.current_job_id.clone(),
            "progress": None::<mongodb::bson::Document>,
            "updated_at": mongodb::bson::DateTime::from_system_time(inbox.updated_at.into()),
            "version": inbox.version + 1,
        }
    };

    match apply_versioned_update(store, id, inbox.version, &update).await {
        Ok(()) => {
            tracing::warn!(
                "Inbox {} marked failed after Citadel trigger failure ({})",
                id,
                error_code
            );
            Ok(())
        }
        // Benign: inbox already transitioned concurrently (e.g. a retried
        // trigger succeeded) — nothing to roll back.
        Err(AppError::Conflict(_)) => Ok(()),
        Err(e) => Err(e),
    }
}
