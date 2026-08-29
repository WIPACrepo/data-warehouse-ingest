use axum::extract::{Json, Path, State};
use mongodb::bson::{doc, oid::ObjectId};
use std::sync::Arc;

use crate::AppState;
use common::{AppError, AppJson, InboxDB, InboxProgress, InboxStatus};
use serde::{Deserialize, Serialize};

use super::apply_versioned_update;

/// Request body for delete start operation
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DeleteStartRequest {
    pub job_id: String,
}

/// Request body for delete progress operation
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DeleteProgressRequest {
    pub job_id: String,
    pub phase: String,
    #[serde(default)]
    pub total_files: u64,
    #[serde(default)]
    pub processed_files: u64,
    #[serde(default)]
    pub total_bytes: u64,
    #[serde(default)]
    pub deleted_bytes: u64,
}

/// Request body for delete complete operation
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DeleteCompleteRequest {
    pub job_id: String,
}

/// Request body for delete failure operation
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DeleteFailureRequest {
    pub job_id: String,
    pub error_code: String,
    pub error_message: String,
    #[serde(default)]
    pub total_bytes: u64,
    #[serde(default)]
    pub deleted_bytes: u64,
}

/// Start delete operation - transitions from Ready/Failed to Deleting
pub async fn delete_start(
    State(state): State<Arc<AppState>>,
    Path(id): Path<ObjectId>,
    Json(input): Json<DeleteStartRequest>,
) -> Result<AppJson<InboxDB>, AppError> {
    let mut inbox = state.store
        .find_one(doc! { "_id": id })
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?
        .ok_or_else(|| AppError::InboxNotFound(id.to_string()))?;

    // Validate state transition - can only delete from Ready or Failed state
    if inbox.status != InboxStatus::Ready && inbox.status != InboxStatus::Failed {
        return Err(AppError::InvalidStateTransition(format!(
            "Cannot start delete from state {:?}",
            inbox.status
        )));
    }

    inbox.status = InboxStatus::Deleting;
    inbox.current_job_id = Some(input.job_id);
    inbox.updated_at = chrono::Utc::now();

    let update = doc! {
        "$set": {
            "status": "deleting",
            "current_job_id": inbox.current_job_id.clone(),
            "updated_at": mongodb::bson::DateTime::from_system_time(inbox.updated_at.into()),
            "version": inbox.version + 1,
        }
    };

    apply_versioned_update(&state.store, &id, inbox.version, &update).await?;

    Ok(AppJson::new(inbox))
}

/// Report delete progress - stays in Deleting state
pub async fn delete_progress(
    State(state): State<Arc<AppState>>,
    Path(id): Path<ObjectId>,
    Json(input): Json<DeleteProgressRequest>,
) -> Result<AppJson<InboxDB>, AppError> {
    let mut inbox = state.store
        .find_one(doc! { "_id": id })
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?
        .ok_or_else(|| AppError::InboxNotFound(id.to_string()))?;

    // Validate state transition
    if inbox.status != InboxStatus::Deleting {
        return Err(AppError::InvalidStateTransition(format!(
            "Cannot report progress from state {:?}",
            inbox.status
        )));
    }

    // Validate job_id matches
    if inbox.current_job_id.as_ref() != Some(&input.job_id) {
        return Err(AppError::ValidationError("Job ID does not match current operation".to_string()));
    }

    let now = chrono::Utc::now();
    let progress = InboxProgress {
        phase: input.phase,
        total_files: input.total_files,
        processed_files: input.processed_files,
        total_bytes: input.total_bytes,
        transferred_bytes: 0,
        deleted_bytes: input.deleted_bytes,
        updated_at: now,
    };

    inbox.progress = Some(progress.clone());
    inbox.updated_at = now;

    let update = doc! {
        "$set": {
            "progress": mongodb::bson::to_document(&progress).unwrap(),
            "updated_at": mongodb::bson::DateTime::from_system_time(inbox.updated_at.into()),
            "version": inbox.version + 1,
        }
    };

    apply_versioned_update(&state.store, &id, inbox.version, &update).await?;

    Ok(AppJson::new(inbox))
}

/// Complete delete operation - transitions from Deleting to Deleted
pub async fn delete_complete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<ObjectId>,
    Json(input): Json<DeleteCompleteRequest>,
) -> Result<AppJson<InboxDB>, AppError> {
    let mut inbox = state.store
        .find_one(doc! { "_id": id })
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?
        .ok_or_else(|| AppError::InboxNotFound(id.to_string()))?;

    // Validate state transition
    if inbox.status != InboxStatus::Deleting {
        return Err(AppError::InvalidStateTransition(format!(
            "Cannot complete delete from state {:?}",
            inbox.status
        )));
    }

    // Validate job_id matches
    if inbox.current_job_id.as_ref() != Some(&input.job_id) {
        return Err(AppError::ValidationError("Job ID does not match current operation".to_string()));
    }

    inbox.status = InboxStatus::Deleted;
    inbox.current_job_id = None;
    inbox.progress = None;
    inbox.updated_at = chrono::Utc::now();

    let update = doc! {
        "$set": {
            "status": "deleted",
            "current_job_id": None::<String>,
            "progress": None::<mongodb::bson::Document>,
            "updated_at": mongodb::bson::DateTime::from_system_time(inbox.updated_at.into()),
            "version": inbox.version + 1,
        }
    };

    apply_versioned_update(&state.store, &id, inbox.version, &update).await?;

    Ok(AppJson::new(inbox))
}

/// Fail delete operation - transitions from Deleting to Failed
pub async fn delete_failure(
    State(state): State<Arc<AppState>>,
    Path(id): Path<ObjectId>,
    Json(input): Json<DeleteFailureRequest>,
) -> Result<AppJson<InboxDB>, AppError> {
    let mut inbox = state.store
        .find_one(doc! { "_id": id })
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?
        .ok_or_else(|| AppError::InboxNotFound(id.to_string()))?;

    // Validate state transition
    if inbox.status != InboxStatus::Deleting {
        return Err(AppError::InvalidStateTransition(format!(
            "Cannot fail delete from state {:?}",
            inbox.status
        )));
    }

    // Validate job_id matches
    if inbox.current_job_id.as_ref() != Some(&input.job_id) {
        return Err(AppError::ValidationError("Job ID does not match current operation".to_string()));
    }

    let now = chrono::Utc::now();
    let error = common::InboxError {
        code: input.error_code,
        message: input.error_message,
        job_id: input.job_id,
        occurred_at: now,
    };

    inbox.status = InboxStatus::Failed;
    inbox.error = Some(error.clone());
    inbox.current_job_id = None;
    inbox.progress = None;
    inbox.updated_at = now;

    let update = doc! {
        "$set": {
            "status": "failed",
            "error": mongodb::bson::to_document(&error).unwrap(),
            "current_job_id": None::<String>,
            "progress": None::<mongodb::bson::Document>,
            "updated_at": mongodb::bson::DateTime::from_system_time(inbox.updated_at.into()),
            "version": inbox.version + 1,
        }
    };

    apply_versioned_update(&state.store, &id, inbox.version, &update).await?;

    Ok(AppJson::new(inbox))
}
