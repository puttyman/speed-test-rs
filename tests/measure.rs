//! Integration tests for the measurement endpoints: a real server on an ephemeral port,
//! driven by `tokio-tungstenite` and `reqwest` clients.
//!
//! Run with `cargo test --features ssr`. The loopback throughput check is ignored by
//! default: `cargo test --release --features ssr --test measure -- --ignored throughput --nocapture`.

#![cfg(feature = "ssr")]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use speed_test_rs::model::protocol::{
    ClientMsg, DeviceInfo, DeviceKind, ErrorCode, PROTOCOL_VERSION, Phase, Role, ServerMsg,
};
use speed_test_rs::server::config::{Config, ServeArgs};
use speed_test_rs::server::rtc_hook::RtcSignaling;
use speed_test_rs::server::{AppState, router};
use tokio::net::TcpStream;
use tokio::time::{Instant, timeout};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use uuid::Uuid;

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

const TOKEN: &str = "s3cret-token";

/// Server options for a test.
#[derive(Default)]
struct Opts {
    max_sessions: Option<usize>,
    max_bytes: Option<u64>,
    token: bool,
    rtc: Option<Arc<dyn RtcSignaling>>,
}

struct Server {
    addr: SocketAddr,
    state: AppState,
}

impl Server {
    async fn start(opts: Opts) -> Self {
        let args = ServeArgs {
            listen: vec!["127.0.0.1:0".into()],
            max_sessions: opts.max_sessions,
            max_bytes: opts.max_bytes,
            server_name: Some("test-server".into()),
            ..ServeArgs::default()
        };
        let token = opts.token;
        let config = Config::resolve(args, move |key| {
            (token && key == "SPEEDTEST_AUTH_TOKEN").then(|| TOKEN.to_owned())
        })
        .expect("valid config");
        let mut state = AppState::new(config);
        state.measure.rtc = opts.rtc;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = router(state.clone());
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        Self { addr, state }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    async fn connect(&self, path: &str) -> Ws {
        let (ws, _) = connect_async(format!("ws://{}{path}", self.addr))
            .await
            .expect("websocket connect");
        ws
    }

    /// Connects to `path`, says hello with `role`, and returns the first reply.
    async fn hello_as(
        &self,
        path: &str,
        session: Uuid,
        role: Role,
        auth: Option<&str>,
    ) -> (Ws, ServerMsg) {
        let mut ws = self.connect(path).await;
        let device = (role == Role::Control).then(|| DeviceInfo {
            device_id: "dev-1".into(),
            device_name: "Test Laptop".into(),
            device_kind: DeviceKind::Laptop,
        });
        send(
            &mut ws,
            &ClientMsg::Hello {
                session,
                role,
                device,
                auth: auth.map(str::to_owned),
            },
        )
        .await;
        let reply = recv(&mut ws).await;
        (ws, reply)
    }

    /// Like [`hello_as`] with the matching role, asserting a `Welcome`.
    async fn open(&self, role: Role, session: Uuid) -> Ws {
        let (ws, reply) = self.hello_as(path_for(role), session, role, None).await;
        assert!(matches!(reply, ServerMsg::Welcome { .. }), "{reply:?}");
        ws
    }
}

fn path_for(role: Role) -> &'static str {
    match role {
        Role::Control => "/ws/control",
        Role::Data => "/ws/data",
        Role::Probe => "/ws/probe",
    }
}

async fn send(ws: &mut Ws, msg: &ClientMsg) {
    ws.send(Message::text(msg.to_json())).await.expect("send");
}

/// The next frame, skipping WebSocket pings/pongs.
async fn next_frame(ws: &mut Ws) -> Option<Message> {
    loop {
        let frame = timeout(Duration::from_secs(20), ws.next())
            .await
            .expect("timed out waiting for a frame");
        match frame {
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
            Some(Ok(m)) => return Some(m),
            Some(Err(_)) | None => return None,
        }
    }
}

/// The next JSON message (panics on binary frames or close).
async fn recv(ws: &mut Ws) -> ServerMsg {
    match next_frame(ws).await {
        Some(Message::Text(t)) => ServerMsg::from_json(t.as_str()).expect("valid server JSON"),
        other => panic!("expected a text frame, got {other:?}"),
    }
}

async fn expect_error(ws: &mut Ws, code: ErrorCode) -> String {
    match recv(ws).await {
        ServerMsg::Error { code: c, message } if c == code => message,
        other => panic!("expected Error {code:?}, got {other:?}"),
    }
}

/// After a rejection the server closes the socket.
async fn expect_closed(ws: &mut Ws) {
    match next_frame(ws).await {
        None | Some(Message::Close(_)) => {}
        other => panic!("expected close, got {other:?}"),
    }
}

/// Reads a download's binary frames until `DownloadDone{id}`; returns (bytes, frames).
async fn read_download(ws: &mut Ws, id: u32, per_frame_delay: Option<Duration>) -> (u64, usize) {
    let mut total = 0u64;
    let mut frames = 0usize;
    loop {
        match next_frame(ws).await {
            Some(Message::Binary(b)) => {
                assert!(b.len() <= 64 * 1024, "frame of {} bytes", b.len());
                total += b.len() as u64;
                frames += 1;
                if let Some(d) = per_frame_delay {
                    tokio::time::sleep(d).await;
                }
            }
            Some(Message::Text(t)) => match ServerMsg::from_json(t.as_str()).unwrap() {
                ServerMsg::DownloadDone {
                    id: done,
                    server_ms,
                } => {
                    assert_eq!(done, id);
                    assert!(server_ms >= 0.0);
                    return (total, frames);
                }
                other => panic!("unexpected {other:?} during download"),
            },
            other => panic!("unexpected {other:?} during download"),
        }
    }
}

async fn upload(ws: &mut Ws, id: u32, bytes: usize) -> ServerMsg {
    send(
        ws,
        &ClientMsg::Upload {
            id,
            bytes: bytes as u64,
        },
    )
    .await;
    let chunk = vec![0xA5u8; 64 * 1024];
    let mut left = bytes;
    while left > 0 {
        let n = left.min(chunk.len());
        ws.send(Message::binary(chunk[..n].to_vec())).await.unwrap();
        left -= n;
    }
    recv(ws).await
}

fn snapshot_of(
    server: &Server,
    id: Uuid,
) -> Option<speed_test_rs::server::sessions::SessionSnapshot> {
    server
        .state
        .measure
        .sessions
        .snapshot()
        .into_iter()
        .find(|s| s.id == id)
}

// -------------------------------------------------------------------------------------
// Handshake

#[tokio::test]
async fn hello_welcome_on_every_role() {
    let server = Server::start(Opts::default()).await;
    let session = Uuid::new_v4();
    for role in [Role::Control, Role::Data, Role::Probe] {
        let (_ws, reply) = server.hello_as(path_for(role), session, role, None).await;
        match reply {
            ServerMsg::Welcome {
                server_version,
                server_name,
                protocol_version,
                limits,
            } => {
                assert_eq!(server_version, env!("CARGO_PKG_VERSION"));
                assert_eq!(server_name, "test-server");
                assert_eq!(protocol_version, PROTOCOL_VERSION);
                assert_eq!(limits.max_concurrent_sessions, 16);
                assert_eq!(limits.max_bytes_per_request, 2_000_000_000);
                assert_eq!(limits.max_streams, 8);
            }
            other => panic!("expected welcome, got {other:?}"),
        }
    }
    // The control socket's device info is stored on the session.
    let snap = snapshot_of(&server, session).unwrap();
    assert_eq!(snap.device.unwrap().device_name, "Test Laptop");
}

#[tokio::test]
async fn role_mismatch_and_missing_hello_are_rejected() {
    let server = Server::start(Opts::default()).await;
    let (mut ws, reply) = server
        .hello_as("/ws/data", Uuid::new_v4(), Role::Probe, None)
        .await;
    assert!(
        matches!(reply, ServerMsg::Error { code: ErrorCode::BadRequest, ref message } if message.contains("role")),
        "{reply:?}"
    );
    expect_closed(&mut ws).await;

    let mut ws = server.connect("/ws/probe").await;
    send(&mut ws, &ClientMsg::Ping { seq: 1 }).await;
    expect_error(&mut ws, ErrorCode::BadRequest).await;
    expect_closed(&mut ws).await;
}

#[tokio::test]
async fn auth_token_on_websockets() {
    let server = Server::start(Opts {
        token: true,
        ..Opts::default()
    })
    .await;
    for auth in [None, Some("wrong")] {
        let (mut ws, reply) = server
            .hello_as("/ws/control", Uuid::new_v4(), Role::Control, auth)
            .await;
        assert!(
            matches!(
                reply,
                ServerMsg::Error {
                    code: ErrorCode::Unauthorized,
                    ..
                }
            ),
            "{reply:?}"
        );
        expect_closed(&mut ws).await;
    }
    let (_ws, reply) = server
        .hello_as("/ws/probe", Uuid::new_v4(), Role::Probe, Some(TOKEN))
        .await;
    assert!(matches!(reply, ServerMsg::Welcome { .. }), "{reply:?}");
}

// -------------------------------------------------------------------------------------
// Probe

#[tokio::test]
async fn ping_pong() {
    let server = Server::start(Opts::default()).await;
    let mut ws = server.open(Role::Probe, Uuid::new_v4()).await;
    for seq in 0..20 {
        send(&mut ws, &ClientMsg::Ping { seq }).await;
        assert_eq!(recv(&mut ws).await, ServerMsg::Pong { seq });
    }
    // Pipelined pings are answered in order.
    for seq in 100..110 {
        send(&mut ws, &ClientMsg::Ping { seq }).await;
    }
    for seq in 100..110 {
        assert_eq!(recv(&mut ws).await, ServerMsg::Pong { seq });
    }
}

// -------------------------------------------------------------------------------------
// Data

#[tokio::test]
async fn download_exact_bytes_then_done() {
    let server = Server::start(Opts::default()).await;
    let mut ws = server.open(Role::Data, Uuid::new_v4()).await;
    for (id, bytes) in [
        (1u32, 10 * 64 * 1024 + 123u64),
        (2, 1),
        (3, 5_000_000),
        (4, 0),
    ] {
        send(&mut ws, &ClientMsg::Download { id, bytes }).await;
        let (total, frames) = read_download(&mut ws, id, None).await;
        assert_eq!(total, bytes, "download {id}");
        assert_eq!(frames as u64, bytes.div_ceil(64 * 1024), "download {id}");
    }
}

#[tokio::test]
async fn upload_exact_bytes() {
    let server = Server::start(Opts::default()).await;
    let session = Uuid::new_v4();
    let mut ws = server.open(Role::Data, session).await;
    for (id, bytes) in [(1u32, 3_000_000usize), (2, 100_000), (3, 1), (4, 0)] {
        match upload(&mut ws, id, bytes).await {
            ServerMsg::UploadDone {
                id: done,
                bytes: got,
                server_ms,
            } => {
                assert_eq!(done, id);
                assert_eq!(got, bytes as u64);
                assert!((0.0..10_000.0).contains(&server_ms), "{server_ms}");
            }
            other => panic!("expected upload_done, got {other:?}"),
        }
    }
    let snap = snapshot_of(&server, session).unwrap();
    assert_eq!(snap.bytes_received, 3_100_001);
}

#[tokio::test]
async fn max_bytes_enforced() {
    let server = Server::start(Opts {
        max_bytes: Some(1000),
        ..Opts::default()
    })
    .await;
    let mut ws = server.open(Role::Data, Uuid::new_v4()).await;
    send(&mut ws, &ClientMsg::Download { id: 1, bytes: 1001 }).await;
    expect_error(&mut ws, ErrorCode::LimitExceeded).await;
    send(&mut ws, &ClientMsg::Upload { id: 2, bytes: 1001 }).await;
    expect_error(&mut ws, ErrorCode::LimitExceeded).await;
    // The socket stays usable.
    send(&mut ws, &ClientMsg::Download { id: 3, bytes: 1000 }).await;
    assert_eq!(read_download(&mut ws, 3, None).await.0, 1000);

    let http = reqwest::Client::new();
    let res = http
        .get(server.url("/api/__down?bytes=1001"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 413);
    let res = http
        .post(server.url("/api/__up"))
        .body(vec![0u8; 1001])
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 413);
}

#[tokio::test]
async fn busy_when_max_sessions_reached() {
    let server = Server::start(Opts {
        max_sessions: Some(1),
        ..Opts::default()
    })
    .await;
    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
    let _a_data = server.open(Role::Data, a).await;
    // A second stream of the same session is fine.
    let _a_data2 = server.open(Role::Data, a).await;
    // Presence-only sockets don't count.
    let _b_control = server.open(Role::Control, b).await;
    let _b_probe = server.open(Role::Probe, b).await;

    let (mut ws, reply) = server.hello_as("/ws/data", b, Role::Data, None).await;
    assert!(
        matches!(
            reply,
            ServerMsg::Error {
                code: ErrorCode::Busy,
                ..
            }
        ),
        "{reply:?}"
    );
    expect_closed(&mut ws).await;

    let res = reqwest::get(server.url(&format!("/api/__down?bytes=10&session={b}")))
        .await
        .unwrap();
    assert_eq!(res.status(), 503);
    // Without a session the HTTP endpoints aren't tracked.
    let res = reqwest::get(server.url("/api/__down?bytes=10"))
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn abort_stops_a_download() {
    let server = Server::start(Opts::default()).await;
    let session = Uuid::new_v4();
    let mut control = server.open(Role::Control, session).await;
    let mut ws = server.open(Role::Data, session).await;

    for via_control in [false, true] {
        send(
            &mut ws,
            &ClientMsg::Download {
                id: 7,
                bytes: 1_000_000_000,
            },
        )
        .await;
        // Read a little, then abort.
        let mut got = 0u64;
        while got < 1_000_000 {
            match next_frame(&mut ws).await {
                Some(Message::Binary(b)) => got += b.len() as u64,
                other => panic!("unexpected {other:?}"),
            }
        }
        if via_control {
            send(&mut control, &ClientMsg::Abort).await;
        } else {
            send(&mut ws, &ClientMsg::Abort).await;
        }
        // Drain until the boundary marker.
        loop {
            match next_frame(&mut ws).await {
                Some(Message::Binary(b)) => got += b.len() as u64,
                Some(Message::Text(t)) => {
                    match ServerMsg::from_json(t.as_str()).unwrap() {
                        ServerMsg::Error {
                            code: ErrorCode::Conflict,
                            message,
                        } => assert!(message.contains("aborted"), "{message}"),
                        other => panic!("unexpected {other:?}"),
                    }
                    break;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
        assert!(got < 1_000_000_000);
        // The socket is usable again.
        send(&mut ws, &ClientMsg::Download { id: 8, bytes: 777 }).await;
        assert_eq!(read_download(&mut ws, 8, None).await.0, 777);
    }
    assert_eq!(server.state.measure.sessions.active_transfer_count(), 0);
}

#[tokio::test]
async fn counters_rates_and_overlap() {
    let server = Server::start(Opts::default()).await;
    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
    let mut wa = server.open(Role::Data, a).await;
    let mut wb = server.open(Role::Data, b).await;
    const N: u64 = 16 * 1024 * 1024;

    send(&mut wa, &ClientMsg::Download { id: 1, bytes: N }).await;
    send(&mut wb, &ClientMsg::Download { id: 1, bytes: N }).await;
    // Paced readers (~1 ms per 64 KiB frame) keep the transfers running for a while.
    let delay = Some(Duration::from_millis(1));
    let ra = tokio::spawn(async move { (read_download(&mut wa, 1, delay).await, wa) });
    let rb = tokio::spawn(async move { (read_download(&mut wb, 1, delay).await, wb) });

    let sessions = server.state.measure.sessions.clone();
    let mut max_rate = 0f64;
    let mut max_active = 0;
    while !(ra.is_finished() && rb.is_finished()) {
        max_active = max_active.max(sessions.active_transfer_count());
        if let Some(s) = snapshot_of(&server, a) {
            max_rate = max_rate.max(s.rates.down_bps);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let ((total_a, _), mut wa) = ra.await.unwrap();
    let ((total_b, _), _wb) = rb.await.unwrap();
    assert_eq!((total_a, total_b), (N, N));
    assert_eq!(max_active, 2);
    assert!(max_rate > 1e6, "rate sampler saw {max_rate} bps");

    let sa = snapshot_of(&server, a).unwrap();
    assert_eq!(sa.bytes_sent, N);
    assert_eq!(sa.bytes_received, 0);
    assert_eq!(sa.data_sockets, 1);

    let c = sessions.concurrency_for(a, None).unwrap();
    assert_eq!(c.peers_max, 1, "{c:?}");
    assert!(c.overlap_ms > 0.0 && c.overlap_ratio > 0.5, "{c:?}");

    // An upload after B is done doesn't overlap with anything.
    let before = Instant::now();
    assert!(matches!(
        upload(&mut wa, 2, 1_000_000).await,
        ServerMsg::UploadDone { .. }
    ));
    let after = Instant::now();
    let c = sessions.concurrency_for(a, Some(before..after)).unwrap();
    assert_eq!(c.peers_max, 0, "{c:?}");
    assert_eq!(snapshot_of(&server, a).unwrap().bytes_received, 1_000_000);

    // Closing every socket leaves a tombstone that still answers overlap queries.
    drop(wa);
    drop(_wb);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !snapshot_of(&server, a).unwrap().closed {
        assert!(Instant::now() < deadline, "session not closed");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(sessions.concurrency_for(a, None).unwrap().peers_max, 1);
}

// -------------------------------------------------------------------------------------
// Control

struct EchoRtc;

#[async_trait::async_trait]
impl RtcSignaling for EchoRtc {
    async fn answer(&self, _session: Uuid, offer_sdp: String) -> Result<String, String> {
        Ok(format!("answer-to:{offer_sdp}"))
    }
}

#[tokio::test]
async fn control_messages() {
    let server = Server::start(Opts::default()).await;
    let session = Uuid::new_v4();
    let mut ws = server.open(Role::Control, session).await;

    send(
        &mut ws,
        &ClientMsg::Status {
            phase: Some(Phase::Download),
            label: Some("Desk".into()),
        },
    )
    .await;
    send(&mut ws, &ClientMsg::RtcOffer { sdp: "v=0".into() }).await;
    let msg = expect_error(&mut ws, ErrorCode::NotFound).await;
    assert_eq!(msg, "packet loss test unavailable");
    send(&mut ws, &ClientMsg::GroupCreate).await;
    expect_error(&mut ws, ErrorCode::NotFound).await;
    send(&mut ws, &ClientMsg::Ping { seq: 1 }).await;
    expect_error(&mut ws, ErrorCode::BadRequest).await;
    ws.send(Message::text("{\"t\":\"nope\"}")).await.unwrap();
    expect_error(&mut ws, ErrorCode::BadRequest).await;

    let snap = snapshot_of(&server, session).unwrap();
    assert_eq!(snap.phase, Some(Phase::Download));
    assert_eq!(snap.label.as_deref(), Some("Desk"));
    assert_eq!(snap.control_sockets, 1);

    // The server can push to a control socket at any time.
    let pushed = server
        .state
        .measure
        .sessions
        .send_to(session, &ServerMsg::Pong { seq: 42 });
    assert_eq!(pushed, 1);
    assert_eq!(recv(&mut ws).await, ServerMsg::Pong { seq: 42 });
}

#[tokio::test]
async fn rtc_offer_goes_to_the_hook() {
    let server = Server::start(Opts {
        rtc: Some(Arc::new(EchoRtc)),
        ..Opts::default()
    })
    .await;
    let mut ws = server.open(Role::Control, Uuid::new_v4()).await;
    send(&mut ws, &ClientMsg::RtcOffer { sdp: "v=0".into() }).await;
    assert_eq!(
        recv(&mut ws).await,
        ServerMsg::RtcAnswer {
            sdp: "answer-to:v=0".into()
        }
    );
}

// -------------------------------------------------------------------------------------
// HTTP

fn header<'a>(res: &'a reqwest::Response, name: &str) -> &'a str {
    res.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
}

#[tokio::test]
async fn http_down_and_up() {
    let server = Server::start(Opts::default()).await;
    let http = reqwest::Client::new();
    let session = Uuid::new_v4();

    let res = http
        .get(server.url(&format!("/api/__down?bytes=1000000&session={session}")))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(header(&res, "content-type"), "application/octet-stream");
    assert_eq!(header(&res, "cache-control"), "no-store");
    assert_eq!(header(&res, "content-length"), "1000000");
    assert!(header(&res, "server-timing").starts_with("app;dur="));
    assert_eq!(res.bytes().await.unwrap().len(), 1_000_000);

    // Latency probe.
    let res = http
        .get(server.url("/api/__down?bytes=0"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert!(header(&res, "server-timing").starts_with("app;dur="));
    assert!(res.bytes().await.unwrap().is_empty());

    let res = http
        .post(server.url(&format!("/api/__up?session={session}")))
        .body(vec![7u8; 3_000_000])
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert!(header(&res, "server-timing").starts_with("app;dur="));
    assert_eq!(header(&res, "cache-control"), "no-store");
    let body: serde_json::Value = serde_json::from_str(&res.text().await.unwrap()).unwrap();
    assert_eq!(body["bytes"], 3_000_000);

    let snap = snapshot_of(&server, session).unwrap();
    assert_eq!(snap.bytes_sent, 1_000_000);
    assert_eq!(snap.bytes_received, 3_000_000);
    assert_eq!(snap.active_transfers, 0);
    assert!(
        snap.closed,
        "an HTTP-only session is a tombstone between requests"
    );
}

#[tokio::test]
async fn http_info() {
    let server = Server::start(Opts::default()).await;
    let text = reqwest::get(server.url("/api/info"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let info: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(info["server_name"], "test-server");
    assert_eq!(info["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(info["protocol_version"], PROTOCOL_VERSION);
    assert_eq!(info["client_ip"], "127.0.0.1");
    assert_eq!(info["udp_port"], 3478);
    assert_eq!(info["tls"], false);
    assert_eq!(info["presence"], true);
    assert_eq!(info["limits"]["max_concurrent_sessions"], 16);
    assert!(info["history"].is_boolean());
}

#[tokio::test]
async fn http_auth_layer() {
    let server = Server::start(Opts {
        token: true,
        ..Opts::default()
    })
    .await;
    let http = reqwest::Client::new();
    let status = |res: reqwest::Response| res.status().as_u16();

    // Health stays open.
    let res = http.get(server.url("/api/health")).send().await.unwrap();
    assert_eq!(status(res), 200);

    for path in ["/api/info", "/api/__down?bytes=10"] {
        let res = http.get(server.url(path)).send().await.unwrap();
        assert_eq!(res.status(), 401, "{path}");
        assert_eq!(header(&res, "www-authenticate"), "Bearer");
        let res = http
            .get(server.url(path))
            .bearer_auth("wrong")
            .send()
            .await
            .unwrap();
        assert_eq!(status(res), 401, "{path}");
        let res = http
            .get(server.url(path))
            .bearer_auth(TOKEN)
            .send()
            .await
            .unwrap();
        assert_eq!(status(res), 200, "{path}");
        let res = http
            .get(server.url(path))
            .header("cookie", format!("a=b; speedtest_token={TOKEN}"))
            .send()
            .await
            .unwrap();
        assert_eq!(status(res), 200, "{path}");
    }
    let res = http
        .post(server.url("/api/__up"))
        .body(vec![0u8; 10])
        .send()
        .await
        .unwrap();
    assert_eq!(status(res), 401);
}

// -------------------------------------------------------------------------------------
// Throughput

/// Loopback WebSocket download of 1 GB. Meaningful in release mode only:
/// `cargo test --release --features ssr --test measure -- --ignored throughput --nocapture`
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn throughput_loopback_download_1gb() {
    const N: u64 = 1_000_000_000;
    let server = Server::start(Opts::default()).await;
    let mut ws = server.open(Role::Data, Uuid::new_v4()).await;
    send(&mut ws, &ClientMsg::Download { id: 1, bytes: N }).await;
    let mut total = 0u64;
    let mut first: Option<Instant> = None;
    let mut last = Instant::now();
    let server_ms = loop {
        match ws.next().await {
            Some(Ok(Message::Binary(b))) => {
                first.get_or_insert_with(Instant::now);
                total += b.len() as u64;
                last = Instant::now();
            }
            Some(Ok(Message::Text(t))) => match ServerMsg::from_json(t.as_str()).unwrap() {
                ServerMsg::DownloadDone { server_ms, .. } => break server_ms,
                other => panic!("unexpected {other:?}"),
            },
            other => panic!("unexpected {other:?}"),
        }
    };
    assert_eq!(total, N);
    let secs = (last - first.unwrap()).as_secs_f64();
    let gbps = N as f64 * 8.0 / secs / 1e9;
    println!(
        "loopback WS download: {N} bytes in {:.3} s (client), {:.1} ms (server) = {gbps:.2} Gbps",
        secs, server_ms
    );
    assert!(gbps > 1.0, "{gbps:.2} Gbps");

    // Upload in the other direction, timed by the server.
    let started = Instant::now();
    match upload(&mut ws, 2, 1_000_000_000).await {
        ServerMsg::UploadDone {
            bytes, server_ms, ..
        } => {
            let gbps = bytes as f64 * 8.0 / (server_ms / 1000.0) / 1e9;
            println!(
                "loopback WS upload: {bytes} bytes in {server_ms:.1} ms (server), {:.3} s (client) = {gbps:.2} Gbps",
                started.elapsed().as_secs_f64()
            );
        }
        other => panic!("unexpected {other:?}"),
    }
}
