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

        let protected_path = fixture.protected_base.join(&inbox_id.to_string());
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
        let inbox_path = fixture.inbox_base.join(&inbox_id.to_string());
        std::fs::create_dir_all(&inbox_path).unwrap();

        let test_file = inbox_path.join("test.txt");
        std::fs::write(&test_file, "hello world").unwrap();

        let protected_path = fixture.protected_base.join(&inbox_id.to_string());
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
        let inbox_path = fixture.inbox_base.join(&inbox_id.to_string());
        std::fs::create_dir_all(&inbox_path).unwrap();

        std::fs::write(inbox_path.join("file1.txt"), "aaa").unwrap();
        std::fs::write(inbox_path.join("file2.txt"), "bbbb").unwrap();
        std::fs::write(inbox_path.join("file3.txt"), "ccccc").unwrap();

        let protected_path = fixture.protected_base.join(&inbox_id.to_string());
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