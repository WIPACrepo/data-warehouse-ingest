use barbican::serve;
use common::{logging_setup, start_metrics_server, db_setup};

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