use std::str::FromStr;
use tokio::signal;
use tracing::{Level, error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// Set up logging for the application
pub fn logging_setup(crate_name: &str) {
    let mut validation_errors = Vec::new();

    // Define the parsing logic
    let mut get_level = |key: &str, default: &str| {
        let raw = std::env::var(key).unwrap_or_default();
        if raw.is_empty() {
            return default.to_string();
        }
        match Level::from_str(&raw.to_lowercase()) {
            Ok(level) => level.to_string(),
            Err(_) => {
                validation_errors.push(format!(
                    "Invalid log level '{}' for {}; falling back to '{}'",
                    raw, key, default
                ));
                default.to_string()
            }
        }
    };

    // Capture the resolved levels so we can log them later
    let overall = get_level("THIRD_PARTY_LOG_LEVEL", "warn");
    let http = get_level("HTTP_LOG_LEVEL", "debug");
    let crate_level = get_level("LOG_LEVEL", "info");
    let common_crate_name = env!("CARGO_CRATE_NAME");

    let filter_str = format!(
        "{},tower_http={},{}={},{}={}",
        overall, http, common_crate_name, crate_level, crate_name, crate_level
    );

    // Initialize
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| filter_str.clone().into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Log the outcome
    for err in validation_errors {
        error!("{}", err);
    }

    info!("Logging initialized: global={}", filter_str);
}

/// Set up shutdown signal handler for graceful shutdown
pub async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("Shutting down");
}
