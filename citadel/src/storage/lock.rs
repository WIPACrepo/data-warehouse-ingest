//! File locking during transfer operations

use std::path::Path;
use crate::error::{AppError, Result};
use tracing::info;

/// Lock guard for inbox directory
pub struct InboxLock {
    pub path: std::path::PathBuf,
}

impl InboxLock {
    /// Lock inbox directory by setting read-only permissions
    pub async fn lock(path: &Path) -> Result<Self> {
        // Set directory to read-only (chmod 555)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = tokio::fs::metadata(path).await
                .map_err(|e| AppError::StorageError(format!("Failed to get metadata: {}", e)))?;
            
            let mut permissions = metadata.permissions();
            permissions.set_mode(0o555);
            
            tokio::fs::set_permissions(path, permissions).await
                .map_err(|e| AppError::StorageError(format!("Failed to set permissions: {}", e)))?;
            
            info!("Locked inbox directory: {:?}", path);
        }
        
        // TODO: Verify no files are currently open for write (fuser/lsof)
        // TODO: Implement grace period for open handles
        
        Ok(Self {
            path: path.to_path_buf(),
        })
    }
}

impl Drop for InboxLock {
    fn drop(&mut self) {
        // Unlock directory on drop
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = std::fs::metadata(&self.path) {
                let mut permissions = metadata.permissions();
                permissions.set_mode(0o755);
                let _ = std::fs::set_permissions(&self.path, permissions);
                info!("Unlocked inbox directory: {:?}", self.path);
            }
        }
    }
}