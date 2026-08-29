use axum::{
    extract::{Json, State},
    http::StatusCode,
};
use mongodb::bson::doc;
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;
use common::{AppError, AppJson, CreateInbox, InboxStatus};

/// Response body for inbox creation (202 Accepted)
#[derive(Debug, Serialize)]
pub struct CreateInboxResponse {
    pub inbox_id: String,
    pub status: String,
    pub message: String,
}

/// Handler to create a new inbox
///
/// Inserts an `Active` inbox and fires the Citadel create trigger
/// asynchronously, returning `202 Accepted` immediately. A duplicate
/// `warehouse_path` is rejected: the fast in-process check returns 400, and
/// the unique index on `warehouse_path` turns a concurrent race into a 409.
pub async fn create_inbox(
    State(state): State<Arc<AppState>>,
    Json(input): Json<CreateInbox>,
) -> Result<(StatusCode, AppJson<CreateInboxResponse>), AppError> {
    // Validate input
    if input.warehouse_path.is_empty() {
        return Err(AppError::ValidationError(
            "warehouse_path is required".to_string(),
        ));
    }

    // Check for existing inbox with same warehouse_path (fast, friendly 400)
    let existing = state
        .store
        .find_one(doc! { "warehouse_path": &input.warehouse_path })
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

    if existing.is_some() {
        return Err(AppError::ValidationError(
            "An inbox already exists for this warehouse path".to_string(),
        ));
    }

    let now = chrono::Utc::now();
    let inbox = common::InboxDB {
        id: None,
        name: input.name,
        size: input.size,
        status: InboxStatus::Active,
        warehouse_path: input.warehouse_path,
        inbox_path: None,
        lifetime: input.lifetime,
        archive_enabled: input.archive_enabled,
        archive_sites: input.archive_sites,
        version: 0,
        current_job_id: None,
        progress: None,
        error: None,
        created_at: now,
        updated_at: now,
    };

    let result = state.store.insert_one(inbox.clone()).await.map_err(|e| {
        use mongodb::error::ErrorKind;
        use mongodb::error::WriteFailure;
        let is_duplicate = matches!(&*e.kind, ErrorKind::Write(WriteFailure::WriteError(w))
                if w.code == 11000);
        if is_duplicate {
            AppError::Conflict("An inbox already exists for this warehouse path".to_string())
        } else {
            AppError::DatabaseError(e.to_string())
        }
    })?;

    let inserted_id = result.inserted_id.as_object_id();

    // Trigger Citadel create operation (fire-and-forget). On failure, apply
    // the in-Barbican fallback (D2) so the inbox never stays `Active`/stuck.
    if let Some(ref citadel) = state.citadel_client {
        let warehouse_path = inbox.warehouse_path.clone();
        let citadel_clone = citadel.clone();
        let store = state.store.clone();
        let id = inserted_id;
        let id_str = id.map(|v| v.to_hex()).unwrap_or_default();
        tokio::spawn(async move {
            match citadel_clone.trigger_create(&id_str, &warehouse_path).await {
                Ok(resp) => tracing::info!("Citadel create triggered: job_id={}", resp.job_id),
                Err(e) => {
                    tracing::error!("Failed to trigger Citadel create: {}", e);
                    if let Some(id) = id
                        && let Err(ferr) = crate::handlers::transition::apply_trigger_failure(
                            &store,
                            &id,
                            "citadel_trigger_failed",
                            &format!("Citadel create trigger failed: {}", e),
                        )
                        .await
                    {
                        tracing::warn!("Failed to apply create-trigger failure fallback: {}", ferr);
                    }
                }
            }
        });
    }

    Ok((
        StatusCode::ACCEPTED,
        AppJson::new(CreateInboxResponse {
            inbox_id: inserted_id.map(|id| id.to_hex()).unwrap_or_default(),
            status: "active".to_string(),
            message: "Inbox creation initiated".to_string(),
        }),
    ))
}
