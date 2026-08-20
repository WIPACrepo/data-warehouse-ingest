//! Delete inbox handler - starts inbox deletion job

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    Json,
};
use mongodb::bson::{doc, oid::ObjectId};
use serde::Serialize;
use tracing::{error, info};
use uuid::Uuid;

use crate::AppState;
use crate::auth::AuthState;
use crate::db::inboxes_collection;

use crate::error::{AppError, Result};
use crate::jobs::types::JobType;
use crate::models::JobDB;

#[derive(Debug, Serialize)]
pub struct DeleteInboxResponse {
    pub job_id: String,
    pub inbox_id: String,
    pub status: String,
    pub job_type: String,
}

pub async fn delete_inbox(
    State(app_state): State<AppState>,
    Extension(auth_state): Extension<AuthState>,
    Path(inbox_id): Path<String>,
) -> Result<(StatusCode, Json<DeleteInboxResponse>)> {
    // Check authorization
    if !auth_state.claims.can_write() {
        return Err(AppError::Forbidden(
            "Insufficient permissions to delete inbox".to_string(),
        ));
    }

    let object_id = ObjectId::parse_str(&inbox_id)
        .map_err(|_| AppError::InboxNotFound(inbox_id.clone()))?;

    // Get inbox metadata directly from MongoDB (shared collection, read-only for Citadel)
    let inboxes_coll = inboxes_collection(&app_state.db_client, &app_state.config);
    let inbox = inboxes_coll
        .find_one(doc! { "_id": object_id })
        .await
        .map_err(|e| {
            error!("Failed to get inbox from MongoDB: {}", e);
            AppError::DatabaseError(format!("Failed to query inbox: {}", e))
        })?
        .ok_or_else(|| AppError::InboxNotFound(inbox_id.clone()))?;

    // Validate inbox must exist and have a path to be deleted
    if inbox.inbox_path.is_none() {
        return Err(AppError::InvalidStateTransition(
            "Inbox must be created before deletion".to_string(),
        ));
    }

    // Create job record
    let job_id = Uuid::new_v4().to_string();
    let job_type = JobType::DeleteInbox {
        inbox_id: object_id,
    };

    let executor_type = app_state.config.job_executor.to_string();

    let mut job = JobDB::new(
        job_id.clone(),
        object_id,
        job_type.clone(),
        &executor_type,
    );

    // Insert job into MongoDB
    let jobs_coll = crate::db::jobs_collection(&app_state.db_client, &app_state.config);
    jobs_coll
        .insert_one(&job)
        .await
        .map_err(|e| {
            error!("Failed to insert job into MongoDB: {}", e);
            AppError::DatabaseError(format!("Failed to create job: {}", e))
        })?;

    // Transition inbox state via Barbican delete_start action
    app_state.barbican_client
        .delete_start(&object_id, &job_id)
        .await
        .map_err(|e| {
            error!("Failed to signal delete start to Barbican: {}", e);
            e
        })?;

    // Submit job to executor (Kubernetes or Mock)
    let executor_id = app_state.executor.submit(&job).await?;

    // Update job with executor info
    job.executor.executor_id = Some(executor_id);
    job.status = crate::jobs::types::JobStatus::Running;

    let jobs_coll = crate::db::jobs_collection(&app_state.db_client, &app_state.config);
    jobs_coll
        .replace_one(
            mongodb::bson::doc! { "job_id": &job_id },
            &job,
        )
        .await
        .map_err(|e| {
            error!("Failed to update job in MongoDB: {}", e);
            AppError::DatabaseError(format!("Failed to update job: {}", e))
        })?;

    info!("Created delete job {} for inbox {}", job_id, object_id);

    Ok((
        StatusCode::ACCEPTED,
        Json(DeleteInboxResponse {
            job_id,
            inbox_id: object_id.to_string(),
            status: "running".to_string(),
            job_type: "delete".to_string(),
        }),
    ))
}