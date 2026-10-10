use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

#[derive(Serialize)]
struct Health {
    status: &'static str,
    version: &'static str,
}

/// `GET /api/health` - liveness probe used by the Docker HEALTHCHECK.
async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

/// Routes that sit outside the Leptos app. Generic over the state so they can be
/// merged into the main router before `.with_state(...)` is applied.
pub fn routes<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new().route("/api/health", get(health))
}
