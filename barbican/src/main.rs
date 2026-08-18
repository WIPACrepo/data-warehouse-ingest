use barbican::{Config, serve};
use common::{db_setup, logging_setup, start_metrics_server};
use tracing::error;

#[tokio::main]
async fn main() {
    logging_setup(env!("CARGO_CRATE_NAME"));

    // Load configuration early so we can use it for metrics and database
    let config = Config::from_env().expect("Failed to load configuration");

    // Set up prometheus metrics server
    if let Err(e) = start_metrics_server(config.prometheus_port).await {
        error!("Failed to start metrics server: {e}");
    }

    // Connect to database using config values
    let client = match db_setup(&config.mongodb_uri, &config.mongodb_database).await {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to connect to database: {e}");
            return;
        }
    };

    // Run the service
    serve(client, config).await
}