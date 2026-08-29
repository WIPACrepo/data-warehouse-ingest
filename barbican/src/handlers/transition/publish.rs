use axum::extract::{Json, Path, State};
use mongodb::bson::{doc, oid::ObjectId};
use std::sync::Arc;

use crate::AppState;
use common::{AppError, AppJson, InboxDB, InboxProgress, InboxStatus};
use serde::{Deserialize, Serialize};

use super::apply_versioned_update;

/// Request body for publish start operation
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PublishStartRequest {
    pub job_id: String,
}

/// Request body for publish progress operation
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PublishProgressRequest {
    pub job_id: String,
    pub phase: String,
    #[serde(default)]
    pub total_files: u64,
    #[serde(default)]
    pub processed_files: u64,
    #[serde(default)]
    pub total_bytes: u64,
    #[serde(default)]
    pub transferred_bytes: u64,
}

/// Request body for publish complete operation
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PublishCompleteRequest {
    pub job_id: String,
    pub files_transferred: u64,
    pub total_bytes: u64,
    /// Accepted but unused (kept for shape-compat with Citadel's client).
    /// Barbican neither validates nor stores this field.
    pub protected_path: String,
}

/// Request body for publish failure operation
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PublishFailureRequest {
    pub job_id: String,
    pub error_code: String,
    pub error_message: String,
    #[serde(default)]
    pub files_transferred: u64,
    #[serde(default)]
    pub total_bytes: u64,
}

/// Start publish operation - transitions from Ready to Publishing
pub async fn publish_start(
    State(state): State<Arc<AppState>>,
    Path(id): Path<ObjectId>,
    Json(input): Json<PublishStartRequest>,
) -> Result<AppJson<InboxDB>, AppError> {
    let mut inbox = state
        .store
        .find_one(doc! { "_id": id })
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?
        .ok_or_else(|| AppError::InboxNotFound(id.to_string()))?;

    // Validate state transition
    if inbox.status != InboxStatus::Ready {
        return Err(AppError::InvalidStateTransition(format!(
            "Cannot start publish from state {:?}",
            inbox.status
        )));
    }

    inbox.status = InboxStatus::Publishing;
    inbox.current_job_id = Some(input.job_id);
    inbox.updated_at = chrono::Utc::now();

    let update = doc! {
        "$set": {
            "status": "publishing",
            "current_job_id": inbox.current_job_id.clone(),
            "updated_at": mongodb::bson::DateTime::from_system_time(inbox.updated_at.into()),
            "version": inbox.version + 1,
        }
    };

    apply_versioned_update(&state.store, &id, inbox.version, &update).await?;

    Ok(AppJson::new(inbox))
}

/// Report publish progress - stays in Publishing state
pub async fn publish_progress(
    State(state): State<Arc<AppState>>,
    Path(id): Path<ObjectId>,
    Json(input): Json<PublishProgressRequest>,
) -> Result<AppJson<InboxDB>, AppError> {
    let mut inbox = state
        .store
        .find_one(doc! { "_id": id })
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?
        .ok_or_else(|| AppError::InboxNotFound(id.to_string()))?;

    // Validate state transition
    if inbox.status != InboxStatus::Publishing {
        return Err(AppError::InvalidStateTransition(format!(
            "Cannot report progress from state {:?}",
            inbox.status
        )));
    }

    // Validate job_id matches
    if inbox.current_job_id.as_ref() != Some(&input.job_id) {
        return Err(AppError::ValidationError(
            "Job ID does not match current operation".to_string(),
        ));
    }

    let now = chrono::Utc::now();
    let progress = InboxProgress {
        phase: input.phase,
        total_files: input.total_files,
        processed_files: input.processed_files,
        total_bytes: input.total_bytes,
        transferred_bytes: input.transferred_bytes,
        deleted_bytes: 0,
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

/// Complete publish operation - transitions from Publishing to Published
pub async fn publish_complete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<ObjectId>,
    Json(input): Json<PublishCompleteRequest>,
) -> Result<AppJson<InboxDB>, AppError> {
    let mut inbox = state
        .store
        .find_one(doc! { "_id": id })
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?
        .ok_or_else(|| AppError::InboxNotFound(id.to_string()))?;

    // Validate state transition
    if inbox.status != InboxStatus::Publishing {
        return Err(AppError::InvalidStateTransition(format!(
            "Cannot complete publish from state {:?}",
            inbox.status
        )));
    }

    // Validate job_id matches
    if inbox.current_job_id.as_ref() != Some(&input.job_id) {
        return Err(AppError::ValidationError(
            "Job ID does not match current operation".to_string(),
        ));
    }

    inbox.status = InboxStatus::Published;
    inbox.current_job_id = None;
    inbox.progress = None;
    inbox.updated_at = chrono::Utc::now();

    let update = doc! {
        "$set": {
            "status": "published",
            "current_job_id": None::<String>,
            "progress": None::<mongodb::bson::Document>,
            "updated_at": mongodb::bson::DateTime::from_system_time(inbox.updated_at.into()),
            "version": inbox.version + 1,
        }
    };

    apply_versioned_update(&state.store, &id, inbox.version, &update).await?;

    Ok(AppJson::new(inbox))
}

/// Fail publish operation - transitions from Publishing to Failed
pub async fn publish_failure(
    State(state): State<Arc<AppState>>,
    Path(id): Path<ObjectId>,
    Json(input): Json<PublishFailureRequest>,
) -> Result<AppJson<InboxDB>, AppError> {
    let mut inbox = state
        .store
        .find_one(doc! { "_id": id })
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?
        .ok_or_else(|| AppError::InboxNotFound(id.to_string()))?;

    // Validate state transition
    if inbox.status != InboxStatus::Publishing {
        return Err(AppError::InvalidStateTransition(format!(
            "Cannot fail publish from state {:?}",
            inbox.status
        )));
    }

    // Validate job_id matches
    if inbox.current_job_id.as_ref() != Some(&input.job_id) {
        return Err(AppError::ValidationError(
            "Job ID does not match current operation".to_string(),
        ));
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
