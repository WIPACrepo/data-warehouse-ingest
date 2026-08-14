use barbican::serve;
use common::{db_setup, logging_setup, start_metrics_server};

#[tokio::main]
async fn main() {
    logging_setup(env!("CARGO_CRATE_NAME"));

    // set up prometheus
    start_metrics_server().await;

    // get db
    let client = db_setup().await;

    // run it
    serve(client, "127.0.0.1:3000").await
}
