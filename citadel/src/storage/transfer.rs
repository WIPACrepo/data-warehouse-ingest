//! Recursive file transfer operations with copy-verify-delete pattern
//!
//! This module implements the core file transfer logic for citadel-job:
//! 1. Lock inbox directory (chmod 555)
//! 2. Recursively walk the inbox tree, following symlinks safely
//! 3. Detect symlink loops, external symlinks, and invalid targets
//! 4. Pre-create directories in the target area (producer phase)
//! 5. Copy files in parallel with configurable concurrency
//! 6. Verify each file with SHA-512 checksums after writing
//! 7. Commit atomically (temp dir + rename) when enabled
//! 8. Delete source files only after all verifications succeed
//!
//! Symlink handling:
//! - Symlinks to files are transferred as regular files, preserving the link's
//!   path relative to the inbox.
//! - Symlinks to directories are followed and recursed into.
//! - Symlink loops are detected using (dev, ino) active-chain tracking.
//! - External symlinks are rejected by default and only followed when
//!   `allow_external_symlinks` is enabled.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use rand::Rng;
use sha2::{Digest, Sha512};
use tokio::fs::{self, File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, Semaphore};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::config::{CitadelConfig, InboxConfig};
use crate::metrics::CitadelMetrics;

/// File transfer error types
#[derive(Debug, thiserror::Error)]
pub enum TransferError {
    #[error("Storage error: {0}")]
    StorageError(String),

    #[error("File transfer failed: {0}")]
    FileTransferFailed(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Symlink loop detected: {0}")]
    SymlinkLoop(PathBuf),

    #[error("External symlink not allowed: {path} points outside inbox")]
    ExternalSymlinkNotAllowed { path: PathBuf },

    #[error("Maximum recursion depth exceeded: {0}")]
    MaxRecursionDepthExceeded(u32),

    #[error("Maximum symlink chain depth exceeded: {0}")]
    MaxSymlinkDepthExceeded(u32),

    #[error("Broken symlink: {0}")]
    BrokenSymlink(PathBuf),

    #[error("Path exceeds maximum length: {0}")]
    PathTooLong(PathBuf),

    #[error("Multiple errors occurred during transfer: {0:?}")]
    MultipleErrors(Vec<TransferError>),

    #[error("Transfer cancelled: {0}")]
    Cancelled(String),

    #[error("Invalid symlink target: {0} contains forbidden characters")]
    InvalidSymlinkTarget(PathBuf),

    #[error("Symlink target resolution failed: {0}")]
    SymlinkResolutionFailed(PathBuf),

    #[error("Security violation: {0}")]
    SecurityViolation(String),

    #[error("Inbox integrity check failed: {0}")]
    InboxIntegrityFailed(String),

    #[error("Insufficient disk space: required {0} bytes, available {1} bytes")]
    InsufficientDiskSpace(u64, u64),

    #[error("Invalid warehouse path: {0} escapes protected base")]
    InvalidWarehousePath(PathBuf),

    #[error("Inbox lock failed: {0}")]
    InboxLockFailed(String),

    #[error("Maximum symlink references exceeded for inode {0}")]
    MaxSymlinkReferencesExceeded(u64),

    #[error("Maximum total files exceeded: {0}")]
    MaxTotalFilesExceeded(u64),

    #[error("Maximum total symlinks exceeded: {0}")]
    MaxTotalSymlinksExceeded(u32),
}

pub type Result<T> = std::result::Result<T, TransferError>;

/// Severity level for security audit events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuditSeverity {
    Info,
    Warning,
    Error,
    Critical,
}

/// Type of security-relevant event recorded during a transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecurityEventType {
    SymlinkLoopDetected,
    ExternalSymlinkRejected,
    MaxRecursionDepthExceeded,
    MaxSymlinkDepthExceeded,
    InvalidSymlinkTarget,
    InboxIntegrityCheckFailed,
    AtomicRollbackTriggered,
    InsufficientDiskSpace,
    WarehousePathValidationFailed,
    InboxLockFailed,
    MaxSymlinkReferencesExceeded,
    MaxTotalFilesExceeded,
    MaxTotalSymlinksExceeded,
}

/// Audit event emitted for security-relevant transfer incidents.
///
/// `inbox_id` and `job_id` are optional because the low-level transfer engine
/// is not always invoked with full job context. When available they are
/// populated by the caller via [`AuditContext`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityAuditEvent {
    pub timestamp: DateTime<Utc>,
    pub event_type: SecurityEventType,
    pub inbox_id: Option<ObjectId>,
    pub job_id: Option<String>,
    pub details: serde_json::Value,
    pub severity: AuditSeverity,
}

impl SecurityAuditEvent {
    pub fn new(
        event_type: SecurityEventType,
        severity: AuditSeverity,
        context: &AuditContext,
        details: serde_json::Value,
    ) -> Self {
        Self {
            timestamp: Utc::now(),
            event_type,
            inbox_id: context.inbox_id,
            job_id: context.job_id.clone(),
            details,
            severity,
        }
    }
}

/// Caller-provided context for audit logging.
#[derive(Debug, Clone, Default)]
pub struct AuditContext {
    pub inbox_id: Option<ObjectId>,
    pub job_id: Option<String>,
}

impl AuditContext {
    pub fn new(inbox_id: Option<ObjectId>, job_id: Option<String>) -> Self {
        Self { inbox_id, job_id }
    }
}

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
    pub audit_events: Vec<SecurityAuditEvent>,
    pub errors: Vec<TransferError>,
}

impl TransferResult {
    fn empty(success: bool) -> Self {
        Self {
            success,
            files_transferred: Vec::new(),
            files_failed: Vec::new(),
            total_bytes: 0,
            partial_cleanup_completed: true,
            audit_events: Vec::new(),
            errors: Vec::new(),
        }
    }
}

/// Coordinated cancellation and error counting.
pub struct TransferContext {
    cancel_token: CancellationToken,
    error_count: Arc<AtomicU32>,
    max_errors: u32,
}

impl TransferContext {
    pub fn new(max_errors: u32) -> Self {
        Self {
            cancel_token: CancellationToken::new(),
            error_count: Arc::new(AtomicU32::new(0)),
            max_errors,
        }
    }

    /// Record an error and trigger cancellation if the threshold is reached.
    /// Returns `true` if cancellation was triggered by this call.
    pub fn record_error(&self) -> bool {
        let count = self.error_count.fetch_add(1, Ordering::SeqCst);
        if count >= self.max_errors {
            self.cancel_token.cancel();
            true
        } else {
            false
        }
    }

    pub fn error_count(&self) -> u32 {
        self.error_count.load(Ordering::SeqCst)
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel_token.is_cancelled()
    }

    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel_token.clone()
    }
}

/// Shared counters for resource limits.
#[derive(Debug)]
struct TransferCounters {
    file_count: AtomicU64,
    symlink_count: AtomicU32,
    byte_count: AtomicU64,
}

impl TransferCounters {
    fn new() -> Self {
        Self {
            file_count: AtomicU64::new(0),
            symlink_count: AtomicU32::new(0),
            byte_count: AtomicU64::new(0),
        }
    }

    fn increment_file(&self) -> u64 {
        self.file_count.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn increment_symlink(&self) -> u32 {
        self.symlink_count.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn add_bytes(&self, bytes: u64) {
        self.byte_count.fetch_add(bytes, Ordering::SeqCst);
    }
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

            let metadata = fs::metadata(path)
                .await
                .map_err(|e| TransferError::InboxLockFailed(format!("Failed to get metadata: {}", e)))?;

            let original_mode = metadata.permissions().mode();
            let mut permissions = metadata.permissions();
            permissions.set_mode(0o555);

            fs::set_permissions(path, permissions)
                .await
                .map_err(|e| TransferError::InboxLockFailed(format!("Failed to set permissions: {}", e)))?;

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
    let mut file = File::open(path)
        .await
        .map_err(|e| TransferError::StorageError(format!("Failed to open file {:?}: {}", path, e)))?;

    let mut hasher = Sha512::new();
    let mut buffer = vec![0u8; chunk_size];

    loop {
        let bytes_read = file
            .read(&mut buffer)
            .await
            .map_err(|e| TransferError::StorageError(format!("Failed to read file: {}", e)))?;

        if bytes_read == 0 {
            break;
        }

        hasher.update(&buffer[..bytes_read]);
    }

    let checksum = hasher.finalize();
    Ok(hex::encode(checksum))
}

fn file_type_name(meta: &std::fs::Metadata) -> &'static str {
    if meta.is_dir() {
        "directory"
    } else if meta.is_file() {
        "file"
    } else if meta.file_type().is_symlink() {
        "symlink"
    } else {
        "other"
    }
}

#[cfg(unix)]
fn dev_ino(meta: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (meta.dev(), meta.ino())
}

/// Ensure a directory exists, creating it idempotently.
async fn ensure_dir_exists(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .await
        .map_err(|e| TransferError::StorageError(format!("Failed to create directory {:?}: {}", path, e)))
}

/// Validate a symlink target for dangerous characters or traversal attempts.
fn validate_symlink_target(target: &Path) -> Result<()> {
    let bytes = target.as_os_str().as_encoded_bytes();

    // Reject null bytes and ASCII control characters.
    for &b in bytes {
        if b == 0 || (b < 32 && b != b'\t') {
            return Err(TransferError::InvalidSymlinkTarget(target.to_path_buf()));
        }
    }

    // Reject any path component that is `..`.
    for component in target.components() {
        if let std::path::Component::ParentDir = component {
            return Err(TransferError::InvalidSymlinkTarget(target.to_path_buf()));
        }
    }

    Ok(())
}

/// Resolve a symlink chain manually, enforcing a depth limit and validating
/// each intermediate target. Returns the canonical final path, its metadata,
/// whether it is external to the inbox, and the number of links traversed.
async fn resolve_symlink_chain(
    link_path: &Path,
    inbox_root: &Path,
    max_depth: Option<u32>,
) -> Result<(PathBuf, std::fs::Metadata, bool, u32)> {
    let mut current = link_path.to_path_buf();
    let max = max_depth.unwrap_or(u32::MAX);
    let mut depth: u32 = 0;

    loop {
        if depth >= max {
            return Err(TransferError::MaxSymlinkDepthExceeded(max));
        }

        let target = tokio::fs::read_link(&current).await.map_err(|e| {
            if e.kind() == ErrorKind::InvalidInput {
                TransferError::BrokenSymlink(link_path.to_path_buf())
            } else {
                TransferError::SymlinkResolutionFailed(current.clone())
            }
        })?;

        validate_symlink_target(&target)?;
        depth += 1;

        let resolved = if target.is_absolute() {
            target
        } else {
            current
                .parent()
                .unwrap_or_else(|| Path::new("/"))
                .join(target)
        };

        // Inspect the resolved path without following further symlinks.
        let meta = tokio::fs::symlink_metadata(&resolved).await.map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                TransferError::BrokenSymlink(link_path.to_path_buf())
            } else {
                TransferError::SymlinkResolutionFailed(resolved.clone())
            }
        })?;

        if meta.file_type().is_symlink() {
            current = resolved;
            continue;
        }

        // Final target: canonicalize for robust external detection.
        let canonical = tokio::fs::canonicalize(&resolved).await.map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                TransferError::BrokenSymlink(link_path.to_path_buf())
            } else {
                TransferError::SymlinkResolutionFailed(resolved.clone())
            }
        })?;

        let is_external = is_external_to_inbox(&canonical, inbox_root);
        return Ok((canonical, meta, is_external, depth));
    }
}

/// Determine whether a canonical path is outside the inbox tree.
fn is_external_to_inbox(canonical: &Path, inbox_root: &Path) -> bool {
    !canonical.starts_with(inbox_root)
}

/// Count a reference to a symlink target inode and enforce symlink-farm limits.
fn count_symlink_reference(
    global_inodes: &mut HashMap<(u64, u64), u32>,
    dev: u64,
    ino: u64,
    max_references: Option<u32>,
) -> Result<u32> {
    let count = global_inodes.entry((dev, ino)).or_insert(0);
    *count += 1;
    if let Some(max) = max_references {
        if *count > max {
            return Err(TransferError::MaxSymlinkReferencesExceeded(ino));
        }
    }
    Ok(*count)
}

/// Send an error through the channel and record it in the transfer context.
async fn emit_error(
    tx: &mpsc::Sender<Result<(PathBuf, PathBuf), TransferError>>,
    err: TransferError,
    context: &TransferContext,
) {
    context.record_error();
    let _ = tx.send(Err(err)).await;
}

/// Worker task: copy a single file and verify its checksum.
async fn copy_file_worker(
    source: PathBuf,
    target: PathBuf,
    chunk_size: usize,
    context: Arc<TransferContext>,
    counters: Arc<TransferCounters>,
    metrics: Option<&CitadelMetrics>,
) -> Result<FileInfo> {
    if context.is_cancelled() {
        return Err(TransferError::Cancelled("worker cancelled before copy".to_string()));
    }

    // Open source file.
    let mut source_file = File::open(&source)
        .await
        .map_err(|e| TransferError::StorageError(format!("Failed to open source file {:?}: {}", source, e)))?;

    // Create a temporary file for atomic write.
    let temp_target = target.with_extension(format!("tmp.{}", std::process::id()));

    let mut target_file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temp_target)
        .await
        .map_err(|e| TransferError::StorageError(format!("Failed to create target file {:?}: {}", temp_target, e)))?;

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

    // Verify checksum of the on-disk target file.
    let actual_checksum = compute_checksum(&temp_target, chunk_size).await?;
    if expected_checksum != actual_checksum {
        let _ = fs::remove_file(&temp_target).await;
        return Err(TransferError::FileTransferFailed(format!(
            "Checksum mismatch for {:?}: expected {}, got {}",
            source, expected_checksum, actual_checksum
        )));
    }

    // Atomic rename to final target.
    fs::rename(&temp_target, &target).await.map_err(|e| {
        TransferError::StorageError(format!(
            "Failed to rename temp file {:?} to {:?}: {}",
            temp_target, target, e
        ))
    })?;

    counters.add_bytes(total_bytes);
    if let Some(m) = metrics {
        m.transfer_files_total.inc();
        m.transfer_bytes_total.inc_by(total_bytes as f64);
    }

    info!("Successfully transferred {:?} -> {:?} ({} bytes)", source, target, total_bytes);

    Ok(FileInfo {
        source_path: source,
        target_path: target,
        size: total_bytes,
        checksum: expected_checksum,
    })
}

/// Directory tree collector that feeds the worker pipeline.
struct Collector<'a> {
    target_base: &'a Path,
    inbox_root: &'a Path,
    inbox_root_id: (u64, u64),
    config: &'a InboxConfig,
    tx: &'a mpsc::Sender<Result<(PathBuf, PathBuf), TransferError>>,
    context: &'a TransferContext,
    counters: &'a TransferCounters,
    metrics: Option<&'a CitadelMetrics>,
    audit_context: &'a AuditContext,
    audit_events: &'a mut Vec<SecurityAuditEvent>,
}

impl<'a> Collector<'a> {
    fn collect<'b>(
        &'b self,
        source_dir: &'b Path,
        rel_prefix: &'b Path,
        active_chain: &'b mut HashSet<(u64, u64)>,
        global_inodes: &'b mut HashMap<(u64, u64), u32>,
        current_depth: u32,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'b>> {
        Box::pin(async move {
            if self.context.is_cancelled() {
                return Err(TransferError::Cancelled("collector cancelled".to_string()));
            }

            if current_depth > self.config.max_recursion_depth {
                let err = TransferError::MaxRecursionDepthExceeded(self.config.max_recursion_depth);
                self.audit_events.push(SecurityAuditEvent::new(
                    SecurityEventType::MaxRecursionDepthExceeded,
                    AuditSeverity::Warning,
                    self.audit_context,
                    serde_json::json!({ "depth": current_depth }),
                ));
                return Err(err);
            }

            // Resolve the canonical identity of the directory we are about to enter.
            let canonical_dir = fs::canonicalize(source_dir).await.map_err(|e| {
                TransferError::StorageError(format!("Failed to canonicalize {:?}: {}", source_dir, e))
            })?;
            let dir_meta = tokio::fs::symlink_metadata(&canonical_dir).await?;
            let dir_id = dev_ino(&dir_meta);

            if active_chain.contains(&dir_id) {
                self.audit_events.push(SecurityAuditEvent::new(
                    SecurityEventType::SymlinkLoopDetected,
                    AuditSeverity::Error,
                    self.audit_context,
                    serde_json::json!({
                        "path": canonical_dir.to_string_lossy(),
                        "dev": dir_id.0,
                        "ino": dir_id.1,
                    }),
                ));
                if let Some(m) = self.metrics {
                    m.transfer_errors_total.with_label_values(&["symlink_loop"]).inc();
                    m.security_events_total.with_label_values(&["symlink_loop_detected"]).inc();
                }
                return Err(TransferError::SymlinkLoop(canonical_dir));
            }

            active_chain.insert(dir_id);

            let result = self.collect_entries(&canonical_dir, rel_prefix, active_chain, global_inodes, current_depth).await;

            active_chain.remove(&dir_id);
            result
        })
    }

    async fn collect_entries(
        &self,
        source_dir: &Path,
        rel_prefix: &Path,
        active_chain: &mut HashSet<(u64, u64)>,
        global_inodes: &mut HashMap<(u64, u64), u32>,
        _current_depth: u32,
    ) -> Result<()> {
        let mut entries = fs::read_dir(source_dir).await.map_err(|e| {
            TransferError::StorageError(format!("Failed to read directory {:?}: {}", source_dir, e))
        })?;

        while let Some(entry) = entries.next_entry().await? {
            if self.context.is_cancelled() {
                return Err(TransferError::Cancelled("collector cancelled during enumeration".to_string()));
            }

            let entry_path = entry.path();
            let file_name = entry.file_name();
            let rel_path = rel_prefix.join(&file_name);
            let target_path = self.target_base.join(&rel_path);

            // Path length check.
            let max_len = self.config.max_path_length.unwrap_or(4096);
            if target_path.as_os_str().len() > max_len {
                emit_error(
                    self.tx,
                    TransferError::PathTooLong(target_path.clone()),
                    self.context,
                )
                .await;
                if let Some(m) = self.metrics {
                    m.transfer_errors_total.with_label_values(&["path_too_long"]).inc();
                }
                continue;
            }

            let meta = match entry.symlink_metadata().await {
                Ok(m) => m,
                Err(e) => {
                    emit_error(self.tx, TransferError::IoError(e), self.context).await;
                    continue;
                }
            };

            if meta.file_type().is_symlink() {
                let symlink_count = self.counters.increment_symlink();
                if let Some(max) = self.config.max_total_symlinks {
                    if symlink_count > max {
                        let err = TransferError::MaxTotalSymlinksExceeded(max);
                        self.audit_events.push(SecurityAuditEvent::new(
                            SecurityEventType::MaxTotalSymlinksExceeded,
                            AuditSeverity::Error,
                            self.audit_context,
                            serde_json::json!({ "count": symlink_count }),
                        ));
                        if let Some(m) = self.metrics {
                            m.transfer_errors_total.with_label_values(&["max_total_symlinks_exceeded"]).inc();
                        }
                        return Err(err);
                    }
                }

                let (resolved, resolved_meta, is_external, _depth) =
                    match resolve_symlink_chain(&entry_path, self.inbox_root, self.config.max_symlink_depth).await {
                        Ok(r) => r,
                        Err(e) => {
                            let label = match &e {
                                TransferError::BrokenSymlink(_) => "broken_symlink",
                                TransferError::InvalidSymlinkTarget(_) => "invalid_symlink_target",
                                TransferError::MaxSymlinkDepthExceeded(_) => "max_symlink_depth_exceeded",
                                TransferError::SymlinkLoop(_) => "symlink_loop",
                                _ => "symlink_resolution_failed",
                            };
                            if matches!(e, TransferError::SymlinkLoop(_)) {
                                self.audit_events.push(SecurityAuditEvent::new(
                                    SecurityEventType::SymlinkLoopDetected,
                                    AuditSeverity::Error,
                                    self.audit_context,
                                    serde_json::json!({ "path": entry_path.to_string_lossy() }),
                                ));
                            }
                            emit_error(self.tx, e, self.context).await;
                            if let Some(m) = self.metrics {
                                m.transfer_errors_total.with_label_values(&[label]).inc();
                            }
                            continue;
                        }
                    };

                // Symlink farm / repeated inode reference check.
                let resolved_id = dev_ino(&resolved_meta);
                if let Err(e) = count_symlink_reference(
                    global_inodes,
                    resolved_id.0,
                    resolved_id.1,
                    self.config.max_symlink_references,
                ) {
                    warn!(
                        "Symlink reference limit exceeded for inode {} at {:?}; skipping",
                        resolved_id.1, entry_path
                    );
                    self.audit_events.push(SecurityAuditEvent::new(
                        SecurityEventType::MaxSymlinkReferencesExceeded,
                        AuditSeverity::Warning,
                        self.audit_context,
                        serde_json::json!({ "ino": resolved_id.1 }),
                    ));
                    if let Some(m) = self.metrics {
                        m.transfer_errors_total.with_label_values(&["max_symlink_references_exceeded"]).inc();
                        m.security_events_total.with_label_values(&["max_symlink_references_exceeded"]).inc();
                    }
                    emit_error(self.tx, e, self.context).await;
                    continue;
                }

                // External symlink validation.
                let external_by_device = resolved_id.0 != self.inbox_root_id.0 || resolved_id.1 != self.inbox_root_id.1;
                if is_external || external_by_device {
                    if !self.config.allow_external_symlinks {
                        warn!("Rejecting external symlink {:?} -> {:?}", entry_path, resolved);
                        self.audit_events.push(SecurityAuditEvent::new(
                            SecurityEventType::ExternalSymlinkRejected,
                            AuditSeverity::Warning,
                            self.audit_context,
                            serde_json::json!({
                                "symlink_path": entry_path.to_string_lossy(),
                                "resolved_target": resolved.to_string_lossy(),
                            }),
                        ));
                        if let Some(m) = self.metrics {
                            m.transfer_errors_total.with_label_values(&["external_symlink_not_allowed"]).inc();
                            m.security_events_total.with_label_values(&["external_symlink_rejected"]).inc();
                        }
                        emit_error(
                            self.tx,
                            TransferError::ExternalSymlinkNotAllowed { path: entry_path },
                            self.context,
                        )
                        .await;
                        continue;
                    }
                    debug!("Following external symlink {:?} -> {:?}", entry_path, resolved);
                }

                if resolved_meta.is_dir() {
                    // Recurse into the resolved directory, preserving the symlink's path.
                    if let Err(e) = self
                        .collect(&resolved, &rel_path, active_chain, global_inodes, _current_depth + 1)
                        .await
                    {
                        return Err(e);
                    }
                } else if resolved_meta.is_file() {
                    let file_count = self.counters.increment_file();
                    if let Some(max) = self.config.max_total_files {
                        if file_count > max {
                            let err = TransferError::MaxTotalFilesExceeded(max);
                            self.audit_events.push(SecurityAuditEvent::new(
                                SecurityEventType::MaxTotalFilesExceeded,
                                AuditSeverity::Error,
                                self.audit_context,
                                serde_json::json!({ "count": file_count }),
                            ));
                            if let Some(m) = self.metrics {
                                m.transfer_errors_total.with_label_values(&["max_total_files_exceeded"]).inc();
                            }
                            return Err(err);
                        }
                    }

                    if let Some(parent) = target_path.parent() {
                        ensure_dir_exists(parent).await?;
                    }
                    self.tx
                        .send(Ok((resolved, target_path)))
                        .await
                        .map_err(|_| TransferError::Cancelled("channel closed".to_string()))?;
                } else {
                    warn!("Skipping unsupported symlink target type for {:?}", entry_path);
                }
            } else if meta.is_dir() {
                // Recurse into regular directory.
                if let Err(e) = self
                    .collect(&entry_path, &rel_path, active_chain, global_inodes, _current_depth + 1)
                    .await
                {
                    return Err(e);
                }
            } else if meta.is_file() {
                let file_count = self.counters.increment_file();
                if let Some(max) = self.config.max_total_files {
                    if file_count > max {
                        let err = TransferError::MaxTotalFilesExceeded(max);
                        self.audit_events.push(SecurityAuditEvent::new(
                            SecurityEventType::MaxTotalFilesExceeded,
                            AuditSeverity::Error,
                            self.audit_context,
                            serde_json::json!({ "count": file_count }),
                        ));
                        if let Some(m) = self.metrics {
                            m.transfer_errors_total.with_label_values(&["max_total_files_exceeded"]).inc();
                        }
                        return Err(err);
                    }
                }

                if let Some(parent) = target_path.parent() {
                    ensure_dir_exists(parent).await?;
                }
                self.tx
                    .send(Ok((entry_path, target_path)))
                    .await
                    .map_err(|_| TransferError::Cancelled("channel closed".to_string()))?;
            } else {
                warn!("Skipping unsupported filesystem entry {:?}", entry_path);
            }
        }

        Ok(())
    }
}

/// Validate that the warehouse path is contained within the protected base.
async fn validate_warehouse_path(warehouse_path: &Path, protected_base: &Path) -> Result<PathBuf> {
    let protected_canonical = fs::canonicalize(protected_base)
        .await
        .map_err(|e| TransferError::InvalidWarehousePath(protected_base.to_path_buf()))?;

    let warehouse_canonical = if warehouse_path.exists() {
        fs::canonicalize(warehouse_path).await
    } else {
        // Canonicalize the parent and append the leaf.
        let parent = warehouse_path.parent().unwrap_or_else(|| Path::new("/"));
        let leaf = warehouse_path.file_name().unwrap_or_default();
        fs::canonicalize(parent).await.map(|p| p.join(leaf))
    }
    .map_err(|e| TransferError::InvalidWarehousePath(warehouse_path.to_path_buf()))?;

    if !warehouse_canonical.starts_with(&protected_canonical) {
        return Err(TransferError::InvalidWarehousePath(warehouse_canonical));
    }

    Ok(warehouse_canonical)
}

/// Check available disk space on the warehouse filesystem.
#[cfg(unix)]
fn check_disk_space(target_dir: &Path, required_bytes: u64) -> Result<u64> {
    use nix::sys::statvfs::statvfs;

    let stat = statvfs(target_dir).map_err(|e| {
        TransferError::StorageError(format!("Failed to query filesystem space for {:?}: {}", target_dir, e))
    })?;

    let available = stat.blocks_available() as u64 * stat.fragment_size() as u64;
    Ok(available)
}

#[cfg(not(unix))]
fn check_disk_space(_target_dir: &Path, _required_bytes: u64) -> Result<u64> {
    // Skip disk-space check on non-Unix platforms.
    Ok(u64::MAX)
}

/// Pre-transfer integrity scan: read-only walk that validates symlinks, counts
/// files, and sums their sizes before any data is copied.
async fn pre_transfer_integrity_scan(
    inbox_path: &Path,
    config: &InboxConfig,
    inbox_root: &Path,
) -> Result<(u64, u64)> {
    let mut total_size: u64 = 0;
    let mut file_count: u64 = 0;
    let mut symlink_count: u32 = 0;

    let mut stack: Vec<PathBuf> = vec![inbox_path.to_path_buf()];
    let mut visited: HashSet<(u64, u64)> = HashSet::new();

    while let Some(dir) = stack.pop() {
        let canonical = fs::canonicalize(&dir).await.map_err(|e| {
            TransferError::InboxIntegrityFailed(format!("Failed to canonicalize {:?}: {}", dir, e))
        })?;
        let meta = tokio::fs::symlink_metadata(&canonical).await?;
        let id = dev_ino(&meta);
        if !visited.insert(id) {
            continue;
        }

        if file_count >= config.max_total_files.unwrap_or(u64::MAX) {
            return Err(TransferError::MaxTotalFilesExceeded(config.max_total_files.unwrap_or(0)));
        }
        if symlink_count >= config.max_total_symlinks.unwrap_or(u32::MAX) {
            return Err(TransferError::MaxTotalSymlinksExceeded(config.max_total_symlinks.unwrap_or(0)));
        }

        let mut entries = fs::read_dir(&canonical).await.map_err(|e| {
            TransferError::InboxIntegrityFailed(format!("Failed to read directory {:?}: {}", canonical, e))
        })?;

        while let Some(entry) = entries.next_entry().await? {
            let entry_path = entry.path();
            let entry_meta = entry.symlink_metadata().await?;

            if entry_meta.file_type().is_symlink() {
                symlink_count += 1;
                match resolve_symlink_chain(&entry_path, inbox_root, config.max_symlink_depth).await {
                    Ok((resolved, resolved_meta, is_external, _depth)) => {
                        if is_external && !config.allow_external_symlinks {
                            return Err(TransferError::InboxIntegrityFailed(format!(
                                "External symlink not allowed: {:?}",
                                entry_path
                            )));
                        }
                        if resolved_meta.is_dir() {
                            stack.push(resolved);
                        } else if resolved_meta.is_file() {
                            total_size += resolved_meta.len();
                            file_count += 1;
                        }
                    }
                    Err(TransferError::BrokenSymlink(_)) => {
                        if config.max_errors_before_fail > 0 {
                            return Err(TransferError::InboxIntegrityFailed(format!(
                                "Broken symlink: {:?}",
                                entry_path
                            )));
                        }
                    }
                    Err(e) => return Err(TransferError::InboxIntegrityFailed(format!("{:?}: {}", entry_path, e))),
                }
            } else if entry_meta.is_dir() {
                stack.push(entry_path);
            } else if entry_meta.is_file() {
                total_size += entry_meta.len();
                file_count += 1;
            }
        }
    }

    Ok((total_size, file_count))
}

/// Compute a unique temporary directory name next to the warehouse path.
fn temp_transfer_dir_name(warehouse_path: &Path) -> PathBuf {
    let timestamp = Utc::now().timestamp_millis();
    let random: u32 = rand::rng().random();
    let name = format!(
        "{}.transfer-{}-{}",
        warehouse_path.file_name().unwrap_or_default().to_string_lossy(),
        timestamp,
        random
    );
    warehouse_path.parent().unwrap_or_else(|| Path::new(".")).join(name)
}

/// Roll back a list of transferred files in non-atomic mode.
async fn rollback_files(files: &[PathBuf]) {
    for path in files {
        if let Err(e) = fs::remove_file(path).await {
            warn!("Failed to remove rolled-back file {:?}: {}", path, e);
        }
    }
}

/// Delete source files and directories after a successful transfer.
async fn cleanup_source(inbox_path: &Path, transferred_files: &[FileInfo]) -> Result<()> {
    // Delete individual source files first.
    for file in transferred_files {
        if let Err(e) = fs::remove_file(&file.source_path).await {
            warn!("Failed to delete source file {:?}: {}", file.source_path, e);
        }
    }

    // Remove any remaining empty directories.
    if let Err(e) = fs::remove_dir_all(inbox_path).await {
        warn!("Failed to remove inbox directory {:?}: {}", inbox_path, e);
    }

    Ok(())
}

/// Transfer all files from inbox to protected area.
pub async fn transfer_files(
    inbox_path: &Path,
    warehouse_path: &Path,
    config: &CitadelConfig,
    parallelism: usize,
    chunk_size_mb: usize,
    audit_context: AuditContext,
) -> Result<TransferResult> {
    let metrics_ref: Option<&CitadelMetrics> = None; // Metrics are injected by callers if available.
    transfer_files_with_metrics(inbox_path, warehouse_path, config, parallelism, chunk_size_mb, audit_context, metrics_ref).await
}

/// Internal variant that accepts an optional metrics reference.
pub async fn transfer_files_with_metrics(
    inbox_path: &Path,
    warehouse_path: &Path,
    config: &CitadelConfig,
    parallelism: usize,
    chunk_size_mb: usize,
    audit_context: AuditContext,
    metrics: Option<&CitadelMetrics>,
) -> Result<TransferResult> {
    info!("Starting file transfer from {:?} to {:?}", inbox_path, warehouse_path);

    // Step 1: Validate warehouse path.
    let warehouse_canonical = validate_warehouse_path(warehouse_path, &config.protected_base_path).await?;

    // Step 2: Lock inbox directory.
    let lock = InboxLock::lock(inbox_path).await?;

    // Step 3: Resolve canonical inbox root and its identity.
    let inbox_canonical = fs::canonicalize(inbox_path).await.map_err(|e| {
        TransferError::StorageError(format!("Failed to canonicalize inbox {:?}: {}", inbox_path, e))
    })?;
    let inbox_meta = tokio::fs::symlink_metadata(&inbox_canonical).await?;
    let inbox_root_id = dev_ino(&inbox_meta);

    // Step 4: Pre-transfer integrity scan (optional).
    let (required_bytes, _file_count) = if config.inbox.pre_transfer_integrity_scan {
        match pre_transfer_integrity_scan(&inbox_canonical, &config.inbox, &inbox_canonical).await {
            Ok(size_count) => size_count,
            Err(e) => {
                let mut result = TransferResult::empty(false);
                result.errors.push(e.clone());
                result.audit_events.push(SecurityAuditEvent::new(
                    SecurityEventType::InboxIntegrityCheckFailed,
                    AuditSeverity::Error,
                    &audit_context,
                    serde_json::json!({ "reason": e.to_string() }),
                ));
                if let Some(m) = metrics {
                    m.transfer_errors_total.with_label_values(&["inbox_integrity_failed"]).inc();
                    m.security_events_total.with_label_values(&["inbox_integrity_check_failed"]).inc();
                }
                drop(lock);
                return Err(e);
            }
        }
    } else {
        (0, 0)
    };

    // Step 5: Disk space check with 10% buffer.
    if required_bytes > 0 {
        let required_with_buffer = (required_bytes as f64 * 1.10).ceil() as u64;
        let target_dir = warehouse_canonical.parent().unwrap_or_else(|| Path::new("/"));
        let available = check_disk_space(target_dir, required_with_buffer)?;
        if available < required_with_buffer {
            let event = SecurityAuditEvent::new(
                SecurityEventType::InsufficientDiskSpace,
                AuditSeverity::Error,
                &audit_context,
                serde_json::json!({
                    "required": required_with_buffer,
                    "available": available,
                }),
            );
            if let Some(m) = metrics {
                m.transfer_errors_total.with_label_values(&["insufficient_disk_space"]).inc();
                m.security_events_total.with_label_values(&["insufficient_disk_space"]).inc();
            }
            drop(lock);
            return Err(TransferError::InsufficientDiskSpace(required_with_buffer, available));
        }
    }

    // Step 6: Determine target base (atomic temp dir or direct warehouse path).
    let (target_base, temp_dir) = if config.inbox.atomic_transfer {
        let temp = temp_transfer_dir_name(&warehouse_canonical);
        fs::create_dir_all(&temp).await.map_err(|e| {
            TransferError::StorageError(format!("Failed to create temp transfer directory {:?}: {}", temp, e))
        })?;
        (temp.clone(), Some(temp))
    } else {
        (warehouse_canonical.clone(), None)
    };

    // Step 7: Set up producer-consumer pipeline.
    let channel_capacity = (parallelism * 2).max(1);
    let (tx, mut rx) = mpsc::channel::<Result<(PathBuf, PathBuf), TransferError>>(channel_capacity);

    let context = TransferContext::new(config.inbox.max_errors_before_fail.max(1));
    let counters = Arc::new(TransferCounters::new());
    let chunk_size = chunk_size_mb * 1024 * 1024;

    let mut active_chain: HashSet<(u64, u64)> = HashSet::new();
    active_chain.insert(inbox_root_id);
    let mut global_inodes: HashMap<(u64, u64), u32> = HashMap::new();
    let mut audit_events: Vec<SecurityAuditEvent> = Vec::new();

    let collector = Collector {
        target_base: &target_base,
        inbox_root: &inbox_canonical,
        inbox_root_id,
        config: &config.inbox,
        tx: &tx,
        context: &context,
        counters: &counters,
        metrics,
        audit_context: &audit_context,
        audit_events: &mut audit_events,
    };

    // Producer task.
    let producer_handle = {
        let inbox = inbox_canonical.clone();
        tokio::spawn(async move {
            let mut chain = active_chain;
            let mut inodes = global_inodes;
            collector.collect(&inbox, Path::new(""), &mut chain, &mut inodes, 0).await
        })
    };

    // Worker tasks.
    let semaphore = Arc::new(Semaphore::new(parallelism));
    let mut join_set = JoinSet::new();
    let context_arc = Arc::new(context);

    let worker_loop = {
        let context = context_arc.clone();
        let counters = counters.clone();
        async move {
            let mut manifest: Vec<PathBuf> = Vec::new();
            while let Some(item) = rx.recv().await {
                if context.is_cancelled() {
                    break;
                }
                match item {
                    Ok((source, target)) => {
                        let _permit = semaphore.clone().acquire_owned().await.unwrap();
                        let ctx = context.clone();
                        let cnt = counters.clone();
                        join_set.spawn(async move {
                            copy_file_worker(source, target.clone(), chunk_size, ctx, cnt, metrics).await
                        });
                        manifest.push(target);
                    }
                    Err(e) => {
                        context.record_error();
                        // Errors are collected below via the context counter.
                        let _ = e;
                    }
                }
            }
            manifest
        }
    };

    // Drive the worker loop and producer to completion.
    let manifest = worker_loop.await;

    // Wait for all in-flight copies to finish.
    let mut transferred: Vec<FileInfo> = Vec::new();
    let mut failed: Vec<FileInfo> = Vec::new();
    while let Some(res) = join_set.join_next().await {
        match res {
            Ok(Ok(info)) => {
                transferred.push(info);
            }
            Ok(Err(e)) => {
                error!("File transfer failed: {}", e);
                if context_arc.record_error() {
                    // Cancellation already triggered.
                }
                if let Some(m) = metrics {
                    m.transfer_errors_total.with_label_values(&["file_transfer_failed"]).inc();
                }
            }
            Err(e) => {
                error!("Worker join error: {}", e);
                context_arc.record_error();
            }
        }
    }

    // Wait for producer to finish.
    let producer_result = producer_handle.await.map_err(|e| {
        TransferError::StorageError(format!("Producer task failed: {}", e))
    })?;

    // Collect any errors from the channel that arrived after workers drained.
    while let Ok(Some(item)) = tokio::time::timeout(std::time::Duration::from_millis(10), rx.recv()).await {
        if let Err(e) = item {
            context_arc.record_error();
            let _ = e;
        }
    }

    let success = !context_arc.is_cancelled() && producer_result.is_ok();

    if !success {
        // Rollback.
        if let Some(temp) = temp_dir {
            if let Err(e) = fs::remove_dir_all(&temp).await {
                warn!("Failed to remove temp transfer directory {:?}: {}", temp, e);
            }
            if let Some(m) = metrics {
                m.transfer_atomic_rollbacks_total.inc();
            }
            audit_events.push(SecurityAuditEvent::new(
                SecurityEventType::AtomicRollbackTriggered,
                AuditSeverity::Error,
                &audit_context,
                serde_json::json!({ "reason": "transfer failure", "temp_dir": temp.to_string_lossy() }),
            ));
        } else {
            rollback_files(&manifest).await;
        }

        drop(lock);

        let mut result = TransferResult::empty(false);
        result.files_failed = failed;
        result.audit_events = audit_events;
        result.partial_cleanup_completed = true;
        return Err(TransferError::MultipleErrors(vec![TransferError::FileTransferFailed(
            "one or more files failed to transfer".to_string(),
        )]));
    }

    // Step 8: Commit atomic transfer.
    if let Some(temp) = temp_dir {
        // Remove an existing empty warehouse directory if present so rename succeeds.
        if warehouse_canonical.exists() {
            let existing = fs::read_dir(&warehouse_canonical).await.map_err(|e| {
                TransferError::StorageError(format!("Failed to inspect existing warehouse path: {}", e))
            })?;
            let has_entries = existing.next_entry().await?.is_some();
            if has_entries {
                return Err(TransferError::InvalidWarehousePath(warehouse_canonical));
            }
            fs::remove_dir(&warehouse_canonical).await.map_err(|e| {
                TransferError::StorageError(format!("Failed to remove empty warehouse path: {}", e))
            })?;
        }

        fs::rename(&temp, &warehouse_canonical).await.map_err(|e| {
            TransferError::StorageError(format!(
                "Failed to commit atomic transfer from {:?} to {:?}: {}",
                temp, warehouse_canonical, e
            ))
        })?;
    }

    let total_bytes = counters.byte_count.load(Ordering::SeqCst);

    // Step 9: Cleanup source files and directories.
    drop(lock);
    cleanup_source(inbox_path, &transferred).await?;

    info!(
        "Successfully transferred {} files ({} bytes)",
        transferred.len(),
        total_bytes
    );

    let mut result = TransferResult::empty(true);
    result.files_transferred = transferred;
    result.total_bytes = total_bytes;
    result.audit_events = audit_events;
    Ok(result)
}

// Re-export serde for derive macros used inside this module.
use serde::{Deserialize, Serialize};
