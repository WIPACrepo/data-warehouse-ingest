//! Citadel unit and integration tests

pub mod test_utils;

/// Test the transfer module end-to-end
#[cfg(test)]
mod transfer_tests {
    use super::test_utils::TestFixture;
    use crate::storage::transfer::AuditContext;

    fn default_audit() -> AuditContext {
        AuditContext::new(None, None)
    }

    #[tokio::test]
    async fn test_transfer_files_empty_dir() {
        let fixture = TestFixture::new();

        // Create an empty inbox directory
        let inbox_id = mongodb::bson::oid::ObjectId::new();
        let inbox_path = fixture.inbox_base.join(inbox_id.to_string());
        std::fs::create_dir_all(&inbox_path).unwrap();

        let protected_path = fixture.protected_base.join(inbox_id.to_string());
        std::fs::create_dir_all(&protected_path).unwrap();

        let result = crate::storage::transfer::transfer_files(
            &inbox_path,
            &protected_path,
            &fixture.config,
            4,
            16,
            default_audit(),
        ).await;

        assert!(result.is_ok());
        let transfer = result.unwrap();
        assert!(transfer.success);
        assert!(transfer.files_transferred.is_empty());
    }

    #[tokio::test]
    async fn test_transfer_files_single_file() {
        let fixture = TestFixture::new();

        // Create inbox with a file
        let inbox_id = mongodb::bson::oid::ObjectId::new();
        let inbox_path = fixture.inbox_base.join(inbox_id.to_string());
        std::fs::create_dir_all(&inbox_path).unwrap();

        let test_file = inbox_path.join("test.txt");
        std::fs::write(&test_file, "hello world").unwrap();

        let protected_path = fixture.protected_base.join(inbox_id.to_string());
        std::fs::create_dir_all(&protected_path).unwrap();

        let result = crate::storage::transfer::transfer_files(
            &inbox_path,
            &protected_path,
            &fixture.config,
            4,
            16,
            default_audit(),
        ).await;

        assert!(result.is_ok(), "transfer failed: {:?}", result.err());
        let transfer = result.unwrap();
        assert!(transfer.success);
        assert_eq!(transfer.files_transferred.len(), 1);
        assert_eq!(transfer.total_bytes, 11);

        // Source file should be deleted
        assert!(!test_file.exists());

        // Target file should exist
        let target_file = protected_path.join("test.txt");
        assert!(target_file.exists());
        assert_eq!(std::fs::read_to_string(&target_file).unwrap(), "hello world");
    }

    #[tokio::test]
    async fn test_transfer_files_multiple_files() {
        let fixture = TestFixture::new();

        // Create inbox with multiple files
        let inbox_id = mongodb::bson::oid::ObjectId::new();
        let inbox_path = fixture.inbox_base.join(inbox_id.to_string());
        std::fs::create_dir_all(&inbox_path).unwrap();

        std::fs::write(inbox_path.join("file1.txt"), "aaa").unwrap();
        std::fs::write(inbox_path.join("file2.txt"), "bbbb").unwrap();
        std::fs::write(inbox_path.join("file3.txt"), "ccccc").unwrap();

        let protected_path = fixture.protected_base.join(inbox_id.to_string());
        std::fs::create_dir_all(&protected_path).unwrap();

        let result = crate::storage::transfer::transfer_files(
            &inbox_path,
            &protected_path,
            &fixture.config,
            2,
            16,
            default_audit(),
        ).await;

        assert!(result.is_ok());
        let transfer = result.unwrap();
        assert!(transfer.success);
        assert_eq!(transfer.files_transferred.len(), 3);
        assert_eq!(transfer.total_bytes, 3 + 4 + 5);

        // All source files deleted
        assert!(!inbox_path.join("file1.txt").exists());
        assert!(!inbox_path.join("file2.txt").exists());
        assert!(!inbox_path.join("file3.txt").exists());

        // All target files exist with correct content
        assert_eq!(std::fs::read_to_string(protected_path.join("file1.txt")).unwrap(), "aaa");
        assert_eq!(std::fs::read_to_string(protected_path.join("file2.txt")).unwrap(), "bbbb");
        assert_eq!(std::fs::read_to_string(protected_path.join("file3.txt")).unwrap(), "ccccc");
    }
}

/// Test the checksum module
#[cfg(test)]
mod checksum_tests {
    use super::test_utils::TestFixture;
    use sha2::{Sha512, Digest};

    #[tokio::test]
    async fn test_checksum_deterministic() {
        let fixture = TestFixture::new();

        let test_file = fixture.temp_dir.path().join("deterministic.txt");
        std::fs::write(&test_file, "same content").unwrap();

        let hash1 = crate::storage::checksum::compute_checksum(&test_file, 1024).await.unwrap();
        let hash2 = crate::storage::checksum::compute_checksum(&test_file, 1024).await.unwrap();
        assert_eq!(hash1, hash2);
    }

    #[tokio::test]
    async fn test_checksum_matches_sha512() {
        let fixture = TestFixture::new();

        let test_file = fixture.temp_dir.path().join("sha512_test.txt");
        let content = b"verify checksum correctness";
        std::fs::write(&test_file, content).unwrap();

        let expected = hex::encode(Sha512::digest(content));
        let actual = crate::storage::checksum::compute_checksum(&test_file, 1024).await.unwrap();
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn test_checksum_different_chunk_sizes() {
        let fixture = TestFixture::new();

        let test_file = fixture.temp_dir.path().join("chunks.txt");
        let content = vec![0u8; 2048];
        std::fs::write(&test_file, &content).unwrap();

        let hash_1 = crate::storage::checksum::compute_checksum(&test_file, 1).await.unwrap();
        let hash_64 = crate::storage::checksum::compute_checksum(&test_file, 64 * 1024).await.unwrap();
        assert_eq!(hash_1, hash_64);
    }
}

/// Test the storage manager
#[cfg(test)]
mod storage_manager_tests {
    use super::test_utils::TestFixture;
    use mongodb::bson::oid::ObjectId;

    #[tokio::test]
    async fn test_create_inbox() {
        let fixture = TestFixture::new();
        let manager = fixture.storage_manager();

        let inbox_id = ObjectId::new();
        let result = manager.create_inbox(&inbox_id).await;

        assert!(result.is_ok());
        let path = result.unwrap();
        assert!(path.exists());
        assert!(path.is_dir());
    }

    #[tokio::test]
    async fn test_delete_inbox() {
        let fixture = TestFixture::new();
        let manager = fixture.storage_manager();

        let inbox_id = ObjectId::new();
        let path = manager.create_inbox(&inbox_id).await.unwrap();

        // Write a file
        std::fs::write(path.join("data.txt"), "test").unwrap();

        let result = manager.delete_inbox(&inbox_id).await;
        assert!(result.is_ok());
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn test_path_validation_normal() {
        let fixture = TestFixture::new();
        let manager = fixture.storage_manager();

        // Create a valid subdir
        let subdir = fixture.inbox_base.join("safe_dir");
        std::fs::create_dir_all(&subdir).unwrap();
        let test_file = subdir.join("file.txt");
        std::fs::write(&test_file, "data").unwrap();

        let result = manager.validate_path(&fixture.inbox_base, &subdir.join("file.txt"));
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_path_validation_traversal() {
        let fixture = TestFixture::new();
        let manager = fixture.storage_manager();

        // Try path traversal
        let result = manager.validate_path(
            &fixture.inbox_base,
            &std::path::PathBuf::from("../../../etc/passwd"),
        );
        assert!(result.is_err());
    }
}

/// Test the job builders
#[cfg(test)]
mod builder_tests {
    use super::test_utils::builders::{
        default_test_job, queued_publish_job, running_publish_job,
        queued_delete_job, failed_job, JobBuilder,
    };
    use crate::jobs::types::{JobType, JobStatus};

    #[test]
    fn test_default_test_job() {
        let job = default_test_job();
        assert_eq!(job.job_id, "test-job-001");
        assert_eq!(job.status, JobStatus::Queued);
        assert!(matches!(job.job_type, JobType::PublishInbox { .. }));
        assert_eq!(job.executor.executor_type, "mock");
    }

    #[test]
    fn test_queued_publish_job() {
        let job = queued_publish_job();
        assert_eq!(job.status, JobStatus::Queued);
        assert!(matches!(job.job_type, JobType::PublishInbox { .. }));
    }

    #[test]
    fn test_running_publish_job() {
        let job = running_publish_job();
        assert_eq!(job.status, JobStatus::Running);
        assert!(job.executor.executor_id.is_some());
    }

    #[test]
    fn test_queued_delete_job() {
        let job = queued_delete_job();
        assert_eq!(job.status, JobStatus::Queued);
        assert!(matches!(job.job_type, JobType::DeleteInbox { .. }));
    }

    #[test]
    fn test_failed_job() {
        let job = failed_job();
        assert_eq!(job.status, JobStatus::Failed);
    }

    #[test]
    fn test_custom_job_builder() {
        let job = JobBuilder::default()
            .job_id("custom-123")
            .status(JobStatus::Completed)
            .executor_type("kubernetes")
            .executor_id(Some("citadel-job-custom-123".to_string()))
            .build();

        assert_eq!(job.job_id, "custom-123");
        assert_eq!(job.status, JobStatus::Completed);
        assert_eq!(job.executor.executor_type, "kubernetes");
        assert_eq!(
            job.executor.executor_id,
            Some("citadel-job-custom-123".to_string())
        );
    }
}

/// Comprehensive recursive transfer tests: symlink handling, loop detection,
/// security limits, error collection, atomic rollback, cleanup semantics.
///
/// These require Unix symlink support (the dev/CI environments are Linux).
#[cfg(unix)]
mod recursive_transfer_tests {
    use std::collections::HashMap;
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::Path;

    use super::test_utils::TestFixture;
    use crate::config::InboxConfig;
    use crate::storage::transfer::{
        check_disk_space, count_symlink_reference, is_external_to_inbox, temp_transfer_dir_name,
        validate_symlink_target, validate_warehouse_path, AuditContext, TransferContext,
        TransferError,
    };

    fn default_audit() -> AuditContext {
        AuditContext::new(None, None)
    }

    /// Default test config: integrity scan off (unless a test opts in) so
    /// individual error paths are exercised by the transfer pipeline itself.
    fn scan_off_config(fixture: &TestFixture) -> crate::config::CitadelConfig {
        let mut cfg = fixture.config.clone();
        cfg.inbox.pre_transfer_integrity_scan = false;
        cfg
    }

    /// Assert no `.transfer-*` temp directories leaked into `dir`.
    fn assert_no_temp_dirs(dir: &Path) {
        if !dir.exists() {
            return;
        }
        let leftovers: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".transfer-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp transfer dirs leaked: {:?}",
            leftovers
        );
    }

    fn assert_inbox_untouched(inbox: &Path, files: &[&str]) {
        assert!(inbox.exists(), "inbox must survive a failed transfer");
        for f in files {
            // NOTE: `Path::exists()` follows symlinks — a broken symlink
            // reports `false` even though the entry is still present.
            // Check the entry itself with `symlink_metadata`.
            assert!(
                fs::symlink_metadata(inbox.join(f)).is_ok(),
                "inbox entry {} missing (broken symlinks count too)",
                f
            );
        }
    }

    // ── 1. Recursive multi-level transfer ──────────────────────────────

    #[tokio::test]
    async fn test_recursive_multi_level_transfer() {
        let fixture = TestFixture::new();
        let inbox = fixture.inbox_base.join("inbox_recursive");
        fs::create_dir_all(inbox.join("a/b/c")).unwrap();
        fs::create_dir_all(inbox.join("empty_dir/nested_empty")).unwrap();
        fs::write(inbox.join("root.txt"), "root").unwrap();
        fs::write(inbox.join("a/one.txt"), "12345").unwrap();
        fs::write(inbox.join("a/b/two.txt"), "abcdef").unwrap();
        fs::write(inbox.join("a/b/c/three.txt"), "ghijklm").unwrap();

        let warehouse = fixture.protected_base.join("inbox_recursive");
        let cfg = scan_off_config(&fixture);
        let result = crate::storage::transfer::transfer_files(
            &inbox,
            &warehouse,
            &cfg,
            4,
            1,
            default_audit(),
        )
        .await
        .expect("recursive transfer should succeed");

        assert!(result.success);
        assert_eq!(result.files_transferred.len(), 4);
        assert_eq!(result.total_bytes, 4 + 5 + 6 + 7);

        // Structure preserved, content correct.
        assert_eq!(fs::read_to_string(warehouse.join("root.txt")).unwrap(), "root");
        assert_eq!(fs::read_to_string(warehouse.join("a/one.txt")).unwrap(), "12345");
        assert_eq!(fs::read_to_string(warehouse.join("a/b/two.txt")).unwrap(), "abcdef");
        assert_eq!(
            fs::read_to_string(warehouse.join("a/b/c/three.txt")).unwrap(),
            "ghijklm"
        );
        // Empty subdirectories created in target.
        assert!(warehouse.join("empty_dir/nested_empty").is_dir());
        // Inbox removed entirely on success.
        assert!(!inbox.exists());
    }

    // ── 2. Symlink to file ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_symlink_to_file_transferred_as_regular_file() {
        let fixture = TestFixture::new();
        let inbox = fixture.inbox_base.join("inbox_link_file");
        fs::create_dir_all(&inbox).unwrap();
        fs::write(inbox.join("real.txt"), "link content").unwrap();
        symlink(inbox.join("real.txt"), inbox.join("link.txt")).unwrap();

        let warehouse = fixture.protected_base.join("inbox_link_file");
        let cfg = scan_off_config(&fixture);
        let result = crate::storage::transfer::transfer_files(
            &inbox,
            &warehouse,
            &cfg,
            2,
            1,
            default_audit(),
        )
        .await
        .expect("symlink-to-file transfer should succeed");

        assert!(result.success);
        assert_eq!(result.files_transferred.len(), 2);

        // Target is a regular file with the linked content.
        let target = warehouse.join("link.txt");
        assert!(target.is_file(), "target of file symlink must be a regular file");
        assert_eq!(fs::read_to_string(&target).unwrap(), "link content");
        assert_eq!(
            fs::read_to_string(warehouse.join("real.txt")).unwrap(),
            "link content"
        );
        assert!(!inbox.exists());
    }

    // ── 3. Symlink to directory ─────────────────────────────────────────

    #[tokio::test]
    async fn test_symlink_to_directory_followed() {
        let fixture = TestFixture::new();
        let inbox = fixture.inbox_base.join("inbox_link_dir");
        fs::create_dir_all(inbox.join("subdir")).unwrap();
        fs::write(inbox.join("subdir/data.txt"), "dir data").unwrap();
        // Relative symlink to the sibling directory.
        symlink("subdir", inbox.join("linkdir")).unwrap();

        let warehouse = fixture.protected_base.join("inbox_link_dir");
        let cfg = scan_off_config(&fixture);
        let result = crate::storage::transfer::transfer_files(
            &inbox,
            &warehouse,
            &cfg,
            2,
            1,
            default_audit(),
        )
        .await
        .expect("symlink-to-dir transfer should succeed");

        assert!(result.success);
        assert_eq!(result.files_transferred.len(), 2);
        // Content appears both under the real dir and under the link path.
        assert_eq!(
            fs::read_to_string(warehouse.join("subdir/data.txt")).unwrap(),
            "dir data"
        );
        let linked = warehouse.join("linkdir/data.txt");
        assert!(linked.is_file());
        assert_eq!(fs::read_to_string(&linked).unwrap(), "dir data");
        assert!(!inbox.exists());
    }

    // ── 4. Symlink loops ────────────────────────────────────────────────

    #[tokio::test]
    async fn test_directory_symlink_loop_detected() {
        let fixture = TestFixture::new();
        let inbox = fixture.inbox_base.join("inbox_loop_dir");
        fs::create_dir_all(inbox.join("sub")).unwrap();
        fs::write(inbox.join("sub/real.txt"), "ok").unwrap();
        // Absolute symlink from sub/ back to the inbox root → active-chain
        // loop detection must catch it.
        let canonical_inbox = fs::canonicalize(&inbox).unwrap();
        symlink(&canonical_inbox, inbox.join("sub/loop")).unwrap();

        let warehouse = fixture.protected_base.join("inbox_loop_dir");
        let cfg = scan_off_config(&fixture);
        let result = crate::storage::transfer::transfer_files(
            &inbox,
            &warehouse,
            &cfg,
            2,
            1,
            default_audit(),
        )
        .await;

        let err = result.expect_err("directory symlink loop must fail the transfer");
        let inner = match err {
            TransferError::TransferFailed { ref errors, .. } => {
                assert!(
                    errors.iter().any(|e| matches!(e, TransferError::SymlinkLoop(_))),
                    "expected SymlinkLoop among {:?}",
                    errors
                );
                errors
            }
            other => panic!("expected TransferFailed, got {:?}", other),
        };
        assert_inbox_untouched(&inbox, &["sub/real.txt", "sub/loop"]);
        assert_no_temp_dirs(fixture.protected_base.as_path());
        assert!(!warehouse.exists(), "no partial warehouse data on loop failure");
        let _ = inner;
    }

    #[tokio::test]
    async fn test_pure_symlink_cycle_detected() {
        let fixture = TestFixture::new();
        let inbox = fixture.inbox_base.join("inbox_loop_chain");
        fs::create_dir_all(&inbox).unwrap();
        // link_a -> link_b -> link_a (chain-level cycle, distinct inodes).
        symlink(inbox.join("link_b"), inbox.join("link_a")).unwrap();
        symlink(inbox.join("link_a"), inbox.join("link_b")).unwrap();
        fs::write(inbox.join("fine.txt"), "fine").unwrap();

        let warehouse = fixture.protected_base.join("inbox_loop_chain");
        let cfg = scan_off_config(&fixture);
        let result = crate::storage::transfer::transfer_files(
            &inbox,
            &warehouse,
            &cfg,
            2,
            1,
            default_audit(),
        )
        .await;

        let err = result.expect_err("pure symlink cycle must fail");
        if let TransferError::TransferFailed { errors, .. } = err {
            assert!(
                errors.iter().any(|e| matches!(e, TransferError::SymlinkLoop(_))),
                "expected SymlinkLoop, got {:?}",
                errors
            );
        } else {
            panic!("expected TransferFailed, got {:?}", err);
        }
        assert_inbox_untouched(&inbox, &["link_a", "link_b", "fine.txt"]);
    }

    // ── 5/6. Broken symlinks ────────────────────────────────────────────

    #[tokio::test]
    async fn test_broken_symlink_fails_with_scan_off() {
        let fixture = TestFixture::new();
        let inbox = fixture.inbox_base.join("inbox_broken");
        fs::create_dir_all(&inbox).unwrap();
        fs::write(inbox.join("good.txt"), "good").unwrap();
        symlink(inbox.join("missing.txt"), inbox.join("broken")).unwrap();

        let warehouse = fixture.protected_base.join("inbox_broken");
        let cfg = scan_off_config(&fixture); // max_errors_before_fail = 1 (default)
        let result = crate::storage::transfer::transfer_files(
            &inbox,
            &warehouse,
            &cfg,
            2,
            1,
            default_audit(),
        )
        .await;

        let err = result.expect_err("broken symlink must fail the transfer");
        if let TransferError::TransferFailed { errors, .. } = err {
            assert!(
                errors.iter().any(|e| matches!(e, TransferError::BrokenSymlink(_))),
                "expected BrokenSymlink, got {:?}",
                errors
            );
        } else {
            panic!("expected TransferFailed, got {:?}", err);
        }
        assert_inbox_untouched(&inbox, &["good.txt", "broken"]);
        assert_no_temp_dirs(fixture.protected_base.as_path());
    }

    #[tokio::test]
    async fn test_broken_symlink_fails_fast_with_scan_on() {
        let fixture = TestFixture::new();
        let inbox = fixture.inbox_base.join("inbox_broken_scan");
        fs::create_dir_all(&inbox).unwrap();
        fs::write(inbox.join("good.txt"), "good").unwrap();
        symlink(inbox.join("missing.txt"), inbox.join("broken")).unwrap();

        let warehouse = fixture.protected_base.join("inbox_broken_scan");
        let mut cfg = fixture.config.clone(); // pre_transfer_integrity_scan = true (default)
        cfg.inbox.max_errors_before_fail = 1;
        let result = crate::storage::transfer::transfer_files(
            &inbox,
            &warehouse,
            &cfg,
            2,
            1,
            default_audit(),
        )
        .await;

        let err = result.expect_err("integrity scan must fail on broken symlink");
        if let TransferError::TransferFailed { errors, .. } = err {
            assert!(
                errors
                    .iter()
                    .any(|e| matches!(e, TransferError::InboxIntegrityFailed(_))),
                "expected InboxIntegrityFailed, got {:?}",
                errors
            );
        } else {
            panic!("expected TransferFailed, got {:?}", err);
        }
        // Fail-fast: nothing was copied.
        assert!(!warehouse.exists());
        assert_no_temp_dirs(fixture.protected_base.as_path());
        assert_inbox_untouched(&inbox, &["good.txt", "broken"]);
    }

    // ── 7. External symlinks ────────────────────────────────────────────

    #[tokio::test]
    async fn test_external_symlink_rejected_by_default() {
        let fixture = TestFixture::new();
        let external = tempfile::TempDir::new().unwrap();
        fs::write(external.path().join("secret.txt"), "external").unwrap();

        let inbox = fixture.inbox_base.join("inbox_external");
        fs::create_dir_all(&inbox).unwrap();
        fs::write(inbox.join("local.txt"), "local").unwrap();
        symlink(
            external.path().join("secret.txt"),
            inbox.join("ext_link"),
        )
        .unwrap();

        let warehouse = fixture.protected_base.join("inbox_external");
        let cfg = scan_off_config(&fixture); // allow_external_symlinks = false (default)
        let result = crate::storage::transfer::transfer_files(
            &inbox,
            &warehouse,
            &cfg,
            2,
            1,
            default_audit(),
        )
        .await;

        let err = result.expect_err("external symlink must be rejected");
        if let TransferError::TransferFailed { errors, .. } = err {
            assert!(
                errors
                    .iter()
                    .any(|e| matches!(e, TransferError::ExternalSymlinkNotAllowed { .. })),
                "expected ExternalSymlinkNotAllowed, got {:?}",
                errors
            );
        } else {
            panic!("expected TransferFailed, got {:?}", err);
        }
        assert_inbox_untouched(&inbox, &["local.txt", "ext_link"]);
        // External source file must never be touched.
        assert!(external.path().join("secret.txt").exists());
        assert_no_temp_dirs(fixture.protected_base.as_path());
    }

    #[tokio::test]
    async fn test_external_symlink_allowed_follows_and_preserves_source() {
        let fixture = TestFixture::new();
        let external = tempfile::TempDir::new().unwrap();
        fs::write(external.path().join("ext_file.txt"), "external content").unwrap();

        let inbox = fixture.inbox_base.join("inbox_ext_allowed");
        fs::create_dir_all(&inbox).unwrap();
        fs::write(inbox.join("local.txt"), "local").unwrap();
        symlink(
            external.path().join("ext_file.txt"),
            inbox.join("ext_link"),
        )
        .unwrap();

        let warehouse = fixture.protected_base.join("inbox_ext_allowed");
        let mut cfg = scan_off_config(&fixture);
        cfg.inbox.allow_external_symlinks = true;
        let result = crate::storage::transfer::transfer_files(
            &inbox,
            &warehouse,
            &cfg,
            2,
            1,
            default_audit(),
        )
        .await
        .expect("external symlink must be followed when allowed");

        assert!(result.success);
        // Warehouse path preserves the relative position of the link.
        assert_eq!(
            fs::read_to_string(warehouse.join("ext_link")).unwrap(),
            "external content"
        );
        // REGRESSION: the external source file must survive the transfer.
        assert!(
            external.path().join("ext_file.txt").exists(),
            "external source file was deleted — data loss"
        );
        assert!(!inbox.exists());
    }

    // ── 8. Max recursion depth ──────────────────────────────────────────

    #[tokio::test]
    async fn test_max_recursion_depth_exceeded() {
        let fixture = TestFixture::new();
        let inbox = fixture.inbox_base.join("inbox_depth");
        // root(0)/a(1)/b(2)/c(3) — depth 3 exceeds max 2.
        fs::create_dir_all(inbox.join("a/b/c")).unwrap();
        fs::write(inbox.join("a/b/c/deep.txt"), "deep").unwrap();
        fs::write(inbox.join("top.txt"), "top").unwrap();

        let warehouse = fixture.protected_base.join("inbox_depth");
        let mut cfg = scan_off_config(&fixture);
        cfg.inbox.max_recursion_depth = 2;
        let result = crate::storage::transfer::transfer_files(
            &inbox,
            &warehouse,
            &cfg,
            2,
            1,
            default_audit(),
        )
        .await;

        let err = result.expect_err("recursion depth limit must fail");
        if let TransferError::TransferFailed { errors, .. } = err {
            assert!(
                errors
                    .iter()
                    .any(|e| matches!(e, TransferError::MaxRecursionDepthExceeded(2))),
                "expected MaxRecursionDepthExceeded, got {:?}",
                errors
            );
        } else {
            panic!("expected TransferFailed, got {:?}", err);
        }
        assert_inbox_untouched(&inbox, &["top.txt", "a/b/c/deep.txt"]);
        assert_no_temp_dirs(fixture.protected_base.as_path());
    }

    // ── 9. Max symlink chain depth ──────────────────────────────────────

    #[tokio::test]
    async fn test_max_symlink_depth_exceeded() {
        let fixture = TestFixture::new();
        let inbox = fixture.inbox_base.join("inbox_chain");
        fs::create_dir_all(&inbox).unwrap();
        fs::write(inbox.join("final.txt"), "final").unwrap();

        // Chain: link_12 -> link_11 -> ... -> final.txt (12 links).
        let mut target = inbox.join("final.txt");
        for i in (0..12).rev() {
            let link = inbox.join(format!("link_{}", i + 1));
            symlink(&target, &link).unwrap();
            target = link;
        }

        let warehouse = fixture.protected_base.join("inbox_chain");
        let mut cfg = scan_off_config(&fixture);
        cfg.inbox.max_symlink_depth = Some(5);
        let result = crate::storage::transfer::transfer_files(
            &inbox,
            &warehouse,
            &cfg,
            2,
            1,
            default_audit(),
        )
        .await;

        let err = result.expect_err("symlink chain depth limit must fail");
        if let TransferError::TransferFailed { errors, .. } = err {
            assert!(
                errors
                    .iter()
                    .any(|e| matches!(e, TransferError::MaxSymlinkDepthExceeded(5))),
                "expected MaxSymlinkDepthExceeded, got {:?}",
                errors
            );
        } else {
            panic!("expected TransferFailed, got {:?}", err);
        }
        assert!(inbox.exists());
        assert_no_temp_dirs(fixture.protected_base.as_path());
    }

    // ── 10. Path length ─────────────────────────────────────────────────

    #[tokio::test]
    async fn test_path_too_long() {
        let fixture = TestFixture::new();
        let inbox = fixture.inbox_base.join("inbox_longpath");
        fs::create_dir_all(&inbox).unwrap();
        fs::write(inbox.join("file.txt"), "data").unwrap();

        let warehouse = fixture.protected_base.join("inbox_longpath");
        let mut cfg = scan_off_config(&fixture);
        cfg.inbox.max_path_length = Some(10); // any real target path exceeds this
        let result = crate::storage::transfer::transfer_files(
            &inbox,
            &warehouse,
            &cfg,
            2,
            1,
            default_audit(),
        )
        .await;

        let err = result.expect_err("path length limit must fail");
        if let TransferError::TransferFailed { errors, .. } = err {
            assert!(
                errors.iter().any(|e| matches!(e, TransferError::PathTooLong(_))),
                "expected PathTooLong, got {:?}",
                errors
            );
        } else {
            panic!("expected TransferFailed, got {:?}", err);
        }
        assert_inbox_untouched(&inbox, &["file.txt"]);
        assert_no_temp_dirs(fixture.protected_base.as_path());
    }

    // ── 11. Symlink farm ────────────────────────────────────────────────

    #[tokio::test]
    async fn test_symlink_farm_reference_limit() {
        let fixture = TestFixture::new();
        let inbox = fixture.inbox_base.join("inbox_farm");
        fs::create_dir_all(&inbox).unwrap();
        fs::write(inbox.join("target.txt"), "farm").unwrap();
        // Three symlinks to the same inode with a limit of 2.
        for i in 1..=3 {
            symlink(inbox.join("target.txt"), inbox.join(format!("farm_{}", i))).unwrap();
        }

        let warehouse = fixture.protected_base.join("inbox_farm");
        let mut cfg = scan_off_config(&fixture);
        cfg.inbox.max_symlink_references = Some(2);
        let result = crate::storage::transfer::transfer_files(
            &inbox,
            &warehouse,
            &cfg,
            2,
            1,
            default_audit(),
        )
        .await;

        let err = result.expect_err("symlink farm must be rejected");
        if let TransferError::TransferFailed { errors, .. } = err {
            assert!(
                errors
                    .iter()
                    .any(|e| matches!(e, TransferError::MaxSymlinkReferencesExceeded(_))),
                "expected MaxSymlinkReferencesExceeded, got {:?}",
                errors
            );
        } else {
            panic!("expected TransferFailed, got {:?}", err);
        }
        assert_inbox_untouched(&inbox, &["target.txt", "farm_1", "farm_2", "farm_3"]);
    }

    // ── 12. Total file/symlink limits ───────────────────────────────────

    #[tokio::test]
    async fn test_max_total_files_exceeded() {
        let fixture = TestFixture::new();
        let inbox = fixture.inbox_base.join("inbox_filelimit");
        fs::create_dir_all(&inbox).unwrap();
        for i in 1..=3 {
            fs::write(inbox.join(format!("f{}.txt", i)), "x").unwrap();
        }

        let warehouse = fixture.protected_base.join("inbox_filelimit");
        let mut cfg = scan_off_config(&fixture);
        cfg.inbox.max_total_files = Some(2);
        let result = crate::storage::transfer::transfer_files(
            &inbox,
            &warehouse,
            &cfg,
            2,
            1,
            default_audit(),
        )
        .await;

        let err = result.expect_err("total file limit must fail");
        if let TransferError::TransferFailed { errors, .. } = err {
            assert!(
                errors
                    .iter()
                    .any(|e| matches!(e, TransferError::MaxTotalFilesExceeded(2))),
                "expected MaxTotalFilesExceeded, got {:?}",
                errors
            );
        } else {
            panic!("expected TransferFailed, got {:?}", err);
        }
        assert_inbox_untouched(&inbox, &["f1.txt", "f2.txt", "f3.txt"]);
        assert_no_temp_dirs(fixture.protected_base.as_path());
    }

    #[tokio::test]
    async fn test_max_total_symlinks_exceeded() {
        let fixture = TestFixture::new();
        let inbox = fixture.inbox_base.join("inbox_symllimit");
        fs::create_dir_all(&inbox).unwrap();
        fs::write(inbox.join("t1.txt"), "1").unwrap();
        fs::write(inbox.join("t2.txt"), "2").unwrap();
        // Two distinct symlinks (distinct target inodes).
        symlink(inbox.join("t1.txt"), inbox.join("l1")).unwrap();
        symlink(inbox.join("t2.txt"), inbox.join("l2")).unwrap();

        let warehouse = fixture.protected_base.join("inbox_symllimit");
        let mut cfg = scan_off_config(&fixture);
        cfg.inbox.max_total_symlinks = Some(1);
        let result = crate::storage::transfer::transfer_files(
            &inbox,
            &warehouse,
            &cfg,
            2,
            1,
            default_audit(),
        )
        .await;

        let err = result.expect_err("total symlink limit must fail");
        if let TransferError::TransferFailed { errors, .. } = err {
            assert!(
                errors
                    .iter()
                    .any(|e| matches!(e, TransferError::MaxTotalSymlinksExceeded(1))),
                "expected MaxTotalSymlinksExceeded, got {:?}",
                errors
            );
        } else {
            panic!("expected TransferFailed, got {:?}", err);
        }
        assert_inbox_untouched(&inbox, &["l1", "l2"]);
    }

    // ── 13. Error collection threshold ──────────────────────────────────

    #[tokio::test]
    async fn test_error_threshold_exactly_n_fails() {
        let fixture = TestFixture::new();
        let inbox = fixture.inbox_base.join("inbox_thresh");
        fs::create_dir_all(&inbox).unwrap();
        fs::write(inbox.join("good.txt"), "good").unwrap();
        // Two broken symlinks.
        symlink(inbox.join("nope1"), inbox.join("broken1")).unwrap();
        symlink(inbox.join("nope2"), inbox.join("broken2")).unwrap();

        let warehouse = fixture.protected_base.join("inbox_thresh");
        let mut cfg = scan_off_config(&fixture);
        cfg.inbox.max_errors_before_fail = 2; // fails on the 2nd error
        let result = crate::storage::transfer::transfer_files(
            &inbox,
            &warehouse,
            &cfg,
            2,
            1,
            default_audit(),
        )
        .await;

        let err = result.expect_err("2nd of 2 errors must fail the transfer");
        if let TransferError::TransferFailed { errors, .. } = err {
            assert!(
                errors
                    .iter()
                    .filter(|e| matches!(e, TransferError::BrokenSymlink(_)))
                    .count()
                    >= 1,
                "expected BrokenSymlink errors, got {:?}",
                errors
            );
        } else {
            panic!("expected TransferFailed, got {:?}", err);
        }
        assert_inbox_untouched(&inbox, &["good.txt", "broken1", "broken2"]);
        assert_no_temp_dirs(fixture.protected_base.as_path());
    }

    #[tokio::test]
    async fn test_sub_threshold_errors_do_not_fail_transfer() {
        let fixture = TestFixture::new();
        let inbox = fixture.inbox_base.join("inbox_subthresh");
        fs::create_dir_all(&inbox).unwrap();
        fs::write(inbox.join("good.txt"), "good").unwrap();
        symlink(inbox.join("nope1"), inbox.join("broken1")).unwrap();
        symlink(inbox.join("nope2"), inbox.join("broken2")).unwrap();

        let warehouse = fixture.protected_base.join("inbox_subthresh");
        let mut cfg = scan_off_config(&fixture);
        cfg.inbox.max_errors_before_fail = 5; // 2 errors < 5 → keep going
        let result = crate::storage::transfer::transfer_files(
            &inbox,
            &warehouse,
            &cfg,
            2,
            1,
            default_audit(),
        )
        .await
        .expect("sub-threshold errors must not fail the transfer");

        // The healthy file transferred; the broken symlinks were reported
        // but did not abort the run.
        assert!(result.success);
        assert_eq!(result.files_transferred.len(), 1);
        assert_eq!(fs::read_to_string(warehouse.join("good.txt")).unwrap(), "good");
        assert_eq!(
            result.errors.len(),
            2,
            "both broken symlinks must be reported in result.errors"
        );
        assert!(result.errors.iter().all(|e| matches!(e, TransferError::BrokenSymlink(_))));
        // Success still cleans up the inbox.
        assert!(!inbox.exists());
    }

    // ── 14. Non-atomic rollback ─────────────────────────────────────────

    #[tokio::test]
    async fn test_non_atomic_rollback_on_failure() {
        let fixture = TestFixture::new();
        let inbox = fixture.inbox_base.join("inbox_nonatomic");
        fs::create_dir_all(&inbox).unwrap();
        fs::write(inbox.join("a.txt"), "aaa").unwrap();
        fs::write(inbox.join("b.txt"), "bbb").unwrap();
        symlink(inbox.join("gone"), inbox.join("broken")).unwrap();

        let warehouse = fixture.protected_base.join("inbox_nonatomic");
        let mut cfg = scan_off_config(&fixture);
        cfg.inbox.atomic_transfer = false;
        let result = crate::storage::transfer::transfer_files(
            &inbox,
            &warehouse,
            &cfg,
            2,
            1,
            default_audit(),
        )
        .await;

        assert!(result.is_err(), "broken symlink must still fail non-atomic transfer");
        // Rollback: no partial data in the warehouse.
        let files: Vec<String> = if warehouse.exists() {
            fs::read_dir(&warehouse)
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        } else {
            vec![]
        };
        assert!(
            files.is_empty(),
            "warehouse must have no partial data after non-atomic rollback, found {:?}",
            files
        );
        assert_inbox_untouched(&inbox, &["a.txt", "b.txt", "broken"]);
    }

    // ── 15. Warehouse path validation ───────────────────────────────────

    #[tokio::test]
    async fn test_warehouse_path_outside_protected_base_rejected() {
        let fixture = TestFixture::new();
        let inbox = fixture.inbox_base.join("inbox_badwh");
        fs::create_dir_all(&inbox).unwrap();
        fs::write(inbox.join("f.txt"), "f").unwrap();

        let outside = tempfile::TempDir::new().unwrap();
        let warehouse = outside.path().join("warehouse");
        let cfg = scan_off_config(&fixture);
        let result = crate::storage::transfer::transfer_files(
            &inbox,
            &warehouse,
            &cfg,
            2,
            1,
            default_audit(),
        )
        .await;

        assert!(
            result.is_err(),
            "warehouse outside protected base must be rejected"
        );
        assert_inbox_untouched(&inbox, &["f.txt"]);
    }

    #[tokio::test]
    async fn test_validate_warehouse_path_direct() {
        let fixture = TestFixture::new();
        let base = fixture.protected_base.clone();

        // Inside, not yet existing.
        let inside = base.join("new_wh");
        let canonical = validate_warehouse_path(&inside, &base).await.unwrap();
        assert!(canonical.starts_with(fs::canonicalize(&base).unwrap()));

        // Inside, existing.
        let existing = base.join("existing_wh");
        fs::create_dir_all(&existing).unwrap();
        assert!(validate_warehouse_path(&existing, &base).await.is_ok());

        // Outside.
        let outside = tempfile::TempDir::new().unwrap();
        let outside_wh = outside.path().join("w");
        assert!(
            matches!(
                validate_warehouse_path(&outside_wh, &base).await,
                Err(TransferError::InvalidWarehousePath(_))
            )
        );

        // Nonexistent protected base → error.
        assert!(
            validate_warehouse_path(&inside, &base.join("no_such_base")).await.is_err()
        );
    }

    // ── 16. Disk space check ────────────────────────────────────────────

    #[test]
    fn test_check_disk_space_happy_path() {
        let dir = tempfile::tempdir().unwrap();
        let available = check_disk_space(dir.path(), 0).unwrap();
        assert!(available > 0, "available space must be positive");
    }

    // ── 17. Temp dir name uniqueness ────────────────────────────────────

    #[test]
    fn test_temp_transfer_dir_name_unique() {
        let warehouse = Path::new("/tmp/parent/warehouse");
        let a = temp_transfer_dir_name(warehouse);
        let b = temp_transfer_dir_name(warehouse);
        assert_ne!(a, b, "temp dir names must differ between calls");
        assert!(a.parent().unwrap() == Path::new("/tmp/parent"));
        assert!(
            a.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("warehouse.transfer-")
        );
    }

    // ── 18. Symlink target validation ───────────────────────────────────

    #[test]
    fn test_validate_symlink_target() {
        assert!(validate_symlink_target(Path::new("sub/file.txt")).is_ok());
        assert!(validate_symlink_target(Path::new("/abs/path.txt")).is_ok());
        assert!(validate_symlink_target(Path::new("../escape")).is_err());
        assert!(validate_symlink_target(Path::new("a/../../b")).is_err());
        // Control character.
        use std::os::unix::ffi::OsStringExt;
        let with_ctrl = std::ffi::OsString::from_vec(vec![b'f', 1, b'i', b'l', b'e']);
        assert!(validate_symlink_target(Path::new(&with_ctrl)).is_err());
    }

    // ── 19. External detection ──────────────────────────────────────────

    #[test]
    fn test_is_external_to_inbox() {
        let root = tempfile::tempdir().unwrap();
        let inbox = root.path().join("inbox");
        fs::create_dir_all(&inbox).unwrap();
        let external = root.path().join("external");
        fs::create_dir_all(&external).unwrap();

        let inbox_c = fs::canonicalize(&inbox).unwrap();
        fs::create_dir_all(inbox.join("sub")).unwrap();
        let inside_c = fs::canonicalize(inbox.join("sub")).unwrap();
        let outside_c = fs::canonicalize(&external).unwrap();

        assert!(!is_external_to_inbox(&inside_c, &inbox_c));
        assert!(!is_external_to_inbox(&inbox_c, &inbox_c));
        assert!(is_external_to_inbox(&outside_c, &inbox_c));
    }

    // ── 20. Symlink reference counting ──────────────────────────────────

    #[test]
    fn test_count_symlink_reference() {
        let mut map = HashMap::new();
        assert_eq!(count_symlink_reference(&mut map, 1, 100, Some(2)).unwrap(), 1);
        assert_eq!(count_symlink_reference(&mut map, 1, 100, Some(2)).unwrap(), 2);
        assert!(matches!(
            count_symlink_reference(&mut map, 1, 100, Some(2)),
            Err(TransferError::MaxSymlinkReferencesExceeded(100))
        ));
        // No limit → never errors.
        let mut unlim = HashMap::new();
        for _ in 0..100 {
            count_symlink_reference(&mut unlim, 2, 200, None).unwrap();
        }
    }

    // ── 21. TransferContext cancellation semantics ──────────────────────

    #[test]
    fn test_transfer_context_record_error() {
        let ctx = TransferContext::new(2);
        assert!(!ctx.is_cancelled());
        assert!(!ctx.record_error(), "1st of 2 errors must not cancel");
        assert!(!ctx.is_cancelled());
        assert!(ctx.record_error(), "2nd of 2 errors must cancel");
        assert!(ctx.is_cancelled());
        assert_eq!(ctx.error_count(), 2);

        let single = TransferContext::new(1);
        assert!(single.record_error(), "Nth error with N=1 cancels immediately");
    }

    #[tokio::test]
    async fn test_transfer_context_token_cancellation_future() {
        let ctx = TransferContext::new(1);
        let token = ctx.cancel_token();
        ctx.record_error();
        token.cancelled().await; // must resolve after cancellation
    }

    // ── 22. Constant-time comparison ────────────────────────────────────

    #[test]
    fn test_ct_eq() {
        use crate::handlers::job_callback::ct_eq;
        assert!(ct_eq(b"secret", b"secret"));
        assert!(!ct_eq(b"secret", b"secret2"));
        assert!(!ct_eq(b"secret", b"screret"));
        assert!(!ct_eq(b"", b"x"));
        assert!(ct_eq(b"", b""));
    }

    // ── 24. Parallel ordering / many files ──────────────────────────────

    #[tokio::test]
    async fn test_parallel_many_files_no_races() {
        let fixture = TestFixture::new();
        let inbox = fixture.inbox_base.join("inbox_parallel");
        fs::create_dir_all(&inbox).unwrap();
        let mut total_bytes = 0u64;
        for i in 0..50 {
            let dir = inbox.join(format!("d{}", i % 5));
            fs::create_dir_all(&dir).unwrap();
            let content = format!("file {} content", i);
            fs::write(dir.join(format!("f{}.txt", i)), &content).unwrap();
            total_bytes += content.len() as u64;
        }

        let warehouse = fixture.protected_base.join("inbox_parallel");
        let cfg = scan_off_config(&fixture);
        let result = crate::storage::transfer::transfer_files(
            &inbox,
            &warehouse,
            &cfg,
            8,
            1,
            default_audit(),
        )
        .await
        .expect("parallel transfer of 50 files should succeed");

        assert!(result.success);
        assert_eq!(result.files_transferred.len(), 50);
        assert_eq!(result.total_bytes, total_bytes);
        for i in 0..50 {
            let expected = format!("file {} content", i);
            let got = fs::read_to_string(warehouse.join(format!("d{}", i % 5)).join(format!("f{}.txt", i)))
                .unwrap_or_else(|_| panic!("missing file for i={}", i));
            assert_eq!(got, expected);
        }
        assert!(!inbox.exists());
    }

    // ── 25. Integrity scan (pre-transfer, fail fast) ────────────────────

    #[tokio::test]
    async fn test_integrity_scan_fails_on_external_symlink() {
        let fixture = TestFixture::new();
        let external = tempfile::TempDir::new().unwrap();
        fs::write(external.path().join("e.txt"), "e").unwrap();

        let inbox = fixture.inbox_base.join("inbox_scan_ext");
        fs::create_dir_all(&inbox).unwrap();
        fs::write(inbox.join("ok.txt"), "ok").unwrap();
        symlink(external.path().join("e.txt"), inbox.join("ext")).unwrap();

        let warehouse = fixture.protected_base.join("inbox_scan_ext");
        let mut cfg = fixture.config.clone(); // scan ON (default)
        cfg.inbox.allow_external_symlinks = false;
        let result = crate::storage::transfer::transfer_files(
            &inbox,
            &warehouse,
            &cfg,
            2,
            1,
            default_audit(),
        )
        .await;

        let err = result.expect_err("scan must reject external symlink pre-transfer");
        if let TransferError::TransferFailed { ref errors, ref audit_events } = err {
            assert!(
                errors
                    .iter()
                    .any(|e| matches!(e, TransferError::InboxIntegrityFailed(_))),
                "expected InboxIntegrityFailed, got {:?}",
                errors
            );
            // Audit trail present (plan §12).
            assert!(
                audit_events.iter().any(|ev| {
                    matches!(
                        ev.event_type,
                        crate::storage::transfer::SecurityEventType::InboxIntegrityCheckFailed
                    )
                }),
                "expected InboxIntegrityCheckFailed audit event"
            );
        } else {
            panic!("expected TransferFailed, got {:?}", err);
        }
        assert!(!warehouse.exists());
        assert_inbox_untouched(&inbox, &["ok.txt", "ext"]);
    }

    #[tokio::test]
    async fn test_integrity_scan_passes_clean_tree() {
        let fixture = TestFixture::new();
        let inbox = fixture.inbox_base.join("inbox_scan_ok");
        fs::create_dir_all(inbox.join("s")).unwrap();
        fs::write(inbox.join("root.txt"), "r").unwrap();
        fs::write(inbox.join("s/inner.txt"), "i").unwrap();
        symlink(inbox.join("root.txt"), inbox.join("s/link.txt")).unwrap();

        let warehouse = fixture.protected_base.join("inbox_scan_ok");
        let cfg = fixture.config.clone(); // scan ON
        let result = crate::storage::transfer::transfer_files(
            &inbox,
            &warehouse,
            &cfg,
            2,
            1,
            default_audit(),
        )
        .await
        .expect("clean tree must pass the integrity scan");

        assert!(result.success);
        assert_eq!(result.files_transferred.len(), 3);
        assert!(!inbox.exists());
    }

    // ── 26. Atomic success: warehouse committed, no temp leftovers ──────

    #[tokio::test]
    async fn test_atomic_success_commit() {
        let fixture = TestFixture::new();
        let inbox = fixture.inbox_base.join("inbox_atomic_ok");
        fs::create_dir_all(&inbox).unwrap();
        fs::write(inbox.join("x.txt"), "xyz").unwrap();

        let warehouse = fixture.protected_base.join("inbox_atomic_ok");
        let cfg = fixture.config.clone(); // atomic_transfer = true (default)
        let result = crate::storage::transfer::transfer_files(
            &inbox,
            &warehouse,
            &cfg,
            2,
            1,
            default_audit(),
        )
        .await
        .expect("atomic transfer should succeed");

        assert!(result.success);
        assert!(warehouse.is_dir());
        assert_eq!(fs::read_to_string(warehouse.join("x.txt")).unwrap(), "xyz");
        assert_no_temp_dirs(fixture.protected_base.as_path());
        assert!(!inbox.exists());
    }

    /// Pre-existing non-empty warehouse dir must be rejected (atomic commit
    /// would clobber it).
    #[tokio::test]
    async fn test_atomic_preexisting_nonempty_warehouse_rejected() {
        let fixture = TestFixture::new();
        let inbox = fixture.inbox_base.join("inbox_atomic_pre");
        fs::create_dir_all(&inbox).unwrap();
        fs::write(inbox.join("x.txt"), "xyz").unwrap();

        let warehouse = fixture.protected_base.join("inbox_atomic_pre");
        fs::create_dir_all(&warehouse).unwrap();
        fs::write(warehouse.join("preexisting.txt"), "keep").unwrap();

        let cfg = fixture.config.clone(); // atomic
        let result = crate::storage::transfer::transfer_files(
            &inbox,
            &warehouse,
            &cfg,
            2,
            1,
            default_audit(),
        )
        .await;
        assert!(
            result.is_err(),
            "commit must not clobber a non-empty warehouse path"
        );
        // Pre-existing data intact.
        assert_eq!(
            fs::read_to_string(warehouse.join("preexisting.txt")).unwrap(),
            "keep"
        );
    }

    // ── 27. InboxConfig defaults ────────────────────────────────────────

    #[test]
    fn test_inbox_config_security_defaults() {
        let cfg = InboxConfig::default();
        assert!(!cfg.allow_external_symlinks, "external symlinks must be denied by default");
        assert_eq!(cfg.max_recursion_depth, 25);
        assert_eq!(cfg.max_errors_before_fail, 1);
        assert_eq!(cfg.max_path_length, Some(4096));
        assert_eq!(cfg.max_total_files, Some(100_000));
        assert_eq!(cfg.max_total_symlinks, Some(1_000));
        assert_eq!(cfg.max_symlink_depth, Some(10));
        assert_eq!(cfg.max_symlink_references, Some(10));
        assert!(cfg.atomic_transfer, "atomic transfer must be on by default");
        assert!(
            cfg.pre_transfer_integrity_scan,
            "integrity scan must be on by default"
        );
    }
}
/// Queue rate-limit quota tests (P4.2, D8)
#[cfg(test)]
mod queue_quota_tests {
    use crate::handlers::publish_inbox::quota_exceeded;

    #[test]
    fn test_quota_below_limit_allows() {
        assert!(!quota_exceeded(9, 10));
        assert!(!quota_exceeded(0, 10));
    }

    #[test]
    fn test_quota_at_limit_rejects() {
        assert!(quota_exceeded(10, 10));
    }

    #[test]
    fn test_quota_above_limit_rejects() {
        assert!(quota_exceeded(11, 10));
        assert!(quota_exceeded(999, 20));
    }
}
