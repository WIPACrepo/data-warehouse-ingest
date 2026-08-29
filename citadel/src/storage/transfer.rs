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
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use rand::Rng;
use sha2::{Digest, Sha512};
use tokio::fs::{self, File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::config::{CitadelConfig, InboxConfig};
use crate::metrics::CitadelMetrics;
use crate::storage::progress::ProgressTracker;

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

    /// The transfer failed as a whole.
    ///
    /// `errors` holds every distinct error collected during the run (at
    /// least one); `audit_events` carries the security audit trail so
    /// callers can forward it to the SIEM/audit sink even on failure
    /// (plan §12).
    #[error(
        "Transfer failed with {} error(s): {}",
        errors.len(),
        errors.first().map(|e| e.to_string()).unwrap_or_default()
    )]
    TransferFailed {
        errors: Vec<TransferError>,
        audit_events: Vec<SecurityAuditEvent>,
    },

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
#[derive(Debug)]
pub struct TransferResult {
    pub success: bool,
    pub files_transferred: Vec<FileInfo>,
    pub files_failed: Vec<FileInfo>,
    pub total_bytes: u64,
    pub partial_cleanup_completed: bool,
    pub audit_events: Vec<SecurityAuditEvent>,
    /// Errors collected during the run.
    ///
    /// On failure these are the errors that caused (or accompanied) the
    /// failure — the returned [`TransferError`] wraps this list. On success
    /// these are non-fatal errors collected below the
    /// `max_errors_before_fail` threshold (e.g. broken symlinks that were
    /// skipped while the rest of the tree transferred).
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
        Self::with_external_token(max_errors, CancellationToken::new())
    }

    /// Create a transfer context whose cancellation is shared with an
    /// external token (e.g. citadel-job's metadata bail-out poll, P4.1).
    pub fn with_external_token(max_errors: u32, cancel_token: CancellationToken) -> Self {
        Self {
            cancel_token,
            error_count: Arc::new(AtomicU32::new(0)),
            max_errors,
        }
    }

    /// Record an error and trigger cancellation if the threshold is reached.
    /// Returns `true` if cancellation was triggered by this call.
    ///
    /// With `max_errors = N`, the transfer fails on the Nth error: errors
    /// 1..N-1 are collected and processing continues, the Nth triggers
    /// cancellation. (N is always >= 1.)
    pub fn record_error(&self) -> bool {
        let count = self.error_count.fetch_add(1, Ordering::SeqCst);
        if count + 1 >= self.max_errors {
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

            let metadata = fs::metadata(path).await.map_err(|e| {
                TransferError::InboxLockFailed(format!("Failed to get metadata: {}", e))
            })?;

            let original_mode = metadata.permissions().mode();
            let mut permissions = metadata.permissions();
            permissions.set_mode(0o555);

            fs::set_permissions(path, permissions).await.map_err(|e| {
                TransferError::InboxLockFailed(format!("Failed to set permissions: {}", e))
            })?;

            info!(
                "Locked inbox directory: {:?} (mode {:o} -> 555)",
                path, original_mode
            );

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
    let mut file = File::open(path).await.map_err(|e| {
        TransferError::StorageError(format!("Failed to open file {:?}: {}", path, e))
    })?;

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

#[cfg(unix)]
pub(crate) fn dev_ino(meta: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (meta.dev(), meta.ino())
}

/// Ensure a directory exists, creating it idempotently.
async fn ensure_dir_exists(path: &Path) -> Result<()> {
    fs::create_dir_all(path).await.map_err(|e| {
        TransferError::StorageError(format!("Failed to create directory {:?}: {}", path, e))
    })
}

/// Validate a symlink target for dangerous characters or traversal attempts.
pub(crate) fn validate_symlink_target(target: &Path) -> Result<()> {
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
/// Resolve a symlink chain manually, enforcing a depth limit, detecting
/// pure cycles, and validating each intermediate target. Returns the
/// canonical final path, the final target's metadata, and the number of
/// links traversed. External detection is performed by the caller via
/// [`is_external_to_inbox`] (path prefix) plus a device comparison against
/// the inbox root (bind-mount subtrees).
async fn resolve_symlink_chain(
    link_path: &Path,
    max_depth: Option<u32>,
) -> Result<(PathBuf, std::fs::Metadata, u32)> {
    let mut current = link_path.to_path_buf();
    let max = max_depth.unwrap_or(u32::MAX);
    let mut depth: u32 = 0;
    // (dev, ino) of every link followed so far — detects pure cycles
    // (e.g. a -> b -> a) which a depth limit alone would misreport as
    // MaxSymlinkDepthExceeded.
    let mut visited: HashSet<(u64, u64)> = HashSet::new();

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
            let link_id = dev_ino(&meta);
            if !visited.insert(link_id) {
                return Err(TransferError::SymlinkLoop(resolved));
            }
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

        return Ok((canonical, meta, depth));
    }
}

/// Determine whether a canonical path is outside the inbox tree.
pub(crate) fn is_external_to_inbox(canonical: &Path, inbox_root: &Path) -> bool {
    !canonical.starts_with(inbox_root)
}

/// Count a reference to a symlink target inode and enforce symlink-farm limits.
pub(crate) fn count_symlink_reference(
    global_inodes: &mut HashMap<(u64, u64), u32>,
    dev: u64,
    ino: u64,
    max_references: Option<u32>,
) -> Result<u32> {
    let count = global_inodes.entry((dev, ino)).or_insert(0);
    *count += 1;
    if let Some(max) = max_references
        && *count > max
    {
        return Err(TransferError::MaxSymlinkReferencesExceeded(ino));
    }
    Ok(*count)
}

/// Record an error against the transfer context and, if it is
/// sub-threshold, send it through the channel.
///
/// Returns `Some(err)` when this error reached the `max_errors_before_fail`
/// threshold (cancelling the transfer). The caller MUST then `return Err`
/// with that error so the pipeline unwinds immediately with the specific
/// failure. `None` means the error was collected non-fatally and forwarded
/// through the channel; the caller should `continue`.
async fn emit_error(
    tx: &mpsc::Sender<Result<(PathBuf, PathBuf)>>,
    err: TransferError,
    context: &TransferContext,
) -> Option<TransferError> {
    if context.record_error() {
        return Some(err);
    }
    let _ = tx.send(Err(err)).await;
    None
}

/// Worker task: copy a single file and verify its checksum.
///
/// Writes **directly to the final target** (truncate + write + flush) — there
/// is deliberately no per-file temp name that two workers could collide on
/// (D10). An incomplete transfer leaves a partial file at the final target,
/// which is an accepted risk: the run fails wholesale and either removes the
/// temp transfer dir (atomic mode) or rolls back the manifest (non-atomic
/// mode). On checksum mismatch the partial target file is removed before the
/// error is returned.
async fn copy_file_worker(
    source: PathBuf,
    target: PathBuf,
    chunk_size: usize,
    context: Arc<TransferContext>,
    counters: Arc<TransferCounters>,
    metrics: Option<Arc<CitadelMetrics>>,
    progress: Option<Arc<ProgressTracker>>,
) -> Result<FileInfo> {
    if context.is_cancelled() {
        return Err(TransferError::Cancelled(
            "worker cancelled before copy".to_string(),
        ));
    }

    // Open source file.
    let mut source_file = File::open(&source).await.map_err(|e| {
        TransferError::StorageError(format!("Failed to open source file {:?}: {}", source, e))
    })?;

    // Open the final target directly (truncate any prior partial content).
    let mut target_file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&target)
        .await
        .map_err(|e| {
            TransferError::StorageError(format!("Failed to create target file {:?}: {}", target, e))
        })?;

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

    // Verify checksum of the on-disk target file (the final path).
    let actual_checksum = compute_checksum(&target, chunk_size).await?;
    if expected_checksum != actual_checksum {
        // Remove the partial target so a corrupt file is never left behind.
        if let Err(e) = fs::remove_file(&target).await {
            warn!(
                "Failed to remove partial target {:?} after checksum mismatch: {}",
                target, e
            );
        }
        return Err(TransferError::FileTransferFailed(format!(
            "Checksum mismatch for {:?}: expected {}, got {}",
            source, expected_checksum, actual_checksum
        )));
    }

    counters.add_bytes(total_bytes);
    if let Some(m) = metrics.as_ref() {
        m.transfer_files_total.inc();
        m.transfer_bytes_total.inc_by(total_bytes as f64);
    }
    if let Some(p) = progress.as_ref() {
        p.increment_files(1, total_bytes).await;
    }

    info!(
        "Successfully transferred {:?} -> {:?} ({} bytes)",
        source, target, total_bytes
    );

    Ok(FileInfo {
        source_path: source,
        target_path: target,
        size: total_bytes,
        checksum: expected_checksum,
    })
}

/// Directory tree collector that feeds the worker pipeline.
///
/// The collector borrows local state from the transfer call. The producer
/// phase is driven inline (never `tokio::spawn`ed), so these borrows never
/// escape the enclosing function.
struct Collector<'a> {
    target_base: &'a Path,
    inbox_root: &'a Path,
    inbox_root_id: (u64, u64),
    config: &'a InboxConfig,
    tx: &'a mpsc::Sender<Result<(PathBuf, PathBuf)>>,
    context: &'a TransferContext,
    counters: &'a TransferCounters,
    metrics: Option<&'a CitadelMetrics>,
    audit_context: &'a AuditContext,
    audit_events: &'a mut Vec<SecurityAuditEvent>,
}

impl<'a> Collector<'a> {
    fn collect<'b>(
        &'b mut self,
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
                TransferError::StorageError(format!(
                    "Failed to canonicalize {:?}: {}",
                    source_dir, e
                ))
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
                    m.transfer_errors_total
                        .with_label_values(&["symlink_loop"])
                        .inc();
                    m.security_events_total
                        .with_label_values(&["symlink_loop_detected"])
                        .inc();
                }
                return Err(TransferError::SymlinkLoop(canonical_dir));
            }

            active_chain.insert(dir_id);

            // Pre-create this directory in the warehouse so the structure
            // exists even for empty subdirectories (and so workers only
            // need to write files into directories that are already present).
            let target_dir = if rel_prefix.as_os_str().is_empty() {
                self.target_base.to_path_buf()
            } else {
                self.target_base.join(rel_prefix)
            };
            if let Err(e) = ensure_dir_exists(&target_dir).await {
                active_chain.remove(&dir_id);
                return Err(e);
            }

            let result = self
                .collect_entries(
                    &canonical_dir,
                    rel_prefix,
                    active_chain,
                    global_inodes,
                    current_depth,
                )
                .await;

            active_chain.remove(&dir_id);
            result
        })
    }

    async fn collect_entries(
        &mut self,
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
                return Err(TransferError::Cancelled(
                    "collector cancelled during enumeration".to_string(),
                ));
            }

            let entry_path = entry.path();
            let file_name = entry.file_name();
            let rel_path = rel_prefix.join(&file_name);
            let target_path = self.target_base.join(&rel_path);

            // Path length check. Sub-threshold: skip the file and continue;
            // at/over threshold: fail the transfer (see `emit_error`).
            let max_len = self.config.max_path_length.unwrap_or(4096);
            if target_path.as_os_str().len() > max_len {
                let hard = emit_error(
                    self.tx,
                    TransferError::PathTooLong(target_path.clone()),
                    self.context,
                )
                .await;
                if let Some(m) = self.metrics {
                    m.transfer_errors_total
                        .with_label_values(&["path_too_long"])
                        .inc();
                }
                if let Some(err) = hard {
                    return Err(err);
                }
                continue;
            }

            let meta = match fs::symlink_metadata(&entry_path).await {
                Ok(m) => m,
                Err(e) => {
                    let err: TransferError = e.into();
                    if let Some(hard) = emit_error(self.tx, err, self.context).await {
                        return Err(hard);
                    }
                    continue;
                }
            };

            if meta.file_type().is_symlink() {
                let symlink_count = self.counters.increment_symlink();
                if let Some(max) = self.config.max_total_symlinks
                    && symlink_count > max
                {
                    let err = TransferError::MaxTotalSymlinksExceeded(max);
                    self.audit_events.push(SecurityAuditEvent::new(
                        SecurityEventType::MaxTotalSymlinksExceeded,
                        AuditSeverity::Error,
                        self.audit_context,
                        serde_json::json!({ "count": symlink_count }),
                    ));
                    if let Some(m) = self.metrics {
                        m.transfer_errors_total
                            .with_label_values(&["max_total_symlinks_exceeded"])
                            .inc();
                    }
                    return Err(err);
                }

                let (resolved, resolved_meta, _depth) =
                    match resolve_symlink_chain(&entry_path, self.config.max_symlink_depth).await {
                        Ok(r) => r,
                        Err(e) => {
                            let label = match &e {
                                TransferError::BrokenSymlink(_) => "broken_symlink",
                                TransferError::InvalidSymlinkTarget(_) => "invalid_symlink_target",
                                TransferError::MaxSymlinkDepthExceeded(_) => {
                                    "max_symlink_depth_exceeded"
                                }
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
                            if let Some(m) = self.metrics {
                                m.transfer_errors_total.with_label_values(&[label]).inc();
                            }
                            if let Some(hard) = emit_error(self.tx, e, self.context).await {
                                return Err(hard);
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
                        m.transfer_errors_total
                            .with_label_values(&["max_symlink_references_exceeded"])
                            .inc();
                        m.security_events_total
                            .with_label_values(&["max_symlink_references_exceeded"])
                            .inc();
                    }
                    if let Some(hard) = emit_error(self.tx, e, self.context).await {
                        return Err(hard);
                    }
                    continue;
                }

                // External symlink validation (multi-layer, plan §9):
                // 1. canonical path prefix — resolved target must stay under
                //    the inbox root;
                // 2. device comparison — the resolved target must live on
                //    the same filesystem as the inbox root. This catches
                //    bind-mounted subtrees *inside* the inbox whose paths
                //    canonicalize under the inbox root but whose contents
                //    come from a different filesystem.
                let external_by_path = is_external_to_inbox(&resolved, self.inbox_root);
                let external_by_device = resolved_id.0 != self.inbox_root_id.0;
                if external_by_path || external_by_device {
                    if !self.config.allow_external_symlinks {
                        warn!(
                            "Rejecting external symlink {:?} -> {:?}",
                            entry_path, resolved
                        );
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
                            m.transfer_errors_total
                                .with_label_values(&["external_symlink_not_allowed"])
                                .inc();
                            m.security_events_total
                                .with_label_values(&["external_symlink_rejected"])
                                .inc();
                        }
                        if let Some(hard) = emit_error(
                            self.tx,
                            TransferError::ExternalSymlinkNotAllowed {
                                path: entry_path.clone(),
                            },
                            self.context,
                        )
                        .await
                        {
                            return Err(hard);
                        }
                        continue;
                    }
                    debug!(
                        "Following external symlink {:?} -> {:?}",
                        entry_path, resolved
                    );
                }

                if resolved_meta.is_dir() {
                    // Recurse into the resolved directory, preserving the symlink's path.
                    self.collect(
                        &resolved,
                        &rel_path,
                        active_chain,
                        global_inodes,
                        _current_depth + 1,
                    )
                    .await?;
                } else if resolved_meta.is_file() {
                    let file_count = self.counters.increment_file();
                    if let Some(max) = self.config.max_total_files
                        && file_count > max
                    {
                        let err = TransferError::MaxTotalFilesExceeded(max);
                        self.audit_events.push(SecurityAuditEvent::new(
                            SecurityEventType::MaxTotalFilesExceeded,
                            AuditSeverity::Error,
                            self.audit_context,
                            serde_json::json!({ "count": file_count }),
                        ));
                        if let Some(m) = self.metrics {
                            m.transfer_errors_total
                                .with_label_values(&["max_total_files_exceeded"])
                                .inc();
                        }
                        return Err(err);
                    }

                    // Copy through the symlink: opening the link path reads
                    // the target's content, and the manifest/cleanup then
                    // reference the symlink itself (inside the inbox) rather
                    // than the resolved target — which may live outside the
                    // inbox when `allow_external_symlinks` is enabled.
                    self.tx
                        .send(Ok((entry_path, target_path)))
                        .await
                        .map_err(|_| TransferError::Cancelled("channel closed".to_string()))?;
                } else {
                    warn!(
                        "Skipping unsupported symlink target type for {:?}",
                        entry_path
                    );
                }
            } else if meta.is_dir() {
                // Recurse into regular directory.
                self.collect(
                    &entry_path,
                    &rel_path,
                    active_chain,
                    global_inodes,
                    _current_depth + 1,
                )
                .await?;
            } else if meta.is_file() {
                let file_count = self.counters.increment_file();
                if let Some(max) = self.config.max_total_files
                    && file_count > max
                {
                    let err = TransferError::MaxTotalFilesExceeded(max);
                    self.audit_events.push(SecurityAuditEvent::new(
                        SecurityEventType::MaxTotalFilesExceeded,
                        AuditSeverity::Error,
                        self.audit_context,
                        serde_json::json!({ "count": file_count }),
                    ));
                    if let Some(m) = self.metrics {
                        m.transfer_errors_total
                            .with_label_values(&["max_total_files_exceeded"])
                            .inc();
                    }
                    return Err(err);
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
pub(crate) async fn validate_warehouse_path(
    warehouse_path: &Path,
    protected_base: &Path,
) -> Result<PathBuf> {
    let protected_canonical = fs::canonicalize(protected_base)
        .await
        .map_err(|_e| TransferError::InvalidWarehousePath(protected_base.to_path_buf()))?;

    let warehouse_canonical = if warehouse_path.exists() {
        fs::canonicalize(warehouse_path).await
    } else {
        // Canonicalize the parent and append the leaf.
        let parent = warehouse_path.parent().unwrap_or_else(|| Path::new("/"));
        let leaf = warehouse_path.file_name().unwrap_or_default();
        fs::canonicalize(parent).await.map(|p| p.join(leaf))
    }
    .map_err(|_e| TransferError::InvalidWarehousePath(warehouse_path.to_path_buf()))?;

    if !warehouse_canonical.starts_with(&protected_canonical) {
        return Err(TransferError::InvalidWarehousePath(warehouse_canonical));
    }

    Ok(warehouse_canonical)
}

/// Check available disk space on the warehouse filesystem.
#[cfg(unix)]
pub(crate) fn check_disk_space(target_dir: &Path, _required_bytes: u64) -> Result<u64> {
    use nix::sys::statvfs::statvfs;

    let stat = statvfs(target_dir).map_err(|e| {
        TransferError::StorageError(format!(
            "Failed to query filesystem space for {:?}: {}",
            target_dir, e
        ))
    })?;

    let available = stat.blocks_available() as u64 * stat.fragment_size() as u64;
    Ok(available)
}

#[cfg(not(unix))]
pub(crate) fn check_disk_space(_target_dir: &Path, _required_bytes: u64) -> Result<u64> {
    // Skip disk-space check on non-Unix platforms.
    Ok(u64::MAX)
}

/// Pre-transfer integrity scan: read-only walk that validates symlinks, counts
/// files, and sums their sizes before any data is copied.
async fn pre_transfer_integrity_scan(
    inbox_path: &Path,
    config: &InboxConfig,
    inbox_root_id: (u64, u64),
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

        let mut entries = fs::read_dir(&canonical).await.map_err(|e| {
            TransferError::InboxIntegrityFailed(format!(
                "Failed to read directory {:?}: {}",
                canonical, e
            ))
        })?;

        while let Some(entry) = entries.next_entry().await? {
            let entry_path = entry.path();
            let entry_meta = fs::symlink_metadata(&entry_path).await?;

            if entry_meta.file_type().is_symlink() {
                symlink_count += 1;
                if let Some(max) = config.max_total_symlinks
                    && symlink_count > max
                {
                    return Err(TransferError::MaxTotalSymlinksExceeded(max));
                }
                match resolve_symlink_chain(&entry_path, config.max_symlink_depth).await {
                    Ok((resolved, resolved_meta, _depth)) => {
                        // Same multi-layer external check as the collector:
                        // canonical path prefix + device comparison.
                        let resolved_id = dev_ino(&resolved_meta);
                        let is_external = is_external_to_inbox(&resolved, inbox_path)
                            || resolved_id.0 != inbox_root_id.0;
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
                            if let Some(max) = config.max_total_files
                                && file_count > max
                            {
                                return Err(TransferError::MaxTotalFilesExceeded(max));
                            }
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
                    Err(e) => {
                        return Err(TransferError::InboxIntegrityFailed(format!(
                            "{:?}: {}",
                            entry_path, e
                        )));
                    }
                }
            } else if entry_meta.is_dir() {
                stack.push(entry_path);
            } else if entry_meta.is_file() {
                total_size += entry_meta.len();
                file_count += 1;
                if let Some(max) = config.max_total_files
                    && file_count > max
                {
                    return Err(TransferError::MaxTotalFilesExceeded(max));
                }
            }
        }
    }

    Ok((total_size, file_count))
}

/// Compute a unique temporary directory name next to the warehouse path.
pub(crate) fn temp_transfer_dir_name(warehouse_path: &Path) -> PathBuf {
    let timestamp = Utc::now().timestamp_millis();
    let random: u32 = rand::rng().random();
    let name = format!(
        "{}.transfer-{}-{}",
        warehouse_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy(),
        timestamp,
        random
    );
    warehouse_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(name)
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
///
/// Only paths *inside* the inbox are deleted. Sources that resolved outside
/// the inbox (e.g. via external symlinks when `allow_external_symlinks` is
/// enabled, or files under an external directory symlink) are never touched;
/// the inbox itself is removed wholesale at the end, which also deletes the
/// symlink entries.
async fn cleanup_source(inbox_path: &Path, transferred_files: &[FileInfo]) -> Result<()> {
    let inbox_canonical = fs::canonicalize(inbox_path).await.map_err(|e| {
        TransferError::StorageError(format!(
            "Failed to canonicalize inbox {:?}: {}",
            inbox_path, e
        ))
    })?;

    // Delete individual source files first (inbox-contained only).
    for file in transferred_files {
        if file.source_path.starts_with(&inbox_canonical) {
            if let Err(e) = fs::remove_file(&file.source_path).await {
                warn!("Failed to delete source file {:?}: {}", file.source_path, e);
            }
        } else {
            debug!(
                "Skipping external source {:?} during cleanup",
                file.source_path
            );
        }
    }

    // Remove the inbox directory and any remaining entries (symlinks, empty
    // dirs, failed stragglers).
    if let Err(e) = fs::remove_dir_all(inbox_path).await {
        warn!("Failed to remove inbox directory {:?}: {}", inbox_path, e);
    }

    Ok(())
}

/// Build a [`TransferError::TransferFailed`] for failures that occur before
/// the producer/consumer pipeline (integrity scan, disk space, ...),
/// recording the matching audit event so the compliance trail is populated
/// even when the transfer never started copying (plan §12).
fn early_transfer_failure(
    errors: Vec<TransferError>,
    event_type: SecurityEventType,
    audit_context: &AuditContext,
) -> TransferError {
    let severity = match event_type {
        SecurityEventType::InboxIntegrityCheckFailed
        | SecurityEventType::AtomicRollbackTriggered
        | SecurityEventType::WarehousePathValidationFailed
        | SecurityEventType::InboxLockFailed
        | SecurityEventType::SymlinkLoopDetected => AuditSeverity::Error,
        SecurityEventType::InsufficientDiskSpace
        | SecurityEventType::ExternalSymlinkRejected
        | SecurityEventType::MaxSymlinkReferencesExceeded => AuditSeverity::Warning,
        _ => AuditSeverity::Error,
    };
    let audit_events = vec![SecurityAuditEvent::new(
        event_type,
        severity,
        audit_context,
        serde_json::json!({ "reason": "pre-transfer failure" }),
    )];
    TransferError::TransferFailed {
        errors,
        audit_events,
    }
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
    transfer_files_with_metrics(
        inbox_path,
        warehouse_path,
        config,
        parallelism,
        chunk_size_mb,
        audit_context,
        None,
        None,
        None,
    )
    .await
}

/// Variant that accepts an optional metrics handle for observability.
///
/// Pass `Some(metrics)` (an `Arc<CitadelMetrics>` from the application state)
/// to record transfer metrics; pass `None` to skip metrics recording.
///
/// `progress` (an [`Option<Arc<ProgressTracker>>`]) is the D5 wiring: when
/// provided, [`ProgressTracker::increment_files`] is called as each file
/// completes so the job main loop can report progress to Citadel on an
/// interval.
///
/// `external_cancel` lets a caller (citadel-job's metadata bail-out poll)
/// cancel the transfer mid-flight by triggering the shared token.
#[allow(clippy::too_many_arguments)]
pub async fn transfer_files_with_metrics(
    inbox_path: &Path,
    warehouse_path: &Path,
    config: &CitadelConfig,
    parallelism: usize,
    chunk_size_mb: usize,
    audit_context: AuditContext,
    metrics: Option<Arc<CitadelMetrics>>,
    progress: Option<Arc<ProgressTracker>>,
    external_cancel: Option<CancellationToken>,
) -> Result<TransferResult> {
    info!(
        "Starting file transfer from {:?} to {:?}",
        inbox_path, warehouse_path
    );

    // Step 1: Validate warehouse path.
    let warehouse_canonical =
        match validate_warehouse_path(warehouse_path, &config.protected_base_path).await {
            Ok(c) => c,
            Err(e) => {
                if let Some(m) = metrics.as_ref() {
                    m.transfer_errors_total
                        .with_label_values(&["warehouse_path_validation_failed"])
                        .inc();
                }
                return Err(e);
            }
        };

    // Step 2: Lock inbox directory.
    let lock = match InboxLock::lock(inbox_path).await {
        Ok(l) => l,
        Err(e) => {
            if let Some(m) = metrics.as_ref() {
                m.transfer_errors_total
                    .with_label_values(&["inbox_lock_failed"])
                    .inc();
            }
            return Err(e);
        }
    };

    // Step 3: Resolve canonical inbox root and its filesystem identity.
    //
    // The external-symlink check is multi-layer (see plan §9):
    //   1. canonical path prefix — resolved target must stay under the
    //      inbox root;
    //   2. device comparison — the resolved target's `st_dev` must match
    //      the inbox root's `st_dev`. This catches bind-mounted subtrees
    //      *inside* the inbox whose paths canonicalize under the inbox
    //      root but whose contents live on a different filesystem.
    let inbox_canonical = fs::canonicalize(inbox_path).await.map_err(|e| {
        TransferError::StorageError(format!(
            "Failed to canonicalize inbox {:?}: {}",
            inbox_path, e
        ))
    })?;
    let inbox_meta = tokio::fs::symlink_metadata(&inbox_canonical).await?;
    let inbox_root_id = dev_ino(&inbox_meta);

    // Step 4: Pre-transfer integrity scan (optional).
    let (required_bytes, found_files) = if config.inbox.pre_transfer_integrity_scan {
        match pre_transfer_integrity_scan(&inbox_canonical, &config.inbox, inbox_root_id).await {
            Ok(size_count) => size_count,
            Err(e) => {
                tracing::error!("Pre-transfer integrity scan failed: {}", e);
                if let Some(m) = metrics.as_ref() {
                    m.transfer_errors_total
                        .with_label_values(&["inbox_integrity_failed"])
                        .inc();
                    m.security_events_total
                        .with_label_values(&["inbox_integrity_check_failed"])
                        .inc();
                }
                drop(lock);
                return Err(early_transfer_failure(
                    vec![e],
                    SecurityEventType::InboxIntegrityCheckFailed,
                    &audit_context,
                ));
            }
        }
    } else {
        (0, 0)
    };

    // Step 5: Disk space check with 10% buffer.
    if required_bytes > 0 {
        let required_with_buffer = (required_bytes as f64 * 1.10).ceil() as u64;
        let target_dir = warehouse_canonical
            .parent()
            .unwrap_or_else(|| Path::new("/"));
        let available = check_disk_space(target_dir, required_with_buffer)?;
        if available < required_with_buffer {
            tracing::error!(
                "Insufficient disk space: required {} bytes, available {} bytes",
                required_with_buffer,
                available
            );
            if let Some(m) = metrics.as_ref() {
                m.transfer_errors_total
                    .with_label_values(&["insufficient_disk_space"])
                    .inc();
                m.security_events_total
                    .with_label_values(&["insufficient_disk_space"])
                    .inc();
            }
            drop(lock);
            return Err(early_transfer_failure(
                vec![TransferError::InsufficientDiskSpace(
                    required_with_buffer,
                    available,
                )],
                SecurityEventType::InsufficientDiskSpace,
                &audit_context,
            ));
        }
    }

    // Step 6: Determine target base (atomic temp dir or direct warehouse path).
    let (target_base, mut temp_dir) = if config.inbox.atomic_transfer {
        let temp = temp_transfer_dir_name(&warehouse_canonical);
        fs::create_dir_all(&temp).await.map_err(|e| {
            TransferError::StorageError(format!(
                "Failed to create temp transfer directory {:?}: {}",
                temp, e
            ))
        })?;
        (temp.clone(), Some(temp))
    } else {
        (warehouse_canonical.clone(), None)
    };

    // Step 7: Set up the producer-consumer pipeline.
    let channel_capacity = (parallelism * 2).max(1);
    let (sender, mut receiver) = mpsc::channel::<Result<(PathBuf, PathBuf)>>(channel_capacity);

    let context = Arc::new(TransferContext::with_external_token(
        config.inbox.max_errors_before_fail.max(1),
        external_cancel.unwrap_or_default(),
    ));
    let counters = Arc::new(TransferCounters::new());
    let chunk_size = chunk_size_mb.max(1) * 1024 * 1024;
    let start = std::time::Instant::now();

    if let Some(p) = progress.as_ref() {
        // Expose the scanned totals so live progress can report counts.
        p.set_total_files(found_files as usize).await;
        p.update_phase("transferring").await;
    }

    let mut active_chain: HashSet<(u64, u64)> = HashSet::new();
    // Note: the inbox root is intentionally NOT pre-inserted here. The top
    // level `collect()` call inserts it on entry (and removes it on return),
    // which is what allows symlinks pointing back at the inbox root itself to
    // be detected as loops while avoiding a false positive on first entry.
    let mut global_inodes: HashMap<(u64, u64), u32> = HashMap::new();
    let mut audit_events: Vec<SecurityAuditEvent> = Vec::new();
    let mut collector = Collector {
        target_base: &target_base,
        inbox_root: &inbox_canonical,
        inbox_root_id,
        config: &config.inbox,
        tx: &sender,
        context: &context,
        counters: &counters,
        metrics: metrics.as_deref(),
        audit_context: &audit_context,
        audit_events: &mut audit_events,
    };

    // Copy workers are spawned from the pipeline loop below; the semaphore
    // bounds in-flight copies to `parallelism` (the permit is held inside the
    // spawned task for the duration of the copy).
    let semaphore = Arc::new(Semaphore::new(parallelism.max(1)));
    let mut join_set = JoinSet::new();
    let mut manifest: Vec<PathBuf> = Vec::new();
    let mut transferred: Vec<FileInfo> = Vec::new();
    let mut collected_errors: Vec<TransferError> = Vec::new();

    /// Dispatch a (source, target) pair to a spawned copy worker unless the
    /// transfer has been cancelled.
    macro_rules! dispatch_copy {
        ($source:expr, $target:expr) => {
            if !context.is_cancelled() {
                let target_path = $target.clone();
                let sem = semaphore.clone();
                let ctx = context.clone();
                let cnt = counters.clone();
                let m = metrics.clone();
                let p = progress.clone();
                join_set.spawn(async move {
                    let _permit = sem
                        .acquire_owned()
                        .await
                        .expect("transfer semaphore was closed");
                    match copy_file_worker($source, target_path, chunk_size, ctx.clone(), cnt, m, p)
                        .await
                    {
                        Ok(info) => Ok(info),
                        Err(e) => {
                            // Record here so the producer observes the
                            // cancellation promptly (workers fail-fast).
                            ctx.record_error();
                            Err(e)
                        }
                    }
                });
                manifest.push($target);
            }
        };
    }

    // Drive the producer (tree walk) and the dispatcher concurrently on this
    // task. The producer is never spawned: it borrows local state, and the
    // bounded channel provides backpressure. Either side finishing or the
    // error threshold cancelling the token unwinds the other.
    let mut producer = collector.collect(
        &inbox_canonical,
        Path::new(""),
        &mut active_chain,
        &mut global_inodes,
        0,
    );
    let mut producer_status: Option<Result<()>> = 'pipeline: loop {
        tokio::select! {
            status = producer.as_mut() => {
                break 'pipeline Some(status);
            }
            item = receiver.recv() => match item {
                Some(Ok((source, target))) => {
                    dispatch_copy!(source, target);
                }
                Some(Err(e)) => {
                    // Already recorded in the TransferContext at the error
                    // origin (emit_error); collect it for the final report.
                    // `Cancelled` is the cancellation mechanism, not a
                    // cause — never report it as an error.
                    if !matches!(e, TransferError::Cancelled(_)) {
                        warn!("Transfer error during collection: {}", e);
                        collected_errors.push(e);
                    }
                }
                None => {
                    // All senders dropped without the producer completing on
                    // this task (should not happen, but fail safely as a
                    // hard error — cancellation is not involved).
                    break 'pipeline Some(Err(TransferError::FileTransferFailed(
                        "producer channel closed unexpectedly".to_string(),
                    )));
                }
            },
        }
    };

    // Drain items still buffered after the producer completed.
    while let Ok(item) = receiver.try_recv() {
        match item {
            Ok((source, target)) => {
                if matches!(&producer_status, Some(Ok(_))) {
                    dispatch_copy!(source, target);
                }
            }
            Err(e) => {
                if !matches!(e, TransferError::Cancelled(_)) {
                    warn!("Transfer error during collection: {}", e);
                    collected_errors.push(e);
                }
            }
        }
    }
    drop(receiver);
    drop(producer); // ends the collector's borrow of audit_events & co.

    // Record the producer's own failure (loops, depth/count limits, ...).
    // A structural producer error always fails the run regardless of the
    // error threshold: collection was aborted, so the tree was not fully
    // walked. `Cancelled` is a byproduct of threshold-triggered
    // cancellation — the real cause is already recorded — so it is
    // filtered, mirroring the channel path.
    let producer_ok = !matches!(&producer_status, Some(Err(_)));
    if let Some(Err(e)) = producer_status.take() {
        if matches!(e, TransferError::Cancelled(_)) {
            debug!("Producer exited due to cancellation: {}", e);
        } else {
            error!("Directory collection failed: {}", e);
            collected_errors.push(e);
        }
    }

    // Wait for all in-flight copies to finish.
    while let Some(res) = join_set.join_next().await {
        match res {
            Ok(Ok(info)) => {
                transferred.push(info);
            }
            Ok(Err(e)) => {
                error!("File transfer failed: {}", e);
                collected_errors.push(e);
                if let Some(m) = metrics.as_ref() {
                    m.transfer_errors_total
                        .with_label_values(&["file_transfer_failed"])
                        .inc();
                }
            }
            Err(e) => {
                error!("Worker task panicked: {}", e);
                context.record_error();
                collected_errors.push(TransferError::FileTransferFailed(format!(
                    "worker task panicked: {}",
                    e
                )));
            }
        }
    }

    // Failure is determined by the cancellation token (triggered by
    // `record_error` at the exact point each error is observed, once the
    // configured threshold is reached: `max_errors_before_fail = N` fails on
    // the Nth error, while fewer than N errors are collected in
    // `result.errors` and the transfer still succeeds — plan §2: "Collect
    // up to N errors before failing the transfer") OR by a structural
    // producer error (loops, depth/count limits, channel closed), which
    // always aborts collection and always fails the run.
    let success = producer_ok && !context.is_cancelled();

    if !success {
        // Remove the temporary directory or roll back the manifest before
        // building the failure result.
        if let Some(temp) = temp_dir.take() {
            if let Err(e) = fs::remove_dir_all(&temp).await {
                warn!("Failed to remove temp transfer directory {:?}: {}", temp, e);
            }
            if let Some(m) = metrics.as_ref() {
                m.transfer_atomic_rollbacks_total.inc();
            }
        } else {
            rollback_files(&manifest).await;
        }
        audit_events.push(SecurityAuditEvent::new(
            SecurityEventType::AtomicRollbackTriggered,
            AuditSeverity::Error,
            &audit_context,
            serde_json::json!({ "reason": "transfer failure" }),
        ));

        drop(lock);

        error!(
            "Transfer failed: {} file(s) collected, {} error(s) recorded",
            transferred.len(),
            context.error_count()
        );

        if let Some(p) = progress.as_ref() {
            p.update_phase("failed").await;
        }

        // Surface the errors collected during the run. The wrapper carries
        // the full error list plus the audit trail so callers can report
        // security events even when the transfer failed (plan §12).
        if collected_errors.is_empty() {
            collected_errors.push(TransferError::FileTransferFailed(
                "one or more files failed to transfer".to_string(),
            ));
        }
        return Err(TransferError::TransferFailed {
            errors: collected_errors,
            audit_events,
        });
    }

    // Step 8: Commit atomic transfer.
    if let Some(temp) = temp_dir {
        if warehouse_canonical.exists() {
            let mut existing = fs::read_dir(&warehouse_canonical).await.map_err(|e| {
                TransferError::StorageError(format!(
                    "Failed to inspect existing warehouse path: {}",
                    e
                ))
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

    if let Some(m) = metrics.as_ref() {
        m.transfer_duration.observe(start.elapsed().as_secs_f64());
    }

    if let Some(p) = progress.as_ref() {
        p.update_phase("committed").await;
    }

    info!(
        "Successfully transferred {} files ({} bytes) in {:.2}s",
        transferred.len(),
        total_bytes,
        start.elapsed().as_secs_f64()
    );

    let mut result = TransferResult::empty(true);
    result.files_transferred = transferred;
    result.total_bytes = total_bytes;
    result.audit_events = audit_events;
    // Sub-threshold errors (e.g. a broken symlink collected with
    // `max_errors_before_fail` > 1) do not fail the transfer but are
    // reported to callers via `result.errors`.
    result.errors = collected_errors;
    Ok(result)
}

// Re-export serde for derive macros used inside this module.
use serde::{Deserialize, Serialize};
