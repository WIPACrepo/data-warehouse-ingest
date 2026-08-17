//! File transfer operations with copy-verify-delete pattern
//!
//! This module implements the core file transfer logic for citadel-job:
//! 1. Lock inbox directory (chmod 555)
//! 2. Copy all files in parallel with configurable concurrency
//! 3. Verify all files with SHA-512 checksums
//! 4. Delete source files only after all verifications succeed

use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs::{self, File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Semaphore;
use tracing::{info, error};
use sha2::{Digest, Sha512};

/// File transfer error types
#[derive(Debug, thiserror::Error)]
pub enum TransferError {
    #[error("Storage error: {0}")]
    StorageError(String),
    
    #[error("File transfer failed: {0}")]
    FileTransferFailed(String),
    
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, TransferError>;

/// File information for transfer tracking
#[derive(Debug, Clone)]
pub struct FileInfo {
    pub source_path: PathBuf,
    pub target_path: PathBuf,
    pub size: u64,
    pub checksum: String,
}

/// Transfer result
#[derive(Debug, Clone)]
pub struct TransferResult {
    pub success: bool,
    pub files_transferred: Vec<FileInfo>,
    pub files_failed: Vec<FileInfo>,
    pub total_bytes: u64,
    pub partial_cleanup_completed: bool,
}

/// Inbox lock guard - sets directory to read-only during transfer
pub struct InboxLock {
    pub path: PathBuf,
    #[cfg(unix)]
    pub original_mode: u32,
}

impl InboxLock {
    /// Lock inbox directory by setting read-only permissions (chmod 555)
    pub async fn lock(path: &Path) -> Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            
            let metadata = fs::metadata(path).await
                .map_err(|e| TransferError::StorageError(format!("Failed to get metadata: {}", e)))?;
            
            let original_mode = metadata.permissions().mode();
            let mut permissions = metadata.permissions();
            permissions.set_mode(0o555);
            
            fs::set_permissions(path, permissions).await
                .map_err(|e| TransferError::StorageError(format!("Failed to set permissions: {}", e)))?;
            
            info!("Locked inbox directory: {:?} (mode {:o} -> 555)", path, original_mode);
            
            Ok(Self {
                path: path.to_path_buf(),
                original_mode,
            })
        }
        
        #[cfg(not(unix))]
        {
            info!("Locked inbox directory: {:?} (non-Unix platform)", path);
            Ok(Self {
                path: path.to_path_buf(),
            })
        }
    }
}

impl Drop for InboxLock {
    fn drop(&mut self) {
        // Unlock on drop - restore original permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = std::fs::metadata(&self.path) {
                let mut permissions = metadata.permissions();
                permissions.set_mode(self.original_mode);
                let _ = std::fs::set_permissions(&self.path, permissions);
                info!("Unlocked inbox directory: {:?}", self.path);
            }
        }
    }
}

/// Compute SHA-512 checksum of a file in chunks
pub async fn compute_checksum(path: &Path, chunk_size: usize) -> Result<String> {
    let mut file = File::open(path).await
        .map_err(|e| TransferError::StorageError(format!("Failed to open file {:?}: {}", path, e)))?;
    
    let mut hasher = Sha512::new();
    let mut buffer = vec![0u8; chunk_size];
    
    loop {
        let bytes_read = file.read(&mut buffer).await
            .map_err(|e| TransferError::StorageError(format!("Failed to read file: {}", e)))?;
        
        if bytes_read == 0 {
            break;
        }
        
        hasher.update(&buffer[..bytes_read]);
    }
    
    let checksum = hasher.finalize();
    Ok(hex::encode(checksum))
}

/// Copy a single file from source to target with checksum verification
async fn copy_and_verify_file(
    source: &Path,
    target: &Path,
    chunk_size: usize,
) -> Result<FileInfo> {
    // Ensure target directory exists
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).await
            .map_err(|e| TransferError::StorageError(format!("Failed to create target directory: {}", e)))?;
    }
    
    // Create temporary file for atomic write
    let temp_target = target.with_extension(format!("tmp.{}", std::process::id()));
    
    // Open source file
    let mut source_file = File::open(source).await
        .map_err(|e| TransferError::StorageError(format!("Failed to open source file: {}", e)))?;
    
    // Open temp target file
    let mut target_file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temp_target)
        .await
        .map_err(|e| TransferError::StorageError(format!("Failed to create target file: {}", e)))?;
    
    // Copy with checksum computation
    let mut hasher = Sha512::new();
    let mut buffer = vec![0u8; chunk_size];
    let mut total_bytes = 0u64;
    
    loop {
        let bytes_read = source_file.read(&mut buffer).await?;
        if bytes_read == 0 {
            break;
        }
        
        hasher.update(&buffer[..bytes_read]);
        target_file.write_all(&buffer[..bytes_read]).await?;
        total_bytes += bytes_read as u64;
    }
    
    target_file.flush().await?;
    drop(target_file);
    
    let expected_checksum = hex::encode(hasher.finalize());
    
    // Verify by computing checksum of target
    let actual_checksum = compute_checksum(&temp_target, chunk_size).await?;
    
    if expected_checksum != actual_checksum {
        fs::remove_file(&temp_target).await.ok();
        return Err(TransferError::FileTransferFailed(format!(
            "Checksum mismatch for {:?}: expected {}, got {}",
            source, expected_checksum, actual_checksum
        )));
    }
    
    // Atomic rename
    fs::rename(&temp_target, target).await
        .map_err(|e| TransferError::StorageError(format!("Failed to rename temp file: {}", e)))?;
    
    info!("Successfully transferred {:?} -> {:?} ({} bytes)", source, target, total_bytes);
    
    Ok(FileInfo {
        source_path: source.to_path_buf(),
        target_path: target.to_path_buf(),
        size: total_bytes,
        checksum: expected_checksum,
    })
}

/// Transfer all files from inbox to protected area
pub async fn transfer_files(
    inbox_path: &Path,
    warehouse_path: &Path,
    parallelism: usize,
    chunk_size_mb: usize,
) -> Result<TransferResult> {
    info!("Starting file transfer from {:?} to {:?}", inbox_path, warehouse_path);
    
    // Step 1: Lock inbox directory (chmod 555)
    let _lock = InboxLock::lock(inbox_path).await?;
    
    // Step 2: Collect all files to transfer
    let mut files_to_transfer = Vec::new();
    
    let mut entries = fs::read_dir(inbox_path).await
        .map_err(|e| TransferError::StorageError(format!("Failed to read inbox directory: {}", e)))?;
    
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if let Ok(metadata) = entry.metadata().await {
            if metadata.is_file() {
                files_to_transfer.push(path);
            }
        }
    }
    
    if files_to_transfer.is_empty() {
        info!("No files to transfer in inbox {:?}", inbox_path);
        return Ok(TransferResult {
            success: true,
            files_transferred: vec![],
            files_failed: vec![],
            total_bytes: 0,
            partial_cleanup_completed: true,
        });
    }
    
    info!("Found {} files to transfer", files_to_transfer.len());
    
    // Step 3: Transfer files in parallel with semaphore limiting
    let semaphore = Arc::new(Semaphore::new(parallelism));
    let chunk_size = chunk_size_mb * 1024 * 1024;
    
    let mut handles = Vec::new();
    
    for source_path in files_to_transfer {
        let sem = semaphore.clone();
        let target_path = warehouse_path.join(source_path.file_name().unwrap());
        let source_path_clone = source_path.clone();
        
        let handle = tokio::spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            copy_and_verify_file(&source_path_clone, &target_path, chunk_size).await
        });
        
        handles.push((source_path, handle));
    }
    
    // Step 4: Collect results
    let mut transferred = Vec::new();
    let mut failed = Vec::new();
    
    for (original_source_path, handle) in handles {
        match handle.await {
            Ok(Ok(file_info)) => {
                transferred.push(file_info);
            }
            Ok(Err(e)) => {
                error!("Failed to transfer {:?}: {}", original_source_path, e);
                failed.push(FileInfo {
                    source_path: original_source_path.clone(),
                    target_path: warehouse_path.join(original_source_path.file_name().unwrap()),
                    size: 0,
                    checksum: String::new(),
                });
            }
            Err(_) => {
                error!("Task join error for {:?}", original_source_path);
                failed.push(FileInfo {
                    source_path: original_source_path.clone(),
                    target_path: warehouse_path.join(original_source_path.file_name().unwrap()),
                    size: 0,
                    checksum: String::new(),
                });
            }
        }
    }
    
    // Step 5: Check for failures
    if !failed.is_empty() {
        error!("{} files failed to transfer, cleaning up partial transfers", failed.len());
        // Clean up any partially transferred targets
        for file in &transferred {
            let _ = fs::remove_file(&file.target_path).await;
        }
        
        return Ok(TransferResult {
            success: false,
            files_transferred: vec![],
            files_failed: failed,
            total_bytes: 0,
            partial_cleanup_completed: true,
        });
    }
    
    let total_bytes: u64 = transferred.iter().map(|f| f.size).sum();
    
    // Step 6: Delete source files after successful verification
    for file in &transferred {
        fs::remove_file(&file.source_path).await
            .map_err(|e| TransferError::StorageError(format!("Failed to delete source file: {}", e)))?;
    }
    
    info!("Successfully transferred {} files, deleted sources", transferred.len());
    
    Ok(TransferResult {
        success: true,
        files_transferred: transferred,
        files_failed: vec![],
        total_bytes,
        partial_cleanup_completed: true,
    })
}