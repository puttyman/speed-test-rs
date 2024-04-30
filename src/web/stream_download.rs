use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::Router;

async fn stream_download() -> impl IntoResponse {
    Html("hello")
}

pub fn routes() -> Router {
    Router::new().route("/api/stream", get(stream_download))
}
