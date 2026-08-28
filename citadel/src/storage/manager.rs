//! Storage manager for inbox operations

use std::path::{Path, PathBuf};
use crate::config::CitadelConfig;
use crate::error::{AppError, Result};
use crate::models::TransferResult;
use crate::storage::transfer::TransferError;
use mongodb::bson::oid::ObjectId;
use tracing::{info, error};

/// Storage manager for file operations
pub struct StorageManager {
    inbox_base: PathBuf,
    protected_base: PathBuf,
    max_parallel: usize,
    chunk_size_mb: usize,
    config: CitadelConfig,
}

impl StorageManager {
    pub fn new(config: &CitadelConfig) -> Self {
        Self {
            inbox_base: config.inbox_base_path.clone(),
            protected_base: config.protected_base_path.clone(),
            max_parallel: config.transfer_parallelism,
            chunk_size_mb: config.transfer_chunk_size_mb,
            config: config.clone(),
        }
    }
    
    /// Create inbox directory
    pub async fn create_inbox(&self, inbox_id: &ObjectId) -> Result<PathBuf> {
        let inbox_path = self.inbox_base.join(inbox_id.to_hex());
        
        tokio::fs::create_dir_all(&inbox_path)
            .await
            .map_err(|e| AppError::StorageError(format!("Failed to create inbox: {}", e)))?;
        
        info!("Created inbox directory: {:?}", inbox_path);
        Ok(inbox_path)
    }
    
    /// Transfer files from inbox to protected area
    pub async fn transfer_files(
        &self,
        inbox_id: &ObjectId,
        warehouse_path: &str,
    ) -> Result<TransferResult> {
        let inbox_path = self.inbox_base.join(inbox_id.to_hex());
        let protected_path = self.protected_base.join(warehouse_path.trim_start_matches('/'));
        
        // Use the transfer module for copy-verify-delete pattern
        let audit_context = crate::storage::transfer::AuditContext::new(Some(*inbox_id), None);
        match crate::storage::transfer::transfer_files(
            &inbox_path,
            &protected_path,
            &self.config,
            self.max_parallel,
            self.chunk_size_mb,
            audit_context,
        ).await {
            Ok(storage_result) => {
                Ok(TransferResult {
                    success: storage_result.success,
                    files_transferred: storage_result.files_transferred
                        .iter()
                        .map(|f| crate::models::FileInfo {
                            path: f.target_path.to_string_lossy().to_string(),
                            size: f.size,
                            checksum: f.checksum.clone(),
                        })
                        .collect(),
                    files_failed: storage_result.files_failed
                        .iter()
                        .map(|f| crate::models::FileInfo {
                            path: f.target_path.to_string_lossy().to_string(),
                            size: f.size,
                            checksum: f.checksum.clone(),
                        })
                        .collect(),
                    total_bytes: storage_result.total_bytes,
                    partial_cleanup_completed: storage_result.partial_cleanup_completed,
                })
            }
            Err(e) => {
                error!("Transfer failed: {}", e);
                // `TransferFailed` wraps the collected errors (and the audit
                // trail); surface the first underlying error in logs.
                if let TransferError::TransferFailed { errors, .. } = &e {
                    for err in errors {
                        error!("  - {}", err);
                    }
                }
                Ok(TransferResult {
                    success: false,
                    files_transferred: vec![],
                    files_failed: vec![],
                    total_bytes: 0,
                    partial_cleanup_completed: true,
                })
            }
        }
    }
    
    /// Delete inbox directory after successful transfer
    pub async fn delete_inbox(&self, inbox_id: &ObjectId) -> Result<()> {
        let inbox_path = self.inbox_base.join(inbox_id.to_hex());
        
        tokio::fs::remove_dir_all(&inbox_path)
            .await
            .map_err(|e| AppError::StorageError(format!("Failed to delete inbox: {}", e)))?;
        
        info!("Deleted inbox directory: {:?}", inbox_path);
        Ok(())
    }
    
    /// Validate path to prevent traversal attacks
    pub fn validate_path(&self, base: &Path, requested: &Path) -> Result<PathBuf> {
        let base_canonical = base.canonicalize()
            .map_err(|_| AppError::ValidationError("Invalid base path".to_string()))?;

        let full_path = base.join(requested);
        let canonical = if let Ok(c) = full_path.canonicalize() {
            c
        } else {
            // File doesn't exist yet — canonicalize parent, then re-append leaf
            let parent = full_path.parent().ok_or(AppError::ValidationError("Invalid base path".to_string()))?;
            let parent_canonical = parent.canonicalize()
                .map_err(|_| AppError::Forbidden("Path traversal attempt detected".to_string()))?;
            if !parent_canonical.starts_with(&base_canonical) {
                return Err(AppError::Forbidden("Path traversal attempt detected".to_string()));
            }
            // Re-append any remaining leaf components
            let leaf = full_path
                .strip_prefix(parent)
                .unwrap_or(&full_path);
            parent_canonical.join(leaf)
        };

        if !canonical.starts_with(&base_canonical) {
            return Err(AppError::Forbidden("Path traversal attempt detected".to_string()));
        }

        Ok(canonical)
    }
}