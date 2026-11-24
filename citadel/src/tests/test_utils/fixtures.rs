//! Test fixtures for Citadel

use crate::config::CitadelConfig;
use crate::jobs::executor::MockExecutor;
use crate::storage::manager::StorageManager;
use std::path::PathBuf;
use tempfile::TempDir;

/// A test fixture that provides temporary directories and mock dependencies
pub struct TestFixture {
    pub temp_dir: TempDir,
    pub config: CitadelConfig,
    pub inbox_base: PathBuf,
    pub protected_base: PathBuf,
    pub secret_file: PathBuf,
}

impl Default for TestFixture {
    fn default() -> Self {
        Self::new()
    }
}

impl TestFixture {
    /// Create a new test fixture with temporary directories
    pub fn new() -> Self {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let inbox_base = temp_dir.path().join("inboxes");
        let protected_base = temp_dir.path().join("protected");
        let secret_file = temp_dir.path().join("callback-secret");

        // Create directories
        std::fs::create_dir_all(&inbox_base).ok();
        std::fs::create_dir_all(&protected_base).ok();

        // Write a test secret
        std::fs::write(&secret_file, "test-secret-12345").ok();

        let config = CitadelConfig {
            bind_address: "127.0.0.1:0".to_string(), // random port
            inbox_base_path: inbox_base.clone(),
            protected_base_path: protected_base.clone(),
            job_callback_secret_file: secret_file.clone(),
            job_executor: crate::config::JobExecutorType::Mock,
            mongodb_uri: "mongodb://localhost:27017".to_string(),
            mongodb_database: "citadel_test".to_string(),
            barbican_api_url: "http://localhost:9999".to_string(),
            ..CitadelConfig::default()
        };

        Self {
            temp_dir,
            config,
            inbox_base,
            protected_base,
            secret_file,
        }
    }

    /// Create a mock executor from this fixture's config
    pub fn mock_executor(&self) -> MockExecutor {
        MockExecutor::new(self.config.clone())
    }

    /// Create a storage manager from this fixture's config
    pub fn storage_manager(&self) -> StorageManager {
        StorageManager::new(&self.config)
    }
}

impl Drop for TestFixture {
    fn drop(&mut self) {
        // TempDir is cleaned up automatically
    }
}
