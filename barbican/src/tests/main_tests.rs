use std::sync::Arc;

use crate::{create_app, test_utils::create_test_store};

#[tokio::test]
async fn test_with_axum_test() {
    let store = Arc::new(create_test_store());
    let router = create_app(store);

    let server = axum_test::TestServer::new(router).unwrap();

    let response = server.get("/").await;
    response.assert_status_not_found();
}
