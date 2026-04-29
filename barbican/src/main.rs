
use std::{sync::Arc, time::Duration};

use axum::{
    Json, Router, extract::{FromRequest, MatchedPath, Path, Request, State, rejection::JsonRejection}, http::StatusCode, middleware::{Next, from_fn}, response::{IntoResponse, Response}, routing::{delete, get, post, put}
};
use common::{db_collection, db_setup, logging_setup, router_logging_layer, shutdown_signal};
use mongodb::{
    Client, Collection, bson::{doc, oid::ObjectId}, results::{DeleteResult, InsertOneResult, UpdateResult}
};
use serde::{Deserialize, Serialize};
use serde_aux::prelude::*;
use tokio::{signal};
use tower_http::{timeout::TimeoutLayer, trace::TraceLayer};
use tracing::warn;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};


#[tokio::main]
async fn main() {
    logging_setup(env!("CARGO_CRATE_NAME"));

    // get db
    let client = db_setup().await;

    // run it
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .unwrap();
    tracing::info!("Listening on {}", listener.local_addr().unwrap());
    let _ = axum::serve(listener, app(client))
        .with_graceful_shutdown(shutdown_signal())
        .await;
}

// defining routes and state
fn app(client: Client) -> Router {
    let collection: Collection<InboxDB> = db_collection(client, "inbox");

    let mut router = Router::new()
        .route("/api/v1/inbox", post(create_inbox))
        .route("/api/v1/inbox/{id}", get(get_inbox));
    router = router_logging_layer(router);
    router.layer(from_fn(log_app_errors))
        .with_state(collection)
}

// defining Member type
#[derive(Debug, Deserialize, Serialize)]
struct InboxDB {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    id: Option<ObjectId>,
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default = "bool_true")]
    active: bool,
}

// defining Member type
#[derive(Debug, Deserialize, Serialize)]
struct CreateInbox {
    name: String,
    size: u64,
}

// Create our own JSON extractor by wrapping `axum::Json`. This makes it easy to override the
// rejection and provide our own which formats errors to match our application.
//
// `axum::Json` responds with plain text if the input is invalid.
#[derive(FromRequest)]
#[from_request(via(axum::Json), rejection(AppError))]
struct AppJson<T>(T);

impl<T> IntoResponse for AppJson<T>
where
    axum::Json<T>: IntoResponse,
{
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

// handler to create a new inbox
async fn create_inbox(
    State(db): State<Collection<InboxDB>>,
    Json(input): Json<CreateInbox>,
) -> Result<Json<InsertOneResult>, (StatusCode, String)> {
    let inbox = InboxDB{
        id: None,
        name: input.name,
        size: input.size,
        active: true,
    };
    let result = db.insert_one(inbox).await.map_err(internal_error)?;

    Ok(Json(result))
}

// handler to get inbox
async fn get_inbox(
    State(db): State<Collection<InboxDB>>,
    Path(id): Path<ObjectId>,
) -> Result<Json<InboxDB>, (StatusCode, String)> {
    let result = db.find_one(doc! { "_id": id }).await.map_err(internal_error)?;

    match result {
        Some(x) => Ok(Json(x)),
        None => Err((StatusCode::NOT_FOUND, "Invalid inbox id".to_string()))
    }
}

fn internal_error<E>(err: E) -> (StatusCode, String)
where
    E: std::error::Error,
{
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}

// The kinds of errors we can hit in our application.
#[derive(Debug)]
enum AppError {
    // The request body contained invalid JSON
    JsonRejection(JsonRejection),
}

// Tell axum how `AppError` should be converted into a response.
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        // How we want errors responses to be serialized
        #[derive(Serialize)]
        struct ErrorResponse {
            message: String,
        }

        let (status, message, err) = match &self {
            AppError::JsonRejection(rejection) => {
                // This error is caused by bad user input so don't log it
                (rejection.status(), rejection.body_text(), None)
            }
        };

        let mut response = (status, AppJson(ErrorResponse { message })).into_response();
        if let Some(err) = err {
            // Insert our error into the response, our logging middleware will use this.
            // By wrapping the error in an Arc we can use it as an Extension regardless of any inner types not deriving Clone.
            response.extensions_mut().insert(Arc::<AppError>::new(err));
        }
        response
    }
}

impl From<JsonRejection> for AppError {
    fn from(rejection: JsonRejection) -> Self {
        Self::JsonRejection(rejection)
    }
}

// Our middleware is responsible for logging error details internally
async fn log_app_errors(request: Request, next: Next) -> Response {
    let response = next.run(request).await;
    // If the response contains an AppError Extension, log it.
    if let Some(err) = response.extensions().get::<Arc<AppError>>() {
        tracing::error!(?err, "an unexpected error occurred inside a handler");
    }
    response
}


#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_with_axum_test() {
        let client = db_setup().await;
        let server = axum_test::TestServer::new(app(client)).unwrap();

        let response = server.get("/").await;
        response.assert_status_ok();
        response.assert_text("Hello");
    }
}
