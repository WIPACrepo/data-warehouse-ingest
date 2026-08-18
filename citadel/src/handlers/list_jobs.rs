//! List jobs handler

use axum::{
    extract::{Extension, Query, State},
    http::StatusCode,
    Json,
};
use futures::stream::TryStreamExt;
use mongodb::bson::doc;
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::auth::AuthState;

use crate::db::jobs_collection;
use crate::error::{AppError, Result};
use crate::jobs::types::JobType;

#[derive(Debug, Deserialize)]
pub struct ListJobsParams {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub inbox_id: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: u64,
}

fn default_limit() -> u64 {
    100
}

#[derive(Debug, Serialize)]
pub struct JobSummary {
    pub job_id: String,
    pub inbox_id: String,
    pub job_type: String,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
pub struct ListJobsResponse {
    pub jobs: Vec<JobSummary>,
}

pub async fn list_jobs(
    State(app_state): State<AppState>,
    Extension(auth_state): Extension<AuthState>,
    Query(params): Query<ListJobsParams>,
) -> Result<(StatusCode, Json<ListJobsResponse>)> {
    // Check authorization - read role is sufficient
    if !auth_state.claims.can_read() {
        return Err(AppError::Forbidden(
            "Insufficient permissions to list jobs".to_string(),
        ));
    }

    let jobs_coll = jobs_collection(&app_state.db_client, &app_state.config);
    
    // Build query filter
    let mut filter = doc! {};
    
    if let Some(status) = params.status {
        filter.insert("status", status.to_lowercase());
    }
    
    if let Some(inbox_id) = params.inbox_id {
        if let Ok(object_id) = mongodb::bson::oid::ObjectId::parse_str(&inbox_id) {
            filter.insert("inbox_id", object_id);
        }
    }
    
    let _options = mongodb::options::FindOptions::builder()
        .limit(params.limit as i64)
        .sort(doc! { "created_at": -1 })
        .build();
    
    let mut cursor = jobs_coll.find(filter).await
        .map_err(|e| AppError::DatabaseError(format!("Failed to query jobs: {}", e)))?;
    
    let mut jobs = Vec::new();
    while let Some(job) = cursor.try_next().await
        .map_err(|e| AppError::DatabaseError(format!("Failed to iterate cursor: {}", e)))? 
    {
        let job_type_str = match &job.job_type {
            JobType::PublishInbox { .. } => "publish",
            JobType::DeleteInbox { .. } => "delete",
        };
        jobs.push(JobSummary {
            job_id: job.job_id,
            inbox_id: job.inbox_id.to_string(),
            job_type: job_type_str.to_string(),
            status: format!("{:?}", job.status).to_lowercase(),
            created_at: job.created_at,
        });
    }

    Ok((
        StatusCode::OK,
        Json(ListJobsResponse { jobs }),
    ))
}