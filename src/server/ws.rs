//! Measurement WebSockets (SPEC §5.4–§5.7, §6.1, §11): `/ws/control`, `/ws/data` and
//! `/ws/probe`.
//!
//! Every socket opens with the client's `Hello{session, role, device?, auth?}`. The role
//! must match the path, and the token must match `SPEEDTEST_AUTH_TOKEN` when one is set.
//! The server answers `Welcome{...}`, or `Error{...}` followed by a Close frame.
//!
//! - **probe**: `Ping{seq}` → `Pong{seq}`, immediately.
//! - **data**: one request at a time. `Download{id, bytes}` → `bytes` of binary frames (64
//!   KiB each, the last one shorter) then `DownloadDone{id, server_ms}`.
//!   `Upload{id, bytes}` followed by `bytes` of binary frames → `UploadDone{id, bytes,
//!   server_ms}`. `Abort` (on this socket or the session's control socket) stops the
//!   current transfer, which then ends with `Error{code: Conflict, message: "aborted"}`
//!   instead of the `*Done` message.
//! - **control**: `Status`, `SetHidden`, `Abort`, group messages (handled by
//!   [`Groups`](super::groups::Groups)) and WebRTC signalling through the [`RtcSignaling`]
//!   hook. Every control socket has an outbound queue registered in the
//!   [`SessionRegistry`], so the server can push messages to it at any time: right after
//!   `Welcome` it gets one presence snapshot (`Peers`), then the presence broadcaster's
//!   snapshots (SPEC §5.11). When a control socket that reported a test phase closes, the
//!   session's phase is reset to idle; when a session's last control socket closes, it
//!   leaves its group.
//!
//! Idle timeout: a socket that receives no frame for [`IDLE_TIMEOUT`] gets
//! `Error{code: Timeout}` and is closed. The server sends a WebSocket Ping on control
//! sockets every [`CONTROL_PING_INTERVAL`]; browsers answer it automatically, so an open
//! page keeps its presence socket alive while a vanished one times out. Data sockets never
//! time out while a download is being sent.
//!
//! `permessage-deflate` is never negotiated (Axum doesn't implement it), so random payload
//! bytes go over the wire as-is.

use std::net::{IpAddr, SocketAddr};
use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::response::Response;
use axum::routing::get;
use axum::{Extension, Router};
use bytes::Bytes;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{Sink, SinkExt, StreamExt};
use rand::Rng;
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use uuid::Uuid;

use super::config::Config;
use super::groups::StartContext;
use super::http_measure::token_matches;
use super::presence;
use super::rtc_hook::RtcSignaling;
use super::sessions::{CONTROL_QUEUE, Session, SessionRegistry, SocketGuard, TransferGuard};
use super::state::AppState;
use crate::model::protocol::{
    ClientMsg, DeviceInfo, ErrorCode, Limits, PROTOCOL_VERSION, Phase, Role, ServerMsg,
};

/// Size of the shared random payload buffer.
pub const PAYLOAD_SIZE: usize = 4 * 1024 * 1024;
/// Size of one download frame (and of one HTTP body chunk).
pub const CHUNK_SIZE: usize = 64 * 1024;
/// Largest accepted text (JSON) frame.
pub const MAX_TEXT_SIZE: usize = 64 * 1024;
/// Largest accepted binary frame / message.
pub const MAX_BINARY_SIZE: usize = 1024 * 1024;
/// A socket that receives nothing for this long is closed.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the client has to send `Hello` after connecting.
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// WebSocket Ping interval on control sockets.
pub const CONTROL_PING_INTERVAL: Duration = Duration::from_secs(10);
/// Data sockets allowed per session (`streams`, SPEC §5.5).
pub const MAX_STREAMS: u8 = 8;

// Close codes (RFC 6455 §7.4.1).
const CLOSE_NORMAL: u16 = 1000;
const CLOSE_PROTOCOL: u16 = 1002;
const CLOSE_POLICY: u16 = 1008;
const CLOSE_TOO_BIG: u16 = 1009;
const CLOSE_TRY_AGAIN: u16 = 1013;

/// The random payload every download is sliced from (generated once at startup).
#[derive(Clone)]
pub struct Payload(Bytes);

impl Payload {
    /// `size` random bytes (at least one).
    pub fn random(size: usize) -> Self {
        let mut buf = vec![0u8; size.max(1)];
        rand::rng().fill_bytes(&mut buf);
        Self(Bytes::from(buf))
    }

    /// `total` bytes as zero-copy slices of at most `chunk` bytes, cycling over the buffer.
    pub fn chunks(&self, total: u64, chunk: usize) -> Chunks {
        Chunks {
            buf: self.0.clone(),
            remaining: total,
            offset: 0,
            chunk: chunk.clamp(1, self.0.len()),
        }
    }
}

/// Iterator returned by [`Payload::chunks`].
pub struct Chunks {
    buf: Bytes,
    remaining: u64,
    offset: usize,
    chunk: usize,
}

impl Iterator for Chunks {
    type Item = Bytes;

    fn next(&mut self) -> Option<Bytes> {
        if self.remaining == 0 {
            return None;
        }
        let len = self
            .chunk
            .min(usize::try_from(self.remaining).unwrap_or(usize::MAX));
        if self.offset + len > self.buf.len() {
            self.offset = 0;
        }
        let slice = self.buf.slice(self.offset..self.offset + len);
        self.offset += len;
        self.remaining -= len as u64;
        Some(slice)
    }
}

/// Measurement state shared by the WebSocket and HTTP measurement handlers
/// (`AppState::measure`). Cheap to clone.
#[derive(Clone)]
pub struct MeasureState {
    pub sessions: SessionRegistry,
    pub payload: Payload,
    /// WebRTC signalling for the packet-loss test; `None` until a later wave installs
    /// the str0m endpoint (`state.measure.rtc = Some(Arc::new(...))`).
    pub rtc: Option<Arc<dyn RtcSignaling>>,
    /// Advertised in `Welcome` and `/api/info`, and enforced.
    pub limits: Limits,
}

impl MeasureState {
    /// Builds the registry and the payload, and starts the rate sampler if called inside a
    /// Tokio runtime.
    pub fn new(config: &Config) -> Self {
        let limits = Limits {
            max_bytes_per_request: config.max_bytes,
            max_streams: MAX_STREAMS,
            max_concurrent_sessions: u32::try_from(config.max_sessions).unwrap_or(u32::MAX),
        };
        let sessions = SessionRegistry::new(config.max_sessions, usize::from(MAX_STREAMS));
        if sessions.start_sampler().is_none() {
            tracing::warn!("no Tokio runtime: live session rates are disabled");
        }
        Self {
            sessions,
            payload: Payload::random(PAYLOAD_SIZE),
            rtc: None,
            limits,
        }
    }
}

/// `/ws/control`, `/ws/data` and `/ws/probe`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/ws/control", get(control_upgrade))
        .route("/ws/data", get(data_upgrade))
        .route("/ws/probe", get(probe_upgrade))
}

fn configure(ws: WebSocketUpgrade) -> WebSocketUpgrade {
    ws.max_message_size(MAX_BINARY_SIZE)
        .max_frame_size(MAX_BINARY_SIZE)
        // Bounded so a slow reader applies backpressure instead of growing a buffer.
        .write_buffer_size(256 * 1024)
        .max_write_buffer_size(4 * MAX_BINARY_SIZE)
}

async fn control_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    connect_info: Option<Extension<ConnectInfo<SocketAddr>>>,
) -> Response {
    let peer_ip = connect_info.map(|Extension(ConnectInfo(addr))| addr.ip().to_canonical());
    configure(ws).on_upgrade(move |socket| control_socket(socket, state, peer_ip))
}

async fn data_upgrade(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    configure(ws).on_upgrade(move |socket| data_socket(socket, state))
}

async fn probe_upgrade(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    configure(ws).on_upgrade(move |socket| probe_socket(socket, state))
}

// ---------------------------------------------------------------------------------------
// Shared helpers

fn text(msg: &ServerMsg) -> Message {
    Message::Text(Utf8Bytes::from(msg.to_json()))
}

async fn send_msg<S>(sink: &mut S, msg: &ServerMsg) -> Result<(), axum::Error>
where
    S: Sink<Message, Error = axum::Error> + Unpin,
{
    sink.send(text(msg)).await
}

/// Sends `Error{code, message}` and a Close frame. Errors are ignored: the peer may be gone.
async fn reject<S>(sink: &mut S, code: ErrorCode, message: &str, close_code: u16)
where
    S: Sink<Message, Error = axum::Error> + Unpin,
{
    let _ = send_msg(sink, &ServerMsg::error(code, message)).await;
    let _ = sink
        .send(Message::Close(Some(CloseFrame {
            code: close_code,
            reason: Utf8Bytes::from(truncate(message, 120)),
        })))
        .await;
}

fn close_code_for(code: ErrorCode) -> u16 {
    match code {
        ErrorCode::Busy => CLOSE_TRY_AGAIN,
        ErrorCode::Unauthorized => CLOSE_POLICY,
        ErrorCode::LimitExceeded => CLOSE_POLICY,
        ErrorCode::Timeout => CLOSE_NORMAL,
        _ => CLOSE_PROTOCOL,
    }
}

fn truncate(s: &str, max: usize) -> String {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_owned()
}

/// What the client sent in its `Hello`.
struct Hello {
    session: Uuid,
    device: Option<DeviceInfo>,
}

/// Waits for `Hello`, checks the role and the token. On failure the socket has been sent
/// an error and a Close frame.
async fn handshake(socket: &mut WebSocket, config: &Config, role: Role) -> Option<Hello> {
    let deadline = Instant::now() + HELLO_TIMEOUT;
    let msg = loop {
        match tokio::time::timeout_at(deadline, socket.recv()).await {
            Err(_) => {
                reject(socket, ErrorCode::Timeout, "expected hello", CLOSE_NORMAL).await;
                return None;
            }
            Ok(None | Some(Err(_)) | Some(Ok(Message::Close(_)))) => return None,
            Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => continue,
            Ok(Some(Ok(Message::Text(t)))) if t.len() <= MAX_TEXT_SIZE => {
                break ClientMsg::from_json(t.as_str()).ok();
            }
            Ok(Some(Ok(_))) => break None,
        }
    };
    let Some(ClientMsg::Hello {
        session,
        role: hello_role,
        device,
        auth,
    }) = msg
    else {
        reject(
            socket,
            ErrorCode::BadRequest,
            "the first message must be hello",
            CLOSE_PROTOCOL,
        )
        .await;
        return None;
    };
    if !token_matches(config, auth.as_deref()) {
        reject(
            socket,
            ErrorCode::Unauthorized,
            "missing or wrong auth token",
            CLOSE_POLICY,
        )
        .await;
        return None;
    }
    if hello_role != role {
        let message = format!("role mismatch: this endpoint is for {role:?} sockets");
        reject(socket, ErrorCode::BadRequest, &message, CLOSE_PROTOCOL).await;
        return None;
    }
    Some(Hello { session, device })
}

fn welcome(state: &AppState) -> ServerMsg {
    ServerMsg::Welcome {
        server_version: env!("CARGO_PKG_VERSION").to_owned(),
        server_name: state.config.server_name.clone(),
        protocol_version: PROTOCOL_VERSION,
        limits: state.measure.limits,
    }
}

/// A received frame, classified.
enum Incoming {
    Msg(ClientMsg),
    Binary(Bytes),
    /// A control frame (Ping/Pong): activity only.
    Other,
    /// The text frame was not a valid message.
    Invalid(String),
    TooBig,
    Closed,
}

fn classify(frame: Option<Result<Message, axum::Error>>) -> Incoming {
    match frame {
        None | Some(Err(_)) | Some(Ok(Message::Close(_))) => Incoming::Closed,
        Some(Ok(Message::Text(t))) if t.len() > MAX_TEXT_SIZE => Incoming::TooBig,
        Some(Ok(Message::Text(t))) => match ClientMsg::from_json(t.as_str()) {
            Ok(msg) => Incoming::Msg(msg),
            Err(e) => Incoming::Invalid(format!("invalid message: {e}")),
        },
        Some(Ok(Message::Binary(b))) => Incoming::Binary(b),
        Some(Ok(Message::Ping(_) | Message::Pong(_))) => Incoming::Other,
    }
}

// ---------------------------------------------------------------------------------------
// Probe socket

async fn probe_socket(mut socket: WebSocket, state: AppState) {
    let Some(hello) = handshake(&mut socket, &state.config, Role::Probe).await else {
        return;
    };
    let guard = match state
        .measure
        .sessions
        .attach(hello.session, Role::Probe, None, None)
    {
        Ok(g) => g,
        Err(r) => return reject(&mut socket, r.code, &r.message, close_code_for(r.code)).await,
    };
    if send_msg(&mut socket, &welcome(&state)).await.is_err() {
        return;
    }
    loop {
        let incoming = match tokio::time::timeout(IDLE_TIMEOUT, socket.recv()).await {
            Ok(frame) => classify(frame),
            Err(_) => {
                return reject(
                    &mut socket,
                    ErrorCode::Timeout,
                    "idle timeout",
                    CLOSE_NORMAL,
                )
                .await;
            }
        };
        guard.session().touch();
        let reply = match incoming {
            Incoming::Msg(ClientMsg::Ping { seq }) => ServerMsg::Pong { seq },
            Incoming::Msg(ClientMsg::Abort) | Incoming::Other => continue,
            Incoming::Msg(_) | Incoming::Binary(_) => ServerMsg::error(
                ErrorCode::BadRequest,
                "only ping is accepted on the probe socket",
            ),
            Incoming::Invalid(e) => ServerMsg::error(ErrorCode::BadRequest, e),
            Incoming::TooBig => {
                return reject(
                    &mut socket,
                    ErrorCode::BadRequest,
                    "frame too big",
                    CLOSE_TOO_BIG,
                )
                .await;
            }
            Incoming::Closed => return,
        };
        if send_msg(&mut socket, &reply).await.is_err() {
            return;
        }
    }
}

// ---------------------------------------------------------------------------------------
// Data socket

/// An upload in progress on a data socket.
struct Upload {
    id: u32,
    expected: u64,
    received: u64,
    /// When the `upload` request arrived. The client sends the payload right behind it, so
    /// this marks the first byte (see [`finish_upload`]).
    started: Instant,
    _transfer: TransferGuard,
}

async fn data_socket(mut socket: WebSocket, state: AppState) {
    let Some(hello) = handshake(&mut socket, &state.config, Role::Data).await else {
        return;
    };
    let guard = match state
        .measure
        .sessions
        .attach(hello.session, Role::Data, None, None)
    {
        Ok(g) => g,
        Err(r) => {
            tracing::debug!(session = %hello.session, code = ?r.code, "data socket refused");
            return reject(&mut socket, r.code, &r.message, close_code_for(r.code)).await;
        }
    };
    if send_msg(&mut socket, &welcome(&state)).await.is_err() {
        return;
    }
    let (mut sink, mut stream) = socket.split();
    let session = guard.session().clone();
    let mut abort = session.abort_signal();
    abort.mark_unchanged();
    let mut upload: Option<Upload> = None;

    loop {
        let frame = tokio::select! {
            frame = tokio::time::timeout(IDLE_TIMEOUT, stream.next()) => frame,
            changed = abort.changed() => {
                if changed.is_err() {
                    return;
                }
                if let Some(up) = upload.take() {
                    tracing::debug!(session = %session.id(), id = up.id, "upload aborted");
                    if send_msg(&mut sink, &aborted(up.id)).await.is_err() {
                        return;
                    }
                }
                continue;
            }
        };
        let incoming = match frame {
            Ok(frame) => classify(frame),
            Err(_) => {
                return reject(&mut sink, ErrorCode::Timeout, "idle timeout", CLOSE_NORMAL).await;
            }
        };
        session.touch();
        let reply = match incoming {
            Incoming::Closed => return,
            Incoming::Other => continue,
            Incoming::TooBig => {
                return reject(
                    &mut sink,
                    ErrorCode::BadRequest,
                    "frame too big",
                    CLOSE_TOO_BIG,
                )
                .await;
            }
            Incoming::Invalid(e) => Some(ServerMsg::error(ErrorCode::BadRequest, e)),
            Incoming::Binary(data) => {
                session.add_received(data.len() as u64);
                match upload.as_mut() {
                    Some(up) => {
                        up.received += data.len() as u64;
                        if up.received >= up.expected {
                            upload.take().map(|up| finish_upload(&session, up))
                        } else {
                            None
                        }
                    }
                    // Leftovers of an aborted upload.
                    None => None,
                }
            }
            Incoming::Msg(ClientMsg::Abort) => upload.take().map(|up| aborted(up.id)),
            Incoming::Msg(_) if upload.is_some() => Some(ServerMsg::error(
                ErrorCode::Conflict,
                "an upload is in progress; send its bytes or abort",
            )),
            Incoming::Msg(ClientMsg::Upload { id, bytes }) => {
                if let Some(err) = check_size(&state, bytes) {
                    Some(err)
                } else {
                    let up = Upload {
                        id,
                        expected: bytes,
                        received: 0,
                        started: Instant::now(),
                        _transfer: guard.begin_transfer(),
                    };
                    if bytes == 0 {
                        Some(finish_upload(&session, up))
                    } else {
                        upload = Some(up);
                        None
                    }
                }
            }
            Incoming::Msg(ClientMsg::Download { id, bytes }) => {
                if let Some(err) = check_size(&state, bytes) {
                    Some(err)
                } else {
                    match run_download(
                        &mut sink,
                        &mut stream,
                        &mut abort,
                        &state.measure.payload,
                        &guard,
                        id,
                        bytes,
                    )
                    .await
                    {
                        ControlFlow::Continue(reply) => Some(reply),
                        ControlFlow::Break(()) => return,
                    }
                }
            }
            Incoming::Msg(_) => Some(ServerMsg::error(
                ErrorCode::BadRequest,
                "only download, upload and abort are accepted on the data socket",
            )),
        };
        if let Some(reply) = reply
            && send_msg(&mut sink, &reply).await.is_err()
        {
            return;
        }
    }
}

fn aborted(id: u32) -> ServerMsg {
    ServerMsg::error(ErrorCode::Conflict, format!("transfer {id} aborted"))
}

fn check_size(state: &AppState, bytes: u64) -> Option<ServerMsg> {
    let max = state.measure.limits.max_bytes_per_request;
    (bytes > max).then(|| {
        ServerMsg::error(
            ErrorCode::LimitExceeded,
            format!("{bytes} bytes requested; the limit is {max}"),
        )
    })
}

/// Server time of an upload: from the arrival of the `upload` request, which the client
/// sends immediately before the payload (so it marks the first byte on the wire), to the
/// arrival of the last byte. Timing from the first *frame* instead would leave that whole
/// frame's transfer out of the duration and overstate small uploads.
fn finish_upload(session: &Session, up: Upload) -> ServerMsg {
    let server_ms = up.started.elapsed().as_secs_f64() * 1000.0;
    tracing::debug!(
        session = %session.id(),
        id = up.id,
        bytes = up.received,
        server_ms,
        mbps = mbps(up.received, server_ms),
        "upload done"
    );
    ServerMsg::UploadDone {
        id: up.id,
        bytes: up.received,
        server_ms,
    }
}

fn mbps(bytes: u64, ms: f64) -> f64 {
    if ms > 0.0 {
        bytes as f64 * 8.0 / ms / 1000.0
    } else {
        0.0
    }
}

/// Sends `bytes` of payload, watching the socket (and the session's abort signal) for an
/// `Abort`. Returns the message that ends the transfer, or `Break` if the socket is gone.
async fn run_download(
    sink: &mut SplitSink<WebSocket, Message>,
    stream: &mut SplitStream<WebSocket>,
    abort: &mut watch::Receiver<u64>,
    payload: &Payload,
    guard: &SocketGuard,
    id: u32,
    bytes: u64,
) -> ControlFlow<(), ServerMsg> {
    let session = guard.session().clone();
    let transfer = guard.begin_transfer();
    let started = Instant::now();
    let mut protocol_error = false;

    let completed = {
        let send_all = async {
            for chunk in payload.chunks(bytes, CHUNK_SIZE) {
                let n = chunk.len() as u64;
                // Awaiting each send applies the client's backpressure.
                sink.send(Message::Binary(chunk)).await?;
                session.add_sent(n);
            }
            Ok::<(), axum::Error>(())
        };
        tokio::pin!(send_all);
        loop {
            tokio::select! {
                res = &mut send_all => match res {
                    Ok(()) => break true,
                    Err(_) => return ControlFlow::Break(()),
                },
                changed = abort.changed() => {
                    if changed.is_err() {
                        return ControlFlow::Break(());
                    }
                    break false;
                }
                frame = stream.next() => match classify(frame) {
                    Incoming::Closed => return ControlFlow::Break(()),
                    Incoming::Msg(ClientMsg::Abort) => break false,
                    Incoming::Binary(b) => session.add_received(b.len() as u64),
                    Incoming::Other => {}
                    _ => protocol_error = true,
                },
            }
        }
    };
    drop(transfer);
    session.touch();
    let server_ms = started.elapsed().as_secs_f64() * 1000.0;
    if !completed {
        tracing::debug!(session = %session.id(), id, "download aborted");
        return ControlFlow::Continue(aborted(id));
    }
    tracing::debug!(
        session = %session.id(),
        id,
        bytes,
        server_ms,
        mbps = mbps(bytes, server_ms),
        "download done"
    );
    if protocol_error {
        tracing::debug!(session = %session.id(), "unexpected message during a download ignored");
    }
    ControlFlow::Continue(ServerMsg::DownloadDone { id, server_ms })
}

// ---------------------------------------------------------------------------------------
// Control socket

async fn control_socket(mut socket: WebSocket, state: AppState, peer_ip: Option<IpAddr>) {
    let Some(hello) = handshake(&mut socket, &state.config, Role::Control).await else {
        return;
    };
    let (tx, rx) = mpsc::channel::<ServerMsg>(CONTROL_QUEUE);
    let guard = match state.measure.sessions.attach(
        hello.session,
        Role::Control,
        hello.device,
        Some(tx.clone()),
    ) {
        Ok(g) => g,
        Err(r) => return reject(&mut socket, r.code, &r.message, close_code_for(r.code)).await,
    };
    if send_msg(&mut socket, &welcome(&state)).await.is_err() {
        return;
    }
    if state.config.presence {
        let peers = presence::snapshot_for(&state.measure.sessions, hello.session);
        if send_msg(&mut socket, &peers).await.is_err() {
            return;
        }
    }
    let (sink, mut stream) = socket.split();
    let writer = tokio::spawn(control_writer(sink, rx));
    let ctx = ControlCtx {
        state: &state,
        session: guard.session().clone(),
        tx: tx.clone(),
        peer_ip,
        reported_phase: AtomicBool::new(false),
    };

    let close: Option<(ErrorCode, &str, u16)> = loop {
        let incoming = match tokio::time::timeout(IDLE_TIMEOUT, stream.next()).await {
            Ok(frame) => classify(frame),
            Err(_) => break Some((ErrorCode::Timeout, "idle timeout", CLOSE_NORMAL)),
        };
        ctx.session.touch();
        match incoming {
            Incoming::Closed => break None,
            Incoming::Other => {}
            Incoming::TooBig => {
                break Some((ErrorCode::BadRequest, "frame too big", CLOSE_TOO_BIG));
            }
            Incoming::Invalid(e) => ctx.reply(ServerMsg::error(ErrorCode::BadRequest, e)).await,
            Incoming::Binary(_) => {
                ctx.reply(ServerMsg::error(
                    ErrorCode::BadRequest,
                    "binary frames are not accepted on the control socket",
                ))
                .await;
            }
            Incoming::Msg(msg) => ctx.dispatch(msg).await,
        }
    };

    // Detach (drops the registry's sender), then let the writer drain and hand the sink back.
    let session = ctx.session.clone();
    let reported_phase = ctx.reported_phase.load(Ordering::Relaxed);
    drop(ctx);
    drop(guard);
    drop(tx);
    if session.control_sockets() == 0 {
        state.groups.disconnected(session.id());
    }
    if reported_phase {
        // E.g. a run's own control socket closed by an abort while the page's presence
        // socket stays: don't leave the device "testing" forever.
        state.measure.sessions.clear_phase(session.id());
    }
    let Ok(mut sink) = writer.await else { return };
    if let Some((code, message, close_code)) = close {
        reject(&mut sink, code, message, close_code).await;
    }
}

/// Forwards queued messages to the socket and sends a WebSocket Ping every
/// [`CONTROL_PING_INTERVAL`]. Ends when every sender is dropped or the socket fails, and
/// returns the sink so the caller can close it.
async fn control_writer(
    mut sink: SplitSink<WebSocket, Message>,
    mut rx: mpsc::Receiver<ServerMsg>,
) -> SplitSink<WebSocket, Message> {
    let mut ping = tokio::time::interval_at(
        Instant::now() + CONTROL_PING_INTERVAL,
        CONTROL_PING_INTERVAL,
    );
    loop {
        let res = tokio::select! {
            msg = rx.recv() => match msg {
                Some(msg) => send_msg(&mut sink, &msg).await,
                None => break,
            },
            _ = ping.tick() => sink.send(Message::Ping(Bytes::new())).await,
        };
        if res.is_err() {
            // The reader sees the failure too; keep draining so senders don't block.
            rx.close();
            while rx.recv().await.is_some() {}
            break;
        }
    }
    sink
}

/// Per-connection context of a control socket. Add new message kinds to
/// [`ControlCtx::dispatch`].
struct ControlCtx<'a> {
    state: &'a AppState,
    session: Arc<Session>,
    tx: mpsc::Sender<ServerMsg>,
    /// The client's address (for the server link speed of group tests).
    peer_ip: Option<IpAddr>,
    /// The last `Status` this socket sent had a test phase.
    reported_phase: AtomicBool,
}

impl ControlCtx<'_> {
    async fn reply(&self, msg: ServerMsg) {
        // Fails only once the writer is gone, and then the reader stops too.
        let _ = self.tx.send(msg).await;
    }

    async fn dispatch(&self, msg: ClientMsg) {
        let sessions = &self.state.measure.sessions;
        let id = self.session.id();
        match msg {
            ClientMsg::Status { phase, label } => {
                self.reported_phase
                    .store(phase.is_some_and(|p| p != Phase::Idle), Ordering::Relaxed);
                sessions.set_status(id, phase, label);
            }
            ClientMsg::SetHidden { hidden } => sessions.set_hidden(id, hidden),
            ClientMsg::Abort => {
                tracing::debug!(session = %id, "abort requested on the control socket");
                self.session.abort();
            }
            ClientMsg::RtcOffer { sdp } => self.rtc_offer(sdp).await,
            ClientMsg::Ice {
                candidate,
                sdp_mid,
                sdp_m_line_index,
            } => self.rtc_ice(candidate, sdp_mid, sdp_m_line_index).await,
            ClientMsg::GroupCreate => {
                if let Err(e) = self.state.groups.create(id) {
                    self.reply(e.to_msg()).await;
                }
            }
            ClientMsg::GroupJoin { group } => {
                if let Err(e) = self.state.groups.join(id, group) {
                    self.reply(e.to_msg()).await;
                }
            }
            ClientMsg::GroupLeave => {
                if let Err(e) = self.state.groups.leave(id) {
                    self.reply(e.to_msg()).await;
                }
            }
            ClientMsg::GroupStart => {
                let ctx = StartContext {
                    storage: self.state.storage.clone(),
                    client_ip: self.peer_ip,
                };
                if let Err(e) = self.state.groups.start(id, ctx) {
                    self.reply(e.to_msg()).await;
                }
            }
            ClientMsg::Hello { .. } => {
                self.reply(ServerMsg::error(
                    ErrorCode::BadRequest,
                    "already said hello",
                ))
                .await;
            }
            ClientMsg::Ping { .. } | ClientMsg::Download { .. } | ClientMsg::Upload { .. } => {
                self.reply(ServerMsg::error(
                    ErrorCode::BadRequest,
                    "use the probe and data sockets for measurements",
                ))
                .await;
            }
        }
    }

    fn rtc(&self) -> Option<Arc<dyn RtcSignaling>> {
        self.state.measure.rtc.clone()
    }

    async fn rtc_offer(&self, sdp: String) {
        let Some(rtc) = self.rtc() else {
            return self.rtc_unavailable().await;
        };
        // The answer may take a while (ICE gathering); don't block the reader.
        let tx = self.tx.clone();
        let id = self.session.id();
        tokio::spawn(async move {
            let reply = match rtc.answer(id, sdp).await {
                Ok(sdp) => ServerMsg::RtcAnswer { sdp },
                Err(e) => ServerMsg::error(ErrorCode::Internal, e),
            };
            let _ = tx.send(reply).await;
        });
    }

    async fn rtc_ice(&self, candidate: String, mid: Option<String>, index: Option<u16>) {
        let Some(rtc) = self.rtc() else {
            return self.rtc_unavailable().await;
        };
        let tx = self.tx.clone();
        let id = self.session.id();
        tokio::spawn(async move {
            if let Err(e) = rtc.remote_candidate(id, candidate, mid, index).await {
                let _ = tx.send(ServerMsg::error(ErrorCode::Internal, e)).await;
            }
        });
    }

    async fn rtc_unavailable(&self) {
        self.reply(ServerMsg::error(
            ErrorCode::NotFound,
            "packet loss test unavailable",
        ))
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_cover_exact_bytes_and_cycle() {
        let p = Payload::random(1000);
        let sizes: Vec<usize> = p.chunks(2500, 300).map(|c| c.len()).collect();
        assert_eq!(sizes.iter().sum::<usize>(), 2500);
        assert!(sizes.iter().all(|n| *n <= 300));
        assert_eq!(*sizes.last().unwrap(), 2500 % 300);
        assert_eq!(p.chunks(0, 300).count(), 0);
        // Chunks larger than the buffer are clamped.
        assert!(p.chunks(5000, 4096).all(|c| c.len() <= 1000));
        // Zero-copy: slices share the buffer.
        let a = p.chunks(10, 10).next().unwrap();
        assert_eq!(a.as_ptr(), p.0.as_ptr());
    }
}
