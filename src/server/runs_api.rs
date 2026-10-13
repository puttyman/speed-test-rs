//! History REST API (docs/SPEC.md §6.2): runs, labels, compare, groups and export.
//!
//! Errors are JSON `{"error": "..."}` with a matching status code. When history is off, every
//! endpoint answers `404 {"error":"history disabled"}`, except `POST /api/runs`, which
//! validates the body and answers `202` without storing it, so clients can always post.

use std::net::SocketAddr;

use axum::body::{Body, Bytes};
use axum::extract::rejection::{BytesRejection, QueryRejection};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Query, RawQuery, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE, LOCATION, USER_AGENT};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use futures_util::TryStreamExt;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::state::AppState;
use super::storage::{
    CompareTarget, DEFAULT_PAGE_SIZE, ExportFormat, RunFilter, Storage, StorageError,
};
use crate::model::result::RunResult;

/// Largest accepted `POST /api/runs` body.
pub const MAX_RUN_BODY_BYTES: usize = 2 * 1024 * 1024;
/// Longest label, in characters.
pub const MAX_LABEL_CHARS: usize = 100;
/// Longest notes, in characters.
pub const MAX_NOTES_CHARS: usize = 2000;
/// Most raw points per series.
const MAX_POINTS: usize = 10_000;
/// Longest accepted run.
const MAX_DURATION_MS: u64 = 60 * 60 * 1000;

/// The history routes. Merged into the main router by [`super::router::router`].
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/runs",
            get(list_runs)
                .post(post_run)
                .layer(DefaultBodyLimit::max(MAX_RUN_BODY_BYTES)),
        )
        .route("/api/runs/export", get(export_runs))
        .route(
            "/api/runs/{id}",
            get(get_run).patch(patch_run).delete(delete_run),
        )
        .route("/api/labels", get(labels))
        .route("/api/compare", get(compare))
        .route("/api/groups/{id}", get(get_group))
}

// ---------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------

/// A JSON error response: `{"error": message}`.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    fn history_disabled() -> Self {
        Self::new(StatusCode::NOT_FOUND, "history disabled")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

impl From<StorageError> for ApiError {
    fn from(e: StorageError) -> Self {
        let status = match &e {
            StorageError::NotFound(_) => StatusCode::NOT_FOUND,
            StorageError::Conflict(_) => StatusCode::CONFLICT,
            StorageError::Invalid(_) => StatusCode::BAD_REQUEST,
            _ => {
                tracing::error!("history: {e}");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        Self::new(status, e.to_string())
    }
}

impl From<QueryRejection> for ApiError {
    fn from(e: QueryRejection) -> Self {
        Self::bad_request(e.body_text())
    }
}

type ApiResult<T> = Result<T, ApiError>;

fn storage(state: &AppState) -> ApiResult<&Storage> {
    state
        .storage
        .as_ref()
        .ok_or_else(ApiError::history_disabled)
}

fn parse_id(id: &str, what: &str) -> ApiResult<Uuid> {
    Uuid::parse_str(id).map_err(|_| ApiError::bad_request(format!("invalid {what} id `{id}`")))
}

// ---------------------------------------------------------------------------------------------
// POST /api/runs
// ---------------------------------------------------------------------------------------------

/// Response of `POST /api/runs`.
#[derive(Debug, Serialize, Deserialize)]
pub struct PostRunResponse {
    pub id: Uuid,
    /// `false` when history is off (status 202).
    pub stored: bool,
}

/// Checks that a posted run is within sane bounds.
pub fn validate_run(run: &RunResult) -> Result<(), String> {
    let chars = |s: &Option<String>| s.as_deref().map_or(0, |s| s.chars().count());
    if chars(&run.label) > MAX_LABEL_CHARS {
        return Err(format!("label is longer than {MAX_LABEL_CHARS} characters"));
    }
    if chars(&run.notes) > MAX_NOTES_CHARS {
        return Err(format!(
            "notes are longer than {MAX_NOTES_CHARS} characters"
        ));
    }
    if run.profile.trim().is_empty() || run.profile.len() > 64 {
        return Err("profile must be 1 to 64 characters".into());
    }
    if chars(&run.client.device_id) > 128 || chars(&run.client.device_name) > 128 {
        return Err("device_id and device_name must be at most 128 characters".into());
    }
    if run.streams == 0 || run.streams > 64 {
        return Err("streams must be between 1 and 64".into());
    }
    if run.duration_ms > MAX_DURATION_MS {
        return Err("duration_ms is longer than an hour".into());
    }
    let c = &run.concurrency;
    if !(0.0..=1.0).contains(&c.overlap_ratio) || !c.overlap_ms.is_finite() || c.overlap_ms < 0.0 {
        return Err("concurrency values are out of range".into());
    }
    let s = &run.summary;
    let non_negative = [
        ("download_bps", s.download_bps),
        ("upload_bps", s.upload_bps),
        ("latency_ms", s.latency_ms),
        ("jitter_ms", s.jitter_ms),
        ("down_loaded_latency_ms", s.down_loaded_latency_ms),
        ("down_loaded_jitter_ms", s.down_loaded_jitter_ms),
        ("up_loaded_latency_ms", s.up_loaded_latency_ms),
        ("up_loaded_jitter_ms", s.up_loaded_jitter_ms),
        ("download_peak_bps", s.download_peak_bps),
        ("download_avg_bps", s.download_avg_bps),
        ("upload_peak_bps", s.upload_peak_bps),
        ("upload_avg_bps", s.upload_avg_bps),
    ];
    for (name, v) in non_negative {
        if v.is_some_and(|v| !v.is_finite() || v < 0.0) {
            return Err(format!(
                "summary.{name} must be a finite, non-negative number"
            ));
        }
    }
    if s.packet_loss_ratio
        .is_some_and(|v| !(0.0..=1.0).contains(&v))
    {
        return Err("summary.packet_loss_ratio must be between 0 and 1".into());
    }
    let p = &run.points;
    if [
        p.latency.len(),
        p.download.len(),
        p.upload.len(),
        p.down_loaded_latency.len(),
        p.up_loaded_latency.len(),
        p.download_series.len(),
        p.upload_series.len(),
    ]
    .iter()
    .any(|&n| n > MAX_POINTS)
    {
        return Err(format!("too many points (at most {MAX_POINTS} per series)"));
    }
    Ok(())
}

/// Query of `POST /api/runs`.
#[derive(Debug, Default, Deserialize)]
pub struct PostRunQuery {
    /// The session ID the run's sockets used (`Hello.session`).
    pub session: Option<String>,
}

/// Fills in what the server knows about the run's session (SPEC §5.11 "overlap flagging"):
/// the overlap with other sessions, which overrides any client-sent value, and the group
/// test the run belonged to. An unknown session leaves the run as sent.
///
/// A page keeps one session across runs, so the overlap window is the session's activity
/// within the run's duration (plus a little slack) before now, not its whole history.
fn apply_session_facts(state: &AppState, session: Uuid, run: &mut RunResult) {
    let registry = &state.measure.sessions;
    let lookback = std::time::Duration::from_millis(run.duration_ms.saturating_add(5_000));
    let since = tokio::time::Instant::now()
        .checked_sub(lookback)
        .unwrap_or_else(tokio::time::Instant::now);
    let concurrency = match registry.recent_activity_window(session, since) {
        Some(window) => registry.concurrency_for(session, Some(window)),
        None => registry.concurrency_for(session, None),
    };
    if let Some(c) = concurrency {
        run.concurrency = c;
    }
    if let Some(group) = state.groups.report_run(session, run) {
        run.group_id = Some(group);
    }
}

/// `POST /api/runs[?session=<uuid>]`: stores a run (body: `RunResult`). The client IP and
/// User-Agent come from the connection and the request headers, never from the body. With
/// `session`, the overlap and group are filled in from the server's session registry.
async fn post_run(
    State(state): State<AppState>,
    connect_info: Option<Extension<ConnectInfo<SocketAddr>>>,
    query: Result<Query<PostRunQuery>, QueryRejection>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let Query(query) = query?;
    let session = non_empty(&query.session)
        .map(|s| parse_id(&s, "session"))
        .transpose()?;
    let body = body.map_err(|e| {
        let status = e.status();
        if status == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::new(
                status,
                format!("run is larger than {MAX_RUN_BODY_BYTES} bytes"),
            )
        } else {
            ApiError::new(status, e.body_text())
        }
    })?;
    let mut run: RunResult = serde_json::from_slice(&body)
        .map_err(|e| ApiError::bad_request(format!("invalid run: {e}")))?;
    validate_run(&run).map_err(ApiError::bad_request)?;
    if let Some(session) = session {
        apply_session_facts(&state, session, &mut run);
    }

    let Some(storage) = state.storage.as_ref() else {
        if run.id.is_nil() {
            run.id = Uuid::now_v7();
        }
        let res = PostRunResponse {
            id: run.id,
            stored: false,
        };
        return Ok((StatusCode::ACCEPTED, Json(res)).into_response());
    };

    let (ip, user_agent) = if state.config.store_client_info {
        let ip = connect_info.map(|Extension(ConnectInfo(addr))| {
            // Report IPv4-mapped IPv6 peers (dual-stack sockets) as plain IPv4.
            addr.ip().to_canonical().to_string()
        });
        let ua = headers
            .get(USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.chars().take(512).collect::<String>());
        (ip, ua)
    } else {
        (None, None)
    };
    let id = storage
        .insert_run(&run, ip.as_deref(), user_agent.as_deref())
        .await?;
    let location = HeaderValue::from_str(&format!("/api/runs/{id}")).ok();
    let mut res = (
        StatusCode::CREATED,
        Json(PostRunResponse { id, stored: true }),
    )
        .into_response();
    if let Some(location) = location {
        res.headers_mut().insert(LOCATION, location);
    }
    Ok(res)
}

// ---------------------------------------------------------------------------------------------
// GET /api/runs, /api/runs/export
// ---------------------------------------------------------------------------------------------

/// Query of `GET /api/runs` and `GET /api/runs/export`.
#[derive(Debug, Default, Deserialize)]
pub struct RunsQuery {
    pub label: Option<String>,
    pub device: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub limit: Option<String>,
    pub cursor: Option<String>,
    /// `true` to skip runs that overlapped other devices' tests.
    pub exclude_overlapped: Option<String>,
    /// Export only: `csv` (default) or `json`.
    pub format: Option<String>,
}

fn non_empty(v: &Option<String>) -> Option<String> {
    v.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn parse_bool(v: &Option<String>, name: &str) -> ApiResult<bool> {
    match non_empty(v)
        .as_deref()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        None | Some("0" | "false" | "no") => Ok(false),
        Some("1" | "true" | "yes") => Ok(true),
        Some(other) => Err(ApiError::bad_request(format!(
            "invalid {name} `{other}` (expected true or false)"
        ))),
    }
}

impl RunsQuery {
    fn filter(&self) -> ApiResult<RunFilter> {
        Ok(RunFilter {
            label: non_empty(&self.label),
            device: non_empty(&self.device),
            from: non_empty(&self.from),
            to: non_empty(&self.to),
            exclude_overlapped: parse_bool(&self.exclude_overlapped, "exclude_overlapped")?,
        })
    }
}

/// `GET /api/runs`: runs newest first → `RunPage { runs: [RunSummary], next_cursor }`.
async fn list_runs(
    State(state): State<AppState>,
    query: Result<Query<RunsQuery>, QueryRejection>,
) -> ApiResult<Response> {
    let storage = storage(&state)?;
    let Query(query) = query?;
    let limit = match non_empty(&query.limit) {
        None => DEFAULT_PAGE_SIZE,
        Some(v) => v
            .parse::<u32>()
            .ok()
            .filter(|&n| n > 0)
            .ok_or_else(|| ApiError::bad_request(format!("invalid limit `{v}`")))?,
    };
    let cursor = non_empty(&query.cursor);
    let page = storage
        .list_runs(&query.filter()?, limit, cursor.as_deref())
        .await?;
    Ok(Json(page).into_response())
}

/// `GET /api/runs/export?format=csv|json`: the (optionally filtered) history as a download.
async fn export_runs(
    State(state): State<AppState>,
    query: Result<Query<RunsQuery>, QueryRejection>,
) -> ApiResult<Response> {
    let storage = storage(&state)?;
    let Query(query) = query?;
    let format: ExportFormat = non_empty(&query.format)
        .as_deref()
        .unwrap_or("csv")
        .parse()?;
    let filter = query.filter()?;
    let stream = storage
        .export_stream(format, filter)
        .map_ok(Bytes::from)
        .inspect_err(|e| tracing::error!("history export failed: {e}"));
    let disposition = format!(
        "attachment; filename=\"speedtest-history.{}\"",
        format.extension()
    );
    Ok((
        [
            (CONTENT_TYPE, format.content_type().to_owned()),
            (CONTENT_DISPOSITION, disposition),
        ],
        Body::from_stream(stream),
    )
        .into_response())
}

// ---------------------------------------------------------------------------------------------
// /api/runs/{id}
// ---------------------------------------------------------------------------------------------

/// `GET /api/runs/{id}`: the full `RunResult`, including raw points.
async fn get_run(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Response> {
    let storage = storage(&state)?;
    let id = parse_id(&id, "run")?;
    Ok(Json(storage.get_run(id).await?).into_response())
}

/// Body of `PATCH /api/runs/{id}`. An absent field is unchanged; `null` or `""` clears it.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunPatch {
    #[serde(default, deserialize_with = "present")]
    pub label: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    pub notes: Option<Option<String>>,
}

/// Distinguishes an explicit `null` (`Some(None)`) from an absent field (`None`).
fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// `PATCH /api/runs/{id}` with `{label?, notes?}` → the updated `RunSummary`.
async fn patch_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<Response> {
    let storage = storage(&state)?;
    let id = parse_id(&id, "run")?;
    let patch: RunPatch = serde_json::from_slice(&body)
        .map_err(|e| ApiError::bad_request(format!("invalid body: {e}")))?;
    if patch.label.is_none() && patch.notes.is_none() {
        return Err(ApiError::bad_request("nothing to update (label, notes)"));
    }
    let chars = |v: &Option<Option<String>>| {
        v.as_ref()
            .and_then(|v| v.as_ref())
            .map_or(0, |s| s.trim().chars().count())
    };
    if chars(&patch.label) > MAX_LABEL_CHARS {
        return Err(ApiError::bad_request(format!(
            "label is longer than {MAX_LABEL_CHARS} characters"
        )));
    }
    if chars(&patch.notes) > MAX_NOTES_CHARS {
        return Err(ApiError::bad_request(format!(
            "notes are longer than {MAX_NOTES_CHARS} characters"
        )));
    }
    let summary = storage.update_run(id, patch.label, patch.notes).await?;
    Ok(Json(summary).into_response())
}

/// `DELETE /api/runs/{id}` → `204`.
async fn delete_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let storage = storage(&state)?;
    let id = parse_id(&id, "run")?;
    storage.delete_run(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------------------------
// Labels, compare, groups
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct LabelsQuery {
    pub device: Option<String>,
}

/// `GET /api/labels?device=` → `[LabelSummary]`, most recently used first.
async fn labels(
    State(state): State<AppState>,
    query: Result<Query<LabelsQuery>, QueryRejection>,
) -> ApiResult<Response> {
    let storage = storage(&state)?;
    let Query(query) = query?;
    let device = non_empty(&query.device);
    Ok(Json(storage.labels(device.as_deref()).await?).into_response())
}

#[derive(Debug, Default, Deserialize)]
pub struct CompareQuery {
    /// Comma-separated labels.
    pub labels: Option<String>,
    /// Comma-separated run IDs.
    pub ids: Option<String>,
    pub exclude_overlapped: Option<String>,
}

/// `GET /api/compare?labels=a,b` or `?ids=…` (2–4 items) → `CompareResult`.
async fn compare(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
    query: Result<Query<CompareQuery>, QueryRejection>,
) -> ApiResult<Response> {
    let storage = storage(&state)?;
    let Query(query) = query?;
    // Split `labels` on literal commas *before* percent-decoding, so a label that contains
    // a comma can be passed encoded as `%2C`.
    if let Some(labels) = raw.as_deref().and_then(raw_labels) {
        let exclude = parse_bool(&query.exclude_overlapped, "exclude_overlapped")?;
        if query
            .ids
            .as_deref()
            .is_some_and(|ids| !ids.trim().is_empty())
        {
            return Err(ApiError::bad_request(
                "pass either labels=a,b or ids=x,y (2 to 4 items)",
            ));
        }
        let target = CompareTarget::Labels(labels);
        return Ok(Json(storage.compare(&target, exclude).await?).into_response());
    }
    let split = |v: &str| -> Vec<String> {
        v.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let target = match (non_empty(&query.labels), non_empty(&query.ids)) {
        (Some(labels), None) => CompareTarget::Labels(split(&labels)),
        (None, Some(ids)) => CompareTarget::Ids(
            split(&ids)
                .iter()
                .map(|id| parse_id(id, "run"))
                .collect::<ApiResult<_>>()?,
        ),
        _ => {
            return Err(ApiError::bad_request(
                "pass either labels=a,b or ids=x,y (2 to 4 items)",
            ));
        }
    };
    let exclude = parse_bool(&query.exclude_overlapped, "exclude_overlapped")?;
    Ok(Json(storage.compare(&target, exclude).await?).into_response())
}

/// The `labels` parameter of a raw query string, split on unencoded commas and then
/// form-decoded; `None` when absent or empty.
fn raw_labels(raw: &str) -> Option<Vec<String>> {
    let value = raw
        .split('&')
        .find_map(|pair| pair.strip_prefix("labels="))?;
    let labels: Vec<String> = value
        .split(',')
        .map(|part| {
            form_urlencoded::parse(format!("v={part}").as_bytes())
                .next()
                .map(|(_, v)| v.trim().to_owned())
                .unwrap_or_default()
        })
        .filter(|label| !label.is_empty())
        .collect();
    (!labels.is_empty()).then_some(labels)
}

/// `GET /api/groups/{id}` → `GroupRecord`.
async fn get_group(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Response> {
    let storage = storage(&state)?;
    let id = parse_id(&id, "group")?;
    Ok(Json(storage.get_group(id).await?).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::config::{Config, ServeArgs};
    use crate::server::router::router;
    use crate::server::storage::test_support::{sample_run, temp_storage, ts};
    use crate::server::storage::{LabelSummary, RunPage, RunSummary};
    use axum::http::Request;
    use http_body_util::BodyExt;
    use serde_json::Value;

    #[test]
    fn raw_labels_split_on_unencoded_commas_only() {
        assert_eq!(
            raw_labels("labels=Office%2C%20desk,Bedroom+AP&exclude_overlapped=true"),
            Some(vec!["Office, desk".to_owned(), "Bedroom AP".to_owned()])
        );
        assert_eq!(raw_labels("ids=a,b"), None);
        assert_eq!(raw_labels("labels="), None);
    }
    use tower::ServiceExt;

    struct TestApp {
        app: Router,
        _dir: Option<tempfile::TempDir>,
    }

    fn config(env: &[(&str, &str)]) -> Config {
        let env: Vec<(String, String)> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let args = ServeArgs {
            listen: vec!["127.0.0.1:0".into()],
            ..ServeArgs::default()
        };
        Config::resolve(args, move |key| {
            env.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
        })
        .unwrap()
    }

    async fn app_with(env: &[(&str, &str)]) -> TestApp {
        let config = config(env);
        let mut state = AppState::new(config);
        let dir = if state.config.history {
            let (storage, dir) = temp_storage().await;
            state.storage = Some(storage);
            Some(dir)
        } else {
            None
        };
        TestApp {
            app: router(state),
            _dir: dir,
        }
    }

    async fn app() -> TestApp {
        app_with(&[]).await
    }

    impl TestApp {
        async fn send(&self, req: Request<Body>) -> (StatusCode, HeaderMap, Bytes) {
            let res = self.app.clone().oneshot(req).await.unwrap();
            let status = res.status();
            let headers = res.headers().clone();
            let body = res.into_body().collect().await.unwrap().to_bytes();
            (status, headers, body)
        }

        async fn json(&self, req: Request<Body>) -> (StatusCode, Value) {
            let (status, _, body) = self.send(req).await;
            let value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            (status, value)
        }

        async fn get(&self, uri: &str) -> (StatusCode, Value) {
            self.json(Request::get(uri).body(Body::empty()).unwrap())
                .await
        }

        async fn post_run(&self, run: &RunResult) -> (StatusCode, Value) {
            self.json(post_req(serde_json::to_vec(run).unwrap(), None))
                .await
        }
    }

    fn post_req(body: Vec<u8>, peer: Option<SocketAddr>) -> Request<Body> {
        let mut req = Request::post("/api/runs")
            .header(CONTENT_TYPE, "application/json")
            .header(USER_AGENT, "Mozilla/5.0 (iPhone)")
            .body(Body::from(body))
            .unwrap();
        if let Some(peer) = peer {
            req.extensions_mut().insert(ConnectInfo(peer));
        }
        req
    }

    fn json_req(method: &str, uri: &str, body: Value) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn post_get_patch_delete_flow() {
        let t = app().await;
        let run = sample_run(&ts(10, 9, 0), Some("Office"), "dev-1", 940.0);
        // A dual-stack socket reports IPv4 peers as IPv4-mapped IPv6.
        let peer: SocketAddr = "[::ffff:192.168.1.23]:51000".parse().unwrap();
        let (status, headers, body) = t
            .send(post_req(serde_json::to_vec(&run).unwrap(), Some(peer)))
            .await;
        assert_eq!(status, StatusCode::CREATED);
        let posted: PostRunResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(posted.id, run.id);
        assert!(posted.stored);
        let location = format!("/api/runs/{}", run.id);
        assert_eq!(headers[LOCATION], location.as_str());

        // Full run: the server's view of the client replaces the posted one.
        let (status, got) = t.get(&location).await;
        assert_eq!(status, StatusCode::OK);
        let got: RunResult = serde_json::from_value(got).unwrap();
        assert_eq!(got.client.ip.as_deref(), Some("192.168.1.23"));
        assert_eq!(
            got.client.user_agent.as_deref(),
            Some("Mozilla/5.0 (iPhone)")
        );
        assert_eq!(got.client.device_id.as_deref(), Some("dev-1"));
        assert_eq!(got.points, run.points);

        // List.
        let (status, page) = t.get("/api/runs?label=Office&device=dev-1&limit=10").await;
        assert_eq!(status, StatusCode::OK);
        let page: RunPage = serde_json::from_value(page).unwrap();
        assert_eq!(page.runs.len(), 1);
        assert_eq!(page.runs[0].id, run.id);
        assert_eq!(page.next_cursor, None);

        // Labels.
        let (status, labels) = t.get("/api/labels?device=dev-1").await;
        assert_eq!(status, StatusCode::OK);
        let labels: Vec<LabelSummary> = serde_json::from_value(labels).unwrap();
        assert_eq!(labels[0].label, "Office");
        assert_eq!(labels[0].median_download_bps, Some(940e6));

        // Patch: rename, then clear the notes with null.
        let (status, s) = t
            .json(json_req(
                "PATCH",
                &location,
                json!({"label": "Bedroom", "notes": "near the door"}),
            ))
            .await;
        assert_eq!(status, StatusCode::OK);
        let s: RunSummary = serde_json::from_value(s).unwrap();
        assert_eq!(s.label.as_deref(), Some("Bedroom"));
        assert_eq!(s.notes.as_deref(), Some("near the door"));
        let (_, s) = t
            .json(json_req("PATCH", &location, json!({"notes": null})))
            .await;
        assert_eq!(s["label"], "Bedroom");
        assert_eq!(s["notes"], Value::Null);

        // Delete.
        let (status, _, body) = t
            .send(
                Request::delete(location.as_str())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(body.is_empty());
        let (status, err) = t.get(&location).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(err["error"], "run not found");
    }

    #[tokio::test]
    async fn client_info_can_be_turned_off() {
        let t = app_with(&[("SPEEDTEST_STORE_CLIENT_INFO", "false")]).await;
        let run = sample_run(&ts(10, 9, 0), None, "dev-1", 100.0);
        let peer: SocketAddr = "192.168.1.23:51000".parse().unwrap();
        let (status, _, _) = t
            .send(post_req(serde_json::to_vec(&run).unwrap(), Some(peer)))
            .await;
        assert_eq!(status, StatusCode::CREATED);
        let (_, got) = t.get(&format!("/api/runs/{}", run.id)).await;
        assert_eq!(got["client"]["ip"], Value::Null);
        assert_eq!(got["client"]["user_agent"], Value::Null);
        assert_eq!(got["client"]["device_id"], "dev-1");
    }

    #[tokio::test]
    async fn history_disabled() {
        let t = app_with(&[("SPEEDTEST_HISTORY", "false")]).await;
        let run = sample_run(&ts(10, 9, 0), None, "dev-1", 100.0);
        let (status, body) = t.post_run(&run).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(body["stored"], false);
        assert_eq!(body["id"], run.id.to_string());
        // Invalid bodies are still rejected.
        let (status, _) = t.json(post_req(b"{}".to_vec(), None)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let id = run.id;
        for uri in [
            "/api/runs".to_owned(),
            format!("/api/runs/{id}"),
            "/api/runs/export?format=csv".to_owned(),
            "/api/labels".to_owned(),
            "/api/compare?labels=a,b".to_owned(),
            format!("/api/groups/{id}"),
        ] {
            let (status, body) = t.get(&uri).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
            assert_eq!(body, json!({"error": "history disabled"}), "{uri}");
        }
        let (status, _) = t
            .json(json_req(
                "PATCH",
                &format!("/api/runs/{id}"),
                json!({"label": "x"}),
            ))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = t
            .json(
                Request::delete(format!("/api/runs/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn bad_requests_and_not_found() {
        let t = app().await;
        let missing = Uuid::now_v7();
        let cases = [
            ("/api/runs/not-a-uuid", StatusCode::BAD_REQUEST),
            (&*format!("/api/runs/{missing}"), StatusCode::NOT_FOUND),
            ("/api/runs?limit=abc", StatusCode::BAD_REQUEST),
            ("/api/runs?limit=0", StatusCode::BAD_REQUEST),
            ("/api/runs?cursor=nope", StatusCode::BAD_REQUEST),
            ("/api/runs?from=someday", StatusCode::BAD_REQUEST),
            (
                "/api/runs?exclude_overlapped=maybe",
                StatusCode::BAD_REQUEST,
            ),
            ("/api/compare", StatusCode::BAD_REQUEST),
            ("/api/compare?labels=a", StatusCode::BAD_REQUEST),
            ("/api/compare?labels=a,b,c,d,e", StatusCode::BAD_REQUEST),
            ("/api/compare?labels=a,b", StatusCode::NOT_FOUND),
            ("/api/compare?ids=x,y", StatusCode::BAD_REQUEST),
            ("/api/compare?labels=a,b&ids=x,y", StatusCode::BAD_REQUEST),
            (&*format!("/api/groups/{missing}"), StatusCode::NOT_FOUND),
            ("/api/groups/zzz", StatusCode::BAD_REQUEST),
            ("/api/runs/export?format=xml", StatusCode::BAD_REQUEST),
        ];
        for (uri, expected) in cases {
            let (status, body) = t.get(uri).await;
            assert_eq!(status, expected, "{uri}: {body}");
            assert!(body["error"].is_string(), "{uri}: {body}");
        }

        // Invalid posts.
        let (status, body) = t.json(post_req(b"not json".to_vec(), None)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"].as_str().unwrap().starts_with("invalid run"));
        let mut run = sample_run(&ts(10, 9, 0), None, "d", 100.0);
        run.summary.download_bps = Some(-1.0);
        assert_eq!(t.post_run(&run).await.0, StatusCode::BAD_REQUEST);
        let mut run = sample_run(&ts(10, 9, 0), Some(&"x".repeat(101)), "d", 100.0);
        assert_eq!(t.post_run(&run).await.0, StatusCode::BAD_REQUEST);
        run.label = None;
        run.streams = 0;
        assert_eq!(t.post_run(&run).await.0, StatusCode::BAD_REQUEST);
        let run = sample_run("not a time", None, "d", 100.0);
        assert_eq!(t.post_run(&run).await.0, StatusCode::BAD_REQUEST);

        // Duplicate id.
        let run = sample_run(&ts(10, 9, 0), None, "d", 100.0);
        assert_eq!(t.post_run(&run).await.0, StatusCode::CREATED);
        assert_eq!(t.post_run(&run).await.0, StatusCode::CONFLICT);

        // Bad patches.
        let uri = format!("/api/runs/{}", run.id);
        for body in [json!({}), json!({"labl": "x"}), json!({"label": 3})] {
            let (status, _) = t.json(json_req("PATCH", &uri, body.clone())).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        }
        let (status, _) = t
            .json(json_req(
                "PATCH",
                &format!("/api/runs/{missing}"),
                json!({"label": "x"}),
            ))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn auth_token_protects_history() {
        let t = app_with(&[("SPEEDTEST_AUTH_TOKEN", "s3cret")]).await;
        assert_eq!(t.get("/api/runs").await.0, StatusCode::UNAUTHORIZED);
        let run = sample_run(&ts(10, 9, 0), None, "d", 100.0);
        assert_eq!(t.post_run(&run).await.0, StatusCode::UNAUTHORIZED);
        let req = Request::get("/api/runs")
            .header("authorization", "Bearer s3cret")
            .body(Body::empty())
            .unwrap();
        assert_eq!(t.json(req).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn oversized_body_is_rejected() {
        let t = app().await;
        let mut run = sample_run(&ts(10, 9, 0), None, "d", 100.0);
        run.notes = Some("x".repeat(MAX_RUN_BODY_BYTES + 1));
        let (status, body) = t
            .json(post_req(serde_json::to_vec(&run).unwrap(), None))
            .await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert!(body["error"].as_str().unwrap().contains("larger than"));
        let (_, page) = t.get("/api/runs").await;
        assert_eq!(page["runs"], json!([]));
    }

    #[tokio::test]
    async fn pagination_compare_groups_and_export() {
        let t = app().await;
        for i in 1..=6u32 {
            let label = if i % 2 == 0 { "Office" } else { "Bedroom" };
            let run = sample_run(&ts(i, 9, 0), Some(label), "d1", f64::from(i) * 100.0);
            assert_eq!(t.post_run(&run).await.0, StatusCode::CREATED);
        }

        // Cursor pagination.
        let (_, first) = t.get("/api/runs?limit=4").await;
        assert_eq!(first["runs"].as_array().unwrap().len(), 4);
        let cursor = first["next_cursor"].as_str().unwrap();
        let (_, second) = t.get(&format!("/api/runs?limit=4&cursor={cursor}")).await;
        assert_eq!(second["runs"].as_array().unwrap().len(), 2);
        assert_eq!(second["next_cursor"], Value::Null);
        assert_eq!(second["runs"][1]["created_at"], ts(1, 9, 0));
        let (_, ranged) = t.get("/api/runs?from=2026-10-02&to=2026-10-03").await;
        assert_eq!(ranged["runs"].as_array().unwrap().len(), 2);

        // Compare labels: Office 200/400/600 → 400, Bedroom 100/300/500 → 300.
        let (status, cmp) = t.get("/api/compare?labels=Office,%20Bedroom").await;
        assert_eq!(status, StatusCode::OK, "{cmp}");
        assert_eq!(cmp["items"][0]["key"], "Office");
        assert_eq!(cmp["items"][0]["summary"]["download_bps"], 400e6);
        assert_eq!(cmp["items"][0]["deltas"], Value::Null);
        assert_eq!(cmp["items"][1]["kind"], "label");
        assert_eq!(cmp["items"][1]["deltas"]["download_bps"]["abs"], -100e6);
        assert_eq!(cmp["items"][1]["deltas"]["download_bps"]["ratio"], -0.25);
        assert_eq!(cmp["items"][1]["deltas"]["download_bps"]["better"], false);

        let ids: Vec<String> = first["runs"]
            .as_array()
            .unwrap()
            .iter()
            .take(2)
            .map(|r| r["id"].as_str().unwrap().to_owned())
            .collect();
        let (status, cmp) = t.get(&format!("/api/compare?ids={}", ids.join(","))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(cmp["items"][1]["kind"], "run");
        assert_eq!(cmp["items"][1]["deltas"]["download_bps"]["abs"], -100e6);

        // Group: a member run posted with a group id.
        let group_id = Uuid::now_v7();
        let mut run = sample_run(&ts(7, 9, 0), Some("Office"), "d2", 250.0);
        run.group_id = Some(group_id);
        assert_eq!(t.post_run(&run).await.0, StatusCode::CREATED);
        let (status, group) = t.get(&format!("/api/groups/{group_id}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(group["detail"], Value::Null);
        assert_eq!(group["runs"][0]["id"], run.id.to_string());

        // Export.
        let (status, headers, body) = t
            .send(
                Request::get("/api/runs/export?format=csv")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[CONTENT_TYPE], "text/csv; charset=utf-8");
        assert_eq!(
            headers[CONTENT_DISPOSITION],
            "attachment; filename=\"speedtest-history.csv\""
        );
        let csv = String::from_utf8(body.to_vec()).unwrap();
        assert!(csv.starts_with("id,created_at,label,"));
        assert_eq!(csv.lines().count(), 8);

        let (status, headers, body) = t
            .send(
                Request::get("/api/runs/export?format=json&label=Office")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[CONTENT_TYPE], "application/json");
        let runs: Vec<RunResult> = serde_json::from_slice(&body).unwrap();
        assert_eq!(runs.len(), 4);
        assert!(runs.iter().all(|r| r.label.as_deref() == Some("Office")));
    }
}
