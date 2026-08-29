//! Progress tracking for file transfers

use chrono::Utc;
use crate::jobs::types::JobProgress;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Progress tracker for file transfers
pub struct ProgressTracker {
    progress: Arc<RwLock<JobProgress>>,
}

impl ProgressTracker {
    pub fn new(total_files: usize, _update_interval_secs: u64) -> Self {
        Self {
            progress: Arc::new(RwLock::new(JobProgress {
                total_files,
                processed_files: 0,
                total_bytes: 0,
                processed_bytes: 0,
                phase: "initializing".to_string(),
                last_updated: Utc::now(),
            })),
        }
    }
    
    pub async fn update_phase(&self, phase: &str) {
        let mut progress = self.progress.write().await;
        progress.phase = phase.to_string();
        progress.last_updated = Utc::now();
    }

    /// Update the known total file count once the transfer determines it
    /// (e.g. from the pre-transfer integrity scan).
    pub async fn set_total_files(&self, total_files: usize) {
        let mut progress = self.progress.write().await;
        progress.total_files = total_files;
        progress.last_updated = Utc::now();
    }
    
    pub async fn increment_files(&self, count: usize, bytes: u64) {
        let mut progress = self.progress.write().await;
        progress.processed_files += count;
        progress.processed_bytes += bytes;
        progress.last_updated = Utc::now();
    }
    
    pub async fn get_progress(&self) -> JobProgress {
        self.progress.read().await.clone()
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_progress_tracker_new_defaults() {
        let tracker = ProgressTracker::new(100, 30);
        let p = tracker.get_progress().await;
        assert_eq!(p.total_files, 100);
        assert_eq!(p.processed_files, 0);
        assert_eq!(p.processed_bytes, 0);
        assert_eq!(p.total_bytes, 0);
        assert_eq!(p.phase, "initializing");
    }

    #[tokio::test]
    async fn test_progress_tracker_increment_files() {
        let tracker = ProgressTracker::new(2, 30);
        tracker.increment_files(1, 10).await;
        tracker.increment_files(1, 22).await;
        let p = tracker.get_progress().await;
        assert_eq!(p.processed_files, 2);
        assert_eq!(p.processed_bytes, 32);
    }

    #[tokio::test]
    async fn test_progress_tracker_update_phase() {
        let tracker = ProgressTracker::new(0, 30);
        tracker.update_phase("transferring").await;
        assert_eq!(tracker.get_progress().await.phase, "transferring");
        tracker.update_phase("committed").await;
        assert_eq!(tracker.get_progress().await.phase, "committed");
    }

    #[tokio::test]
    async fn test_progress_tracker_set_total_files() {
        let tracker = ProgressTracker::new(0, 30);
        // Discovered by the pre-transfer integrity scan after construction.
        tracker.set_total_files(57).await;
        assert_eq!(tracker.get_progress().await.total_files, 57);
    }
}
