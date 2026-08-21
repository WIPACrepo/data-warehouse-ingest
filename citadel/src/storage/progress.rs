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