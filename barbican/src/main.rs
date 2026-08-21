use barbican::{Config, serve};
use common::{db_setup_with_shutdown, logging_setup, shutdown_signal, start_metrics_server};
use futures::future::Shared;
use futures::FutureExt;
use tracing::error;

#[tokio::main]
async fn main() {
    logging_setup(env!("CARGO_CRATE_NAME"));

    // Create shutdown signal early so Ctrl+C works during startup
    let shutdown_fut = shutdown_signal();
    let shutdown_shared: Shared<_> = shutdown_fut.shared();

    // Load configuration early so we can use it for metrics and database
    let config = Config::from_env().expect("Failed to load configuration");

    // Set up prometheus metrics server
    if let Err(e) = start_metrics_server(config.prometheus_port).await {
        error!("Failed to start metrics server: {e}");
    }

    // Connect to database with shutdown support
    let shutdown_for_db = shutdown_shared.clone();
    let client = match db_setup_with_shutdown(
        &config.mongodb_uri,
        &config.mongodb_database,
        move || shutdown_for_db.clone(),
    )
    .await
    {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to connect to database: {e}");
            return;
        }
    };

    // Run the service
    serve(client, config).await
}