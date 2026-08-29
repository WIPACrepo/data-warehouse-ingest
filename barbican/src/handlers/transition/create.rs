use axum::extract::{Json, Path, State};
use mongodb::bson::{doc, oid::ObjectId};
use std::sync::Arc;

use crate::AppState;
use common::{AppError, AppJson, InboxDB, InboxError, InboxProgress, InboxStatus};
use serde::{Deserialize, Serialize};

use super::apply_versioned_update;

/// Request body for create start operation
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CreateStartRequest {
    pub job_id: String,
}

/// Request body for create progress operation
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CreateProgressRequest {
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

/// Request body for create complete operation
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CreateCompleteRequest {
    pub job_id: String,
    pub inbox_path: String,
}

/// Request body for create failure operation
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CreateFailureRequest {
    pub job_id: String,
    pub error_code: String,
    pub error_message: String,
    #[serde(default)]
    pub total_bytes: u64,
    #[serde(default)]
    pub transferred_bytes: u64,
}

/// Start inbox creation - transitions from Active to Creating
pub async fn create_start(
    State(state): State<Arc<AppState>>,
    Path(id): Path<ObjectId>,
    Json(input): Json<CreateStartRequest>,
) -> Result<AppJson<InboxDB>, AppError> {
    let mut inbox = state
        .store
        .find_one(doc! { "_id": id })
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?
        .ok_or_else(|| AppError::InboxNotFound(id.to_string()))?;

    // Validate state transition
    if inbox.status != InboxStatus::Active {
        return Err(AppError::InvalidStateTransition(format!(
            "Cannot start creation from state {:?}",
            inbox.status
        )));
    }

    // Update inbox status
    inbox.status = InboxStatus::Creating;
    inbox.current_job_id = Some(input.job_id);
    inbox.updated_at = chrono::Utc::now();

    let update = doc! {
        "$set": {
            "status": "creating",
            "current_job_id": inbox.current_job_id.clone(),
            "updated_at": mongodb::bson::DateTime::from_system_time(inbox.updated_at.into()),
            "version": inbox.version + 1,
        }
    };

    apply_versioned_update(&state.store, &id, inbox.version, &update).await?;

    Ok(AppJson::new(inbox))
}

/// Report create progress - stays in Creating state
pub async fn create_progress(
    State(state): State<Arc<AppState>>,
    Path(id): Path<ObjectId>,
    Json(input): Json<CreateProgressRequest>,
) -> Result<AppJson<InboxDB>, AppError> {
    let mut inbox = state
        .store
        .find_one(doc! { "_id": id })
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?
        .ok_or_else(|| AppError::InboxNotFound(id.to_string()))?;

    // Validate state transition
    if inbox.status != InboxStatus::Creating {
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

/// Complete inbox creation - transitions from Creating to Ready
pub async fn create_complete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<ObjectId>,
    Json(input): Json<CreateCompleteRequest>,
) -> Result<AppJson<InboxDB>, AppError> {
    let mut inbox = state
        .store
        .find_one(doc! { "_id": id })
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?
        .ok_or_else(|| AppError::InboxNotFound(id.to_string()))?;

    // Validate state transition
    if inbox.status != InboxStatus::Creating {
        return Err(AppError::InvalidStateTransition(format!(
            "Cannot complete creation from state {:?}",
            inbox.status
        )));
    }

    // Validate job_id matches
    if inbox.current_job_id.as_ref() != Some(&input.job_id) {
        return Err(AppError::ValidationError(
            "Job ID does not match current operation".to_string(),
        ));
    }

    inbox.status = InboxStatus::Ready;
    inbox.inbox_path = Some(input.inbox_path);
    inbox.current_job_id = None;
    inbox.progress = None;
    inbox.updated_at = chrono::Utc::now();

    let update = doc! {
        "$set": {
            "status": "ready",
            "inbox_path": inbox.inbox_path.clone(),
            "current_job_id": None::<String>,
            "progress": None::<mongodb::bson::Document>,
            "updated_at": mongodb::bson::DateTime::from_system_time(inbox.updated_at.into()),
            "version": inbox.version + 1,
        }
    };

    apply_versioned_update(&state.store, &id, inbox.version, &update).await?;

    Ok(AppJson::new(inbox))
}

/// Fail inbox creation - transitions from Creating to Failed
pub async fn create_failure(
    State(state): State<Arc<AppState>>,
    Path(id): Path<ObjectId>,
    Json(input): Json<CreateFailureRequest>,
) -> Result<AppJson<InboxDB>, AppError> {
    let mut inbox = state
        .store
        .find_one(doc! { "_id": id })
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?
        .ok_or_else(|| AppError::InboxNotFound(id.to_string()))?;

    // Validate state transition
    if inbox.status != InboxStatus::Creating {
        return Err(AppError::InvalidStateTransition(format!(
            "Cannot fail creation from state {:?}",
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
    let error = InboxError {
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
