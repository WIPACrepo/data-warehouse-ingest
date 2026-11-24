//! Callback secret hot-reload
//!
//! Watches the callback secret file for changes and reloads atomically
//! without requiring a service restart.

use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

/// Load a callback secret from a file
pub fn load_callback_secret(path: &PathBuf) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .map(|s| if s.is_empty() { None } else { Some(s) })?
}

/// Start a background watcher that reloads the callback secret when the file changes.
///
/// Uses `notify` crate to watch for file modifications. On change, reads the new
/// content atomically into the shared `Arc<RwLock<Option<String>>>`.
pub async fn start_secret_watcher(secret_file: PathBuf, secret_store: Arc<RwLock<Option<String>>>) {
    let secret_file_for_closure = secret_file.clone();
    let mut watcher =
        match notify::recommended_watcher(move |res: notify::Result<notify::Event>| match res {
            Ok(event) => {
                if event.kind.is_modify() || event.kind.is_access() {
                    info!("Secret file changed, reloading: {:?}", event.paths);
                    let secret = load_callback_secret(&secret_file_for_closure);
                    let mut store = secret_store.blocking_write();
                    match secret {
                        Some(s) if !s.is_empty() => {
                            *store = Some(s);
                            info!("Callback secret reloaded successfully");
                        }
                        _ => {
                            warn!("Callback secret file is empty or unreadable");
                        }
                    }
                }
            }
            Err(e) => {
                error!("Secret watcher error: {}", e);
            }
        }) {
            Ok(w) => w,
            Err(e) => {
                error!("Failed to create secret file watcher: {}", e);
                return;
            }
        };

    // Watch the parent directory for the secret file
    let watch_path = secret_file.parent().unwrap_or(&secret_file);
    if let Err(e) = notify::Watcher::watch(
        &mut watcher,
        watch_path,
        notify::RecursiveMode::NonRecursive,
    ) {
        error!(
            "Failed to watch secret file directory {:?}: {}",
            watch_path, e
        );
        return;
    }

    info!("Secret file watcher started for {:?}", secret_file);

    // Keep the watcher alive forever
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
    }
}
