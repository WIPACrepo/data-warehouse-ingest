//! Publish inbox handler - starts file transfer job

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    Json,
};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};
use tracing::{error, info};
use uuid::Uuid;

use crate::AppState;
use crate::auth::AuthState;

use crate::error::{CitadelError, Result};
use crate::jobs::types::JobType;
use crate::models::JobDB;

#[derive(Debug, Serialize)]
pub struct PublishInboxResponse {
    pub job_id: String,
    pub inbox_id: String,
    pub status: String,
    pub job_type: String,
}

pub async fn publish_inbox(
    State(app_state): State<AppState>,
    Extension(auth_state): Extension<AuthState>,
    Path(inbox_id): Path<String>,
) -> Result<(StatusCode, Json<PublishInboxResponse>)> {
    // Check authorization
    if !auth_state.claims.can_write() {
        return Err(CitadelError::Forbidden(
            "Insufficient permissions to publish inbox".to_string(),
        ));
    }

    let object_id = ObjectId::parse_str(&inbox_id)
        .map_err(|_| CitadelError::InboxNotFound(inbox_id))?;

    // Get inbox metadata from Barbican to validate state
    let inbox = app_state.barbican_client
        .get_inbox(&object_id)
        .await
        .map_err(|e| {
            error!("Failed to get inbox from Barbican: {}", e);
            e
        })?;

    // Validate inbox state - must have inbox_path to publish
    if inbox.inbox_path.is_none() {
        return Err(CitadelError::InvalidInboxState(
            "Inbox must be created before publishing".to_string(),
        ));
    }

    // Create job record
    let job_id = Uuid::new_v4().to_string();
    let job_type = JobType::PublishInbox {
        inbox_id: object_id,
        notify_catalog: true,
        notify_lta: true,
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
            CitadelError::DatabaseError(format!("Failed to create job: {}", e))
        })?;

    // Transition inbox state to publishing via Barbican
    app_state.barbican_client
        .transition_inbox_state(&object_id, "publishing")
        .await
        .map_err(|e| {
            error!("Failed to update inbox state in Barbican: {}", e);
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
            CitadelError::DatabaseError(format!("Failed to update job: {}", e))
        })?;

    info!("Created publish job {} for inbox {}", job_id, object_id);

    Ok((
        StatusCode::ACCEPTED,
        Json(PublishInboxResponse {
            job_id,
            inbox_id: object_id.to_string(),
            status: "running".to_string(),
            job_type: "publish".to_string(),
        }),
    ))
}