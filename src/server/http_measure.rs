//! HTTP measurement endpoints and the API auth layer (SPEC §5.8, §6.2, §11).
//!
//! - `GET /api/info`: server name, version, limits, the client IP as seen by the server.
//! - `GET /api/__down?bytes=N[&session=<uuid>]`: N random bytes (`bytes=0` is the latency
//!   probe), with `Server-Timing: app;dur=<ms before the first byte>`.
//! - `POST /api/__up[?session=<uuid>]`: reads and discards the body, returns `{"bytes":N}`
//!   with `Server-Timing: app;dur=<ms reading the body>`.
//!
//! With `session`, the bytes are counted into that session's counters (live rates and
//! overlap, §5.11) and the request counts toward `max_sessions` (503 when busy).
//!
//! [`require_token`] protects `/api/*` (except `/api/health`) when `SPEEDTEST_AUTH_TOKEN`
//! is set. The routes of [`routes`] include the WebSockets of [`super::ws`], which check the
//! token in their `Hello` instead.

use std::convert::Infallible;
use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Query, Request, State};
use axum::http::header::{
    AUTHORIZATION, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, COOKIE, WWW_AUTHENTICATE,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::time::Instant;
use uuid::Uuid;

use super::config::Config;
use super::sessions::{Rejection, TransferGuard};
use super::state::AppState;
use super::ws::{self, CHUNK_SIZE};
use crate::model::protocol::{ErrorCode, Limits, PROTOCOL_VERSION};

/// Name of the cookie the UI stores the token in.
pub const TOKEN_COOKIE: &str = "speedtest_token";

/// All measurement routes: `/api/info`, `/api/__down`, `/api/__up` and the `/ws/*` sockets.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/info", get(info))
        .route("/api/__down", get(down))
        .route(
            "/api/__up",
            post(up).layer(DefaultBodyLimit::max(usize::MAX)),
        )
        .merge(ws::routes())
}

/// `Server-Timing` value for a duration since `start`.
fn server_timing(start: Instant) -> HeaderValue {
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    HeaderValue::from_str(&format!("app;dur={ms:.3}"))
        .unwrap_or_else(|_| HeaderValue::from_static("app"))
}

fn error_response(status: StatusCode, code: ErrorCode, message: impl Into<String>) -> Response {
    let body = json!({ "error": code, "message": message.into() });
    let mut res = (status, Json(body)).into_response();
    res.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

fn rejection_response(r: Rejection) -> Response {
    let status = match r.code {
        ErrorCode::Busy => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::LimitExceeded => StatusCode::PAYLOAD_TOO_LARGE,
        _ => StatusCode::BAD_REQUEST,
    };
    error_response(status, r.code, r.message)
}

fn too_large(bytes: u64, max: u64) -> Response {
    error_response(
        StatusCode::PAYLOAD_TOO_LARGE,
        ErrorCode::LimitExceeded,
        format!("{bytes} bytes requested; the limit is {max}"),
    )
}

fn begin(state: &AppState, session: Option<Uuid>) -> Result<Option<TransferGuard>, Rejection> {
    session
        .map(|id| state.measure.sessions.begin_transfer(id))
        .transpose()
}

#[derive(Debug, Deserialize)]
struct DownQuery {
    #[serde(default)]
    bytes: u64,
    session: Option<Uuid>,
}

/// `GET /api/__down?bytes=N`
async fn down(State(state): State<AppState>, Query(q): Query<DownQuery>) -> Response {
    let start = Instant::now();
    let max = state.measure.limits.max_bytes_per_request;
    if q.bytes > max {
        return too_large(q.bytes, max);
    }
    let transfer = match begin(&state, q.session) {
        Ok(t) => t,
        Err(r) => return rejection_response(r),
    };
    tracing::debug!(bytes = q.bytes, session = ?q.session, "http download");
    // The guard lives in the stream, so the transfer ends when the body is done or dropped.
    let chunks = state.measure.payload.chunks(q.bytes, CHUNK_SIZE);
    let body = stream::iter(chunks).map(move |chunk| {
        if let Some(t) = &transfer {
            t.session().add_sent(chunk.len() as u64);
        }
        Ok::<_, Infallible>(chunk)
    });
    let mut res = Response::new(Body::from_stream(body));
    let headers = res.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(CONTENT_LENGTH, HeaderValue::from(q.bytes));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert("server-timing", server_timing(start));
    res
}

#[derive(Debug, Deserialize)]
struct UpQuery {
    session: Option<Uuid>,
}

#[derive(Debug, Serialize)]
struct UpResult {
    bytes: u64,
}

/// `POST /api/__up`
async fn up(State(state): State<AppState>, Query(q): Query<UpQuery>, body: Body) -> Response {
    let start = Instant::now();
    let max = state.measure.limits.max_bytes_per_request;
    let declared = body_length(&body);
    if let Some(len) = declared.filter(|len| *len > max) {
        return too_large(len, max);
    }
    let transfer = match begin(&state, q.session) {
        Ok(t) => t,
        Err(r) => return rejection_response(r),
    };
    let mut total = 0u64;
    let mut data = body.into_data_stream();
    while let Some(chunk) = data.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    ErrorCode::BadRequest,
                    format!("failed to read the body: {e}"),
                );
            }
        };
        total += chunk.len() as u64;
        if let Some(t) = &transfer {
            t.session().add_received(chunk.len() as u64);
        }
        if total > max {
            return too_large(total, max);
        }
    }
    drop(transfer);
    tracing::debug!(bytes = total, session = ?q.session, "http upload");
    let timing = server_timing(start);
    let mut res = Json(UpResult { bytes: total }).into_response();
    res.headers_mut().insert("server-timing", timing);
    res.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

fn body_length(body: &Body) -> Option<u64> {
    use axum::body::HttpBody;
    body.size_hint().exact()
}

/// `GET /api/info` payload.
#[derive(Debug, Serialize)]
struct Info {
    server_name: String,
    version: &'static str,
    protocol_version: u32,
    limits: Limits,
    /// The client's address as seen by the server (`null` if unknown).
    client_ip: Option<String>,
    udp_port: u16,
    tls: bool,
    presence: bool,
    history: bool,
}

/// `GET /api/info`
async fn info(State(state): State<AppState>, req: Request) -> Json<Info> {
    let client_ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip().to_canonical().to_string());
    let config = &state.config;
    Json(Info {
        server_name: config.server_name.clone(),
        version: env!("CARGO_PKG_VERSION"),
        protocol_version: PROTOCOL_VERSION,
        limits: state.measure.limits,
        client_ip,
        udp_port: config.udp_port,
        tls: config.tls.is_some(),
        presence: config.presence,
        history: config.history,
    })
}

/// True when no token is configured, or `presented` equals it (constant time).
pub fn token_matches(config: &Config, presented: Option<&str>) -> bool {
    match &config.auth_token {
        None => true,
        Some(secret) => presented.is_some_and(|p| constant_time_eq(p, secret.expose())),
    }
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The token from `Authorization: Bearer <token>` or the `speedtest_token` cookie.
fn presented_token(headers: &HeaderMap) -> Option<&str> {
    let bearer = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            let (scheme, token) = v.trim().split_once(' ')?;
            scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
        });
    bearer.or_else(|| {
        headers
            .get_all(COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(';'))
            .filter_map(|pair| pair.trim().split_once('='))
            .find(|(name, _)| *name == TOKEN_COOKIE)
            .map(|(_, value)| value.trim().trim_matches('"'))
    })
}

/// Middleware: when `SPEEDTEST_AUTH_TOKEN` is set, `/api/*` except `/api/health` needs the
/// token (bearer header or cookie), else 401. Other paths pass through.
pub async fn require_token(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if state.config.auth_token.is_none() {
        return next.run(req).await;
    }
    let path = req.uri().path();
    let protected = (path == "/api" || path.starts_with("/api/")) && path != "/api/health";
    if !protected || token_matches(&state.config, presented_token(req.headers())) {
        return next.run(req).await;
    }
    let mut res = error_response(
        StatusCode::UNAUTHORIZED,
        ErrorCode::Unauthorized,
        "missing or wrong auth token",
    );
    res.headers_mut()
        .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bearer_and_cookie() {
        let mut h = HeaderMap::new();
        assert_eq!(presented_token(&h), None);
        h.insert(AUTHORIZATION, HeaderValue::from_static("Bearer abc"));
        assert_eq!(presented_token(&h), Some("abc"));
        let mut h = HeaderMap::new();
        h.insert(
            COOKIE,
            HeaderValue::from_static("theme=dark; speedtest_token=xyz; other=1"),
        );
        assert_eq!(presented_token(&h), Some("xyz"));
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
    }
}
