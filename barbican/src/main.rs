use barbican::{AuthConfig, Config, serve};
use common::{db_setup_with_shutdown, logging_setup, shutdown_signal};
use futures::FutureExt;
use futures::future::Shared;
use tracing::error;

#[tokio::main]
async fn main() {
    logging_setup(env!("CARGO_CRATE_NAME"));

    // Create shutdown signal early so Ctrl+C works during startup
    let shutdown_fut = shutdown_signal();
    let shutdown_shared: Shared<_> = shutdown_fut.shared();

    // Load configuration early so we can use it for metrics and database
    let config = Config::from_env().expect("Failed to load configuration");
    // Auth settings come from BARBICAN_KEYCLOAK_*, BARBICAN_REQUIRED_ROLES, ...
    // (decision D10). Fail fast, before connecting to MongoDB.
    let auth_config = match AuthConfig::from_env_prefixed("BARBICAN") {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to load auth configuration: {e}");
            std::process::exit(1);
        }
    };

    // Connect to database with shutdown support
    let shutdown_for_db = shutdown_shared.clone();
    let client =
        match db_setup_with_shutdown(&config.mongodb_uri, &config.mongodb_database, move || {
            shutdown_for_db.clone()
        })
        .await
        {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to connect to database: {e}");
                return;
            }
        };

    // Run the service. serve() builds the store and AppState (which owns the
    // Barbican metrics registry and the inbox-count refresh loop) and starts
    // the metrics server on the configured port via
    // start_metrics_server_with_router so the Barbican registry is rendered.
    serve(client, config, auth_config).await
}
