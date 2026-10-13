//! Integration tests for live presence, group tests and overlap flagging (SPEC §5.11): a real
//! server on an ephemeral port, driven by `tokio-tungstenite` and `reqwest` clients.
//!
//! Run with `cargo test --features ssr --test presence`. The group tests run the full
//! short `concurrent` plan (`GROUP_SECONDS` per direction, about 9.5 s), to keep CI fast.

#![cfg(feature = "ssr")]

use std::net::SocketAddr;
use std::time::Duration;

use futures_util::SinkExt;
use futures_util::stream::{SplitSink, StreamExt};
use speed_test_rs::model::profile::Profile;
use speed_test_rs::model::protocol::{
    ClientMsg, DeviceInfo, DeviceKind, ErrorCode, GroupState, Peer, PeerState, Phase, Role,
    ServerMsg,
};
use speed_test_rs::model::result::{GroupResult, RunResult};
use speed_test_rs::server::config::{Config, ServeArgs};
use speed_test_rs::server::storage::Storage;
use speed_test_rs::server::{AppState, router};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep, sleep_until, timeout};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use uuid::Uuid;

/// Seconds of download and of upload in the group tests here (production default: 10).
const GROUP_SECONDS: u32 = 4;

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct Server {
    addr: SocketAddr,
    _dir: Option<tempfile::TempDir>,
}

impl Server {
    async fn start(presence: bool, history: bool) -> Self {
        let args = ServeArgs {
            listen: vec!["127.0.0.1:0".into()],
            server_name: Some("test-server".into()),
            no_presence: !presence,
            no_history: !history,
            server_link_mbps: Some(10_000),
            group_test_seconds: Some(GROUP_SECONDS),
            ..ServeArgs::default()
        };
        let config = Config::resolve(args, |_| None).expect("valid config");
        let mut state = AppState::new(config);
        let dir = if history {
            let dir = tempfile::tempdir().unwrap();
            state.storage = Some(Storage::open(&dir.path().join("t.db")).await.unwrap());
            Some(dir)
        } else {
            None
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = router(state);
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        Self { addr, _dir: dir }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    /// Opens a socket of `role` for `session` and waits for `Welcome`.
    async fn open(&self, role: Role, session: Uuid, name: &str) -> Ws {
        let path = match role {
            Role::Control => "/ws/control",
            Role::Data => "/ws/data",
            Role::Probe => "/ws/probe",
        };
        let (mut ws, _) = connect_async(format!("ws://{}{path}", self.addr))
            .await
            .expect("websocket connect");
        let device = (role == Role::Control).then(|| DeviceInfo {
            device_id: format!("id-{name}"),
            device_name: name.to_owned(),
            device_kind: DeviceKind::Laptop,
        });
        let hello = ClientMsg::Hello {
            session,
            role,
            device,
            auth: None,
        };
        ws.send(Message::text(hello.to_json())).await.unwrap();
        loop {
            match timeout(Duration::from_secs(5), ws.next()).await.unwrap() {
                Some(Ok(Message::Text(t))) => {
                    let msg = ServerMsg::from_json(t.as_str()).unwrap();
                    assert!(matches!(msg, ServerMsg::Welcome { .. }), "{msg:?}");
                    return ws;
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                other => panic!("expected welcome, got {other:?}"),
            }
        }
    }

    /// A control client: a background task reads (and auto-answers pings) and forwards
    /// every message with its arrival time.
    async fn client(&self, name: &str) -> Client {
        let session = Uuid::new_v4();
        let ws = self.open(Role::Control, session, name).await;
        let (sink, mut stream) = ws.split();
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(Ok(frame)) = stream.next().await {
                if let Message::Text(t) = frame {
                    let msg = ServerMsg::from_json(t.as_str()).unwrap();
                    if tx.send((Instant::now(), msg)).is_err() {
                        break;
                    }
                }
            }
        });
        Client { session, sink, rx }
    }
}

struct Client {
    session: Uuid,
    sink: SplitSink<Ws, Message>,
    rx: mpsc::UnboundedReceiver<(Instant, ServerMsg)>,
}

impl Client {
    async fn send(&mut self, msg: &ClientMsg) {
        self.sink.send(Message::text(msg.to_json())).await.unwrap();
    }

    /// The first message matching `f` within `within`, skipping others.
    async fn wait_for<T>(
        &mut self,
        within: Duration,
        mut f: impl FnMut(&ServerMsg) -> Option<T>,
    ) -> T {
        let deadline = Instant::now() + within;
        loop {
            let (_, msg) = timeout(deadline - Instant::now(), self.rx.recv())
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for a message"))
                .expect("socket open");
            if let Some(v) = f(&msg) {
                return v;
            }
        }
    }

    /// The next `Error`, skipping presence frames.
    async fn error(&mut self) -> (ErrorCode, String) {
        self.wait_for(Duration::from_secs(2), |m| match m {
            ServerMsg::Error { code, message } => Some((*code, message.clone())),
            _ => None,
        })
        .await
    }

    /// Waits for a `Peers` snapshot satisfying `f`.
    async fn peers_where(
        &mut self,
        within: Duration,
        f: impl Fn(&[Peer], u32) -> bool,
    ) -> (Vec<Peer>, u32) {
        self.wait_for(within, |m| match m {
            ServerMsg::Peers {
                peers,
                hidden_active,
            } if f(peers, *hidden_active) => Some((peers.clone(), *hidden_active)),
            _ => None,
        })
        .await
    }

    async fn update(&mut self, state: GroupState) -> (Uuid, Uuid, Vec<Peer>) {
        self.wait_for(Duration::from_secs(5), |m| match m {
            ServerMsg::GroupUpdate {
                group,
                host,
                members,
                state: s,
                ..
            } if *s == state => Some((*group, *host, members.clone())),
            _ => None,
        })
        .await
    }

    fn drain(&mut self) -> Vec<(Instant, ServerMsg)> {
        let mut out = Vec::new();
        while let Ok(m) = self.rx.try_recv() {
            out.push(m);
        }
        out
    }
}

async fn send_ws(ws: &mut Ws, msg: &ClientMsg) {
    ws.send(Message::text(msg.to_json())).await.unwrap();
}

/// Downloads `bytes` on a data socket; returns when `DownloadDone` arrives.
async fn download(ws: &mut Ws, id: u32, bytes: u64) {
    send_ws(ws, &ClientMsg::Download { id, bytes }).await;
    let mut got = 0u64;
    loop {
        match timeout(Duration::from_secs(10), ws.next()).await.unwrap() {
            Some(Ok(Message::Binary(b))) => got += b.len() as u64,
            Some(Ok(Message::Text(t))) => match ServerMsg::from_json(t.as_str()).unwrap() {
                ServerMsg::DownloadDone { id: i, .. } if i == id => {
                    assert_eq!(got, bytes);
                    return;
                }
                other => panic!("unexpected {other:?}"),
            },
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
            other => panic!("unexpected {other:?}"),
        }
    }
}

/// Uploads `bytes` on a data socket; returns when `UploadDone` arrives.
async fn upload(ws: &mut Ws, id: u32, bytes: usize) {
    send_ws(
        ws,
        &ClientMsg::Upload {
            id,
            bytes: bytes as u64,
        },
    )
    .await;
    let chunk = vec![0x5Au8; 64 * 1024];
    let mut left = bytes;
    while left > 0 {
        let n = left.min(chunk.len());
        ws.send(Message::binary(chunk[..n].to_vec())).await.unwrap();
        left -= n;
    }
    loop {
        match timeout(Duration::from_secs(10), ws.next()).await.unwrap() {
            Some(Ok(Message::Text(t))) => match ServerMsg::from_json(t.as_str()).unwrap() {
                ServerMsg::UploadDone {
                    id: i, bytes: b, ..
                } if i == id => {
                    assert_eq!(b, bytes as u64);
                    return;
                }
                other => panic!("unexpected {other:?}"),
            },
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
            other => panic!("unexpected {other:?}"),
        }
    }
}

/// Paced transfers: `chunk` bytes every 100 ms from `from` to `until`.
async fn paced(ws: &mut Ws, download_dir: bool, chunk: u64, from: Instant, until: Instant) {
    every(
        ws,
        download_dir,
        chunk,
        Duration::from_millis(100),
        from,
        until,
    )
    .await;
}

/// Transfers of `chunk` bytes every `interval` (back to back with a zero interval).
async fn every(
    ws: &mut Ws,
    download_dir: bool,
    chunk: u64,
    interval: Duration,
    from: Instant,
    until: Instant,
) {
    sleep_until(from).await;
    let mut next = from;
    let mut id = 1;
    while Instant::now() + interval / 2 < until {
        if download_dir {
            download(ws, id, chunk).await;
        } else {
            upload(ws, id, chunk as usize).await;
        }
        id += 1;
        next += interval;
        sleep_until(next).await;
    }
}

fn names(peers: &[Peer]) -> Vec<&str> {
    peers.iter().map(|p| p.device_name.as_str()).collect()
}

fn minimal_run(profile: &str, duration_ms: u64, peers_max: u32) -> RunResult {
    serde_json::from_value(serde_json::json!({
        "id": Uuid::nil(),
        "created_at": "2026-10-10T00:00:00Z",
        "transport": "websocket",
        "profile": profile,
        "streams": 1,
        "summary": {"down_loaded_latency_ms": 12.5},
        "concurrency": {"peers_max": peers_max, "overlap_ms": 0.0, "overlap_ratio": 0.0},
        "duration_ms": duration_ms,
    }))
    .unwrap()
}

async fn post_run(server: &Server, session: Option<Uuid>, run: &RunResult) -> Uuid {
    let url = match session {
        Some(s) => server.url(&format!("/api/runs?session={s}")),
        None => server.url("/api/runs"),
    };
    let res = reqwest::Client::new()
        .post(url)
        .header("content-type", "application/json")
        .body(serde_json::to_vec(run).unwrap())
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{}", res.status());
    let v: serde_json::Value = serde_json::from_slice(&res.bytes().await.unwrap()).unwrap();
    v["id"].as_str().unwrap().parse().unwrap()
}

async fn get_run(server: &Server, id: Uuid) -> RunResult {
    let res = reqwest::get(server.url(&format!("/api/runs/{id}")))
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    serde_json::from_slice(&res.bytes().await.unwrap()).unwrap()
}

// -------------------------------------------------------------------------------------

#[tokio::test]
async fn presence_snapshots_fan_out() {
    let server = Server::start(true, false).await;
    let mut a = server.client("Alpha").await;
    let mut b = server.client("Bravo").await;
    let mut c = server.client("Charlie").await;
    let t0 = Instant::now();
    for cl in [&mut a, &mut b, &mut c] {
        let me = cl.session;
        let (peers, hidden) = cl
            .peers_where(Duration::from_secs(1), |p, _| p.len() == 3)
            .await;
        assert_eq!(names(&peers), ["Alpha", "Bravo", "Charlie"]);
        assert_eq!(hidden, 0);
        for p in &peers {
            assert_eq!(p.is_you, p.id == me, "{p:?}");
            assert_eq!(p.state, PeerState::Idle);
            assert_eq!(p.device_kind, DeviceKind::Laptop);
        }
    }
    assert!(t0.elapsed() < Duration::from_secs(1));

    // Status and label show up for everyone.
    b.send(&ClientMsg::Status {
        phase: Some(Phase::Download),
        label: Some("Desk".into()),
    })
    .await;
    let (peers, _) = a
        .peers_where(Duration::from_secs(1), |p, _| {
            p.iter().any(|p| p.label.as_deref() == Some("Desk"))
        })
        .await;
    let bravo = peers.iter().find(|p| p.device_name == "Bravo").unwrap();
    assert!(matches!(
        bravo.state,
        PeerState::Testing {
            phase: Phase::Download,
            ..
        }
    ));

    // Hidden while testing: left out, but counted.
    b.send(&ClientMsg::SetHidden { hidden: true }).await;
    let (peers, hidden) = c
        .peers_where(Duration::from_secs(1), |p, _| p.len() == 2)
        .await;
    assert_eq!(names(&peers), ["Alpha", "Charlie"]);
    assert_eq!(hidden, 1);
    // The hidden device still sees itself.
    let (peers, hidden) = b.peers_where(Duration::from_secs(1), |_, h| h == 0).await;
    assert_eq!(peers.len(), 3);
    assert!(peers.iter().any(|p| p.is_you && p.device_name == "Bravo"));
    assert_eq!(hidden, 0);

    // Idle and hidden: not counted any more.
    b.send(&ClientMsg::Status {
        phase: None,
        label: None,
    })
    .await;
    c.peers_where(Duration::from_secs(1), |p, h| p.len() == 2 && h == 0)
        .await;

    // A device that leaves disappears.
    a.sink.close().await.unwrap();
    c.peers_where(Duration::from_secs(1), |p, _| names(p) == ["Charlie"])
        .await;
}

#[tokio::test]
async fn live_rates_come_from_the_server_and_are_throttled() {
    let server = Server::start(true, false).await;
    let mut watcher = server.client("Watcher").await;
    let mut tester = server.client("Tester").await;
    let session = tester.session;
    let mut data = server.open(Role::Data, session, "Tester").await;
    watcher
        .peers_where(Duration::from_secs(1), |p, _| p.len() == 2)
        .await;
    watcher.drain();

    // 2 s of paced downloads (~40 Mbps) plus a flood of status changes.
    let start = Instant::now();
    let end = start + Duration::from_secs(2);
    let flood = async {
        let mut i = 0;
        while Instant::now() < end {
            tester
                .send(&ClientMsg::Status {
                    phase: Some(Phase::Download),
                    label: Some(format!("label {i}")),
                })
                .await;
            i += 1;
            sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::join!(paced(&mut data, true, 500_000, start, end), flood);
    sleep(Duration::from_millis(300)).await;

    let frames: Vec<(Instant, ServerMsg)> = watcher
        .drain()
        .into_iter()
        .filter(|(at, m)| matches!(m, ServerMsg::Peers { .. }) && *at <= end)
        .collect();
    assert!(
        frames.len() <= 9,
        "{} Peers frames in 2 s (max 4 Hz)",
        frames.len()
    );
    assert!(frames.len() >= 4, "{} frames", frames.len());
    for w in frames.windows(2) {
        let gap = w[1].0 - w[0].0;
        assert!(gap >= Duration::from_millis(200), "frames {gap:?} apart");
    }
    // The server measured the tester's download.
    let max_down = frames
        .iter()
        .filter_map(|(_, m)| match m {
            ServerMsg::Peers { peers, .. } => peers.iter().find_map(|p| match p.state {
                PeerState::Testing { down_bps, .. } if p.id == session => Some(down_bps),
                _ => None,
            }),
            _ => None,
        })
        .fold(0.0, f64::max);
    assert!(
        (10e6..200e6).contains(&max_down),
        "server-side rate {max_down}"
    );
}

#[tokio::test]
async fn presence_disabled() {
    let server = Server::start(false, false).await;
    let mut a = server.client("Alpha").await;
    let _b = server.client("Bravo").await;
    sleep(Duration::from_millis(600)).await;
    assert!(
        a.drain()
            .iter()
            .all(|(_, m)| !matches!(m, ServerMsg::Peers { .. })),
        "no presence frames"
    );
    for msg in [
        ClientMsg::GroupCreate { direction_ms: None },
        ClientMsg::GroupJoin {
            group: Uuid::new_v4(),
        },
        ClientMsg::GroupLeave,
        ClientMsg::GroupStart { direction_ms: None },
    ] {
        a.send(&msg).await;
        let (code, message) = a.error().await;
        assert_eq!(code, ErrorCode::NotFound);
        assert_eq!(message, "presence disabled");
    }
}

#[tokio::test]
async fn host_leaving_cancels_and_errors() {
    let server = Server::start(true, false).await;
    let mut host = server.client("Host").await;
    let mut guest = server.client("Guest").await;
    let mut other = server.client("Other").await;

    // Unknown group, nothing to start.
    guest
        .send(&ClientMsg::GroupJoin {
            group: Uuid::new_v4(),
        })
        .await;
    assert_eq!(guest.error().await.0, ErrorCode::NotFound);
    guest
        .send(&ClientMsg::GroupStart { direction_ms: None })
        .await;
    assert_eq!(guest.error().await.0, ErrorCode::NotFound);

    host.send(&ClientMsg::GroupCreate { direction_ms: None })
        .await;
    let (group, h, members) = host.update(GroupState::Forming).await;
    assert_eq!(h, host.session);
    assert_eq!(names(&members), ["Host"]);
    assert!(members[0].is_you);
    // Alone: can't start.
    host.send(&ClientMsg::GroupStart { direction_ms: None })
        .await;
    assert_eq!(host.error().await.0, ErrorCode::Conflict);

    let invited = guest
        .wait_for(Duration::from_secs(1), |m| match m {
            ServerMsg::GroupInvite { group, host, .. } => Some((*group, host.clone())),
            _ => None,
        })
        .await;
    assert_eq!(invited, (group, "Host".to_owned()));
    guest.send(&ClientMsg::GroupJoin { group }).await;
    let (_, _, members) = guest.update(GroupState::Forming).await;
    // Members may arrive in either update; wait for the one with both.
    let members = if members.len() == 2 {
        members
    } else {
        guest.update(GroupState::Forming).await.2
    };
    assert_eq!(members.len(), 2);
    assert!(members.iter().any(|p| p.is_you && p.id == guest.session));

    // Only the host may start.
    guest
        .send(&ClientMsg::GroupStart { direction_ms: None })
        .await;
    let (code, _) = guest.error().await;
    assert_eq!(code, ErrorCode::Conflict);

    host.send(&ClientMsg::GroupLeave).await;
    let (g, _, _) = guest.update(GroupState::Cancelled).await;
    assert_eq!(g, group);
    host.update(GroupState::Cancelled).await;

    // Too late to join a cancelled group.
    other.send(&ClientMsg::GroupJoin { group }).await;
    assert_eq!(other.error().await.0, ErrorCode::Conflict);
}

#[tokio::test]
async fn host_picks_the_group_length() {
    let server = Server::start(true, false).await;
    let mut host = server.client("Host").await;
    let mut guest = server.client("Guest").await;

    // A custom length at creation: the invite and every update carry it.
    host.send(&ClientMsg::GroupCreate {
        direction_ms: Some(3000),
    })
    .await;
    let length = |m: &ServerMsg| match m {
        ServerMsg::GroupUpdate { direction_ms, .. } => *direction_ms,
        _ => None,
    };
    let first = host
        .wait_for(Duration::from_secs(2), |m| {
            matches!(m, ServerMsg::GroupUpdate { .. }).then(|| m.clone())
        })
        .await;
    assert_eq!(length(&first), Some(3000));
    let (group, invite_ms) = guest
        .wait_for(Duration::from_secs(1), |m| match m {
            ServerMsg::GroupInvite {
                group,
                direction_ms,
                ..
            } => Some((*group, *direction_ms)),
            _ => None,
        })
        .await;
    assert_eq!(invite_ms, Some(3000));
    guest.send(&ClientMsg::GroupJoin { group }).await;
    let joined = guest
        .wait_for(Duration::from_secs(2), |m| match m {
            ServerMsg::GroupUpdate {
                members,
                direction_ms,
                ..
            } if members.len() == 2 => Some(*direction_ms),
            _ => None,
        })
        .await;
    assert_eq!(joined, Some(3000));

    // The plan uses it (no override at start).
    host.send(&ClientMsg::GroupStart { direction_ms: None })
        .await;
    for cl in [&mut host, &mut guest] {
        let plan = cl
            .wait_for(Duration::from_secs(1), |m| match m {
                ServerMsg::GroupStart { group: g, plan, .. } if *g == group => Some(plan.clone()),
                _ => None,
            })
            .await;
        assert_eq!(plan, Profile::concurrent_for(3000));
    }
    host.send(&ClientMsg::GroupLeave).await;
    guest.send(&ClientMsg::GroupLeave).await;

    // Too long a length is clamped to 5 min; `GroupStart` can replace it.
    host.send(&ClientMsg::GroupCreate {
        direction_ms: Some(3_600_000),
    })
    .await;
    let (group, clamped) = host
        .wait_for(Duration::from_secs(2), |m| match m {
            ServerMsg::GroupUpdate {
                group,
                state: GroupState::Forming,
                members,
                direction_ms,
                ..
            } if members.len() == 1 => Some((*group, *direction_ms)),
            _ => None,
        })
        .await;
    assert_eq!(clamped, Some(300_000));
    let invite_ms = guest
        .wait_for(Duration::from_secs(1), |m| match m {
            ServerMsg::GroupInvite {
                group: g,
                direction_ms,
                ..
            } if *g == group => Some(*direction_ms),
            _ => None,
        })
        .await;
    assert_eq!(invite_ms, Some(300_000));
    guest.send(&ClientMsg::GroupJoin { group }).await;
    host.wait_for(Duration::from_secs(2), |m| match m {
        ServerMsg::GroupUpdate { members, .. } if members.len() == 2 => Some(()),
        _ => None,
    })
    .await;
    host.send(&ClientMsg::GroupStart {
        direction_ms: Some(2500),
    })
    .await;
    let plan = guest
        .wait_for(Duration::from_secs(1), |m| match m {
            ServerMsg::GroupStart { group: g, plan, .. } if *g == group => Some(plan.clone()),
            _ => None,
        })
        .await;
    assert_eq!(plan, Profile::concurrent_for(2500));
    let starting = guest
        .wait_for(Duration::from_secs(1), |m| match m {
            ServerMsg::GroupUpdate {
                state: GroupState::Starting,
                direction_ms,
                ..
            } => Some(*direction_ms),
            _ => None,
        })
        .await;
    assert_eq!(starting, Some(2500));
}

#[tokio::test]
async fn group_test_measures_concurrent_transfers() {
    let server = Server::start(true, true).await;
    let mut a = server.client("Alpha").await;
    let mut b = server.client("Bravo").await;
    let mut c = server.client("Charlie").await;

    a.send(&ClientMsg::GroupCreate { direction_ms: None }).await;
    let (group, _, _) = a.update(GroupState::Forming).await;
    for cl in [&mut b, &mut c] {
        cl.wait_for(Duration::from_secs(1), |m| {
            matches!(m, ServerMsg::GroupInvite { group: g, .. } if *g == group).then_some(())
        })
        .await;
        cl.send(&ClientMsg::GroupJoin { group }).await;
    }
    a.wait_for(Duration::from_secs(2), |m| match m {
        ServerMsg::GroupUpdate { members, .. } if members.len() == 3 => Some(()),
        _ => None,
    })
    .await;
    a.send(&ClientMsg::GroupStart { direction_ms: None }).await;
    let mut starts = Vec::new();
    for cl in [&mut a, &mut b, &mut c] {
        let (at, start) = cl
            .wait_for(Duration::from_secs(1), |m| match m {
                ServerMsg::GroupStart {
                    group: g,
                    starts_in_ms,
                    plan,
                } if *g == group => Some((Instant::now(), (*starts_in_ms, plan.clone()))),
                _ => None,
            })
            .await;
        assert_eq!(start, (3000, Profile::concurrent_for(GROUP_SECONDS * 1000)));
        starts.push(at);
    }
    a.update(GroupState::Running).await;

    // Charlie drops out during the test; Alpha and Bravo transfer at controlled rates:
    // download 0.5 / 1 MB per 100 ms (40 / 80 Mbps), upload 0.25 / 0.5 MB (20 / 40 Mbps).
    // Windows follow the plan: latency, download, pause, upload.
    let plan = Profile::concurrent_for(GROUP_SECONDS * 1000);
    let d: Vec<u64> = plan
        .steps
        .iter()
        .map(|s| u64::from(s.duration_ms().unwrap()))
        .collect();
    let (down_start, down_end) = (d[0], d[0] + d[1]);
    let (up_start, up_end) = (down_end + d[2], down_end + d[2] + d[3]);
    let t0 = starts[0] + Duration::from_millis(3000);
    let ms = |n: u64| t0 + Duration::from_millis(n);
    let mut da = server.open(Role::Data, a.session, "Alpha").await;
    let mut db = server.open(Role::Data, b.session, "Bravo").await;
    let drop_c = async {
        sleep_until(ms(down_start + 500)).await;
        c.sink.close().await.unwrap();
    };
    let run_a = async {
        paced(&mut da, true, 500_000, ms(down_start), ms(down_end)).await;
        paced(&mut da, false, 250_000, ms(up_start), ms(up_end)).await;
    };
    let run_b = async {
        paced(&mut db, true, 1_000_000, ms(down_start), ms(down_end)).await;
        paced(&mut db, false, 500_000, ms(up_start), ms(up_end)).await;
    };
    tokio::join!(run_a, run_b, drop_c);
    sleep_until(ms(up_end + 100)).await;

    // The members post their runs: loaded latency for the result, group_id for history.
    let run = minimal_run("concurrent", up_end, 0);
    let run_a = post_run(&server, Some(a.session), &run).await;
    let run_b = post_run(&server, Some(b.session), &run).await;

    // `GroupUpdate{Finished}` comes first, then the result.
    a.update(GroupState::Finished).await;
    let result: GroupResult = a
        .wait_for(Duration::from_secs(4), |m| match m {
            ServerMsg::GroupResult { group: g, result } if *g == group => Some(result.clone()),
            _ => None,
        })
        .await;
    b.wait_for(Duration::from_secs(1), |m| {
        matches!(m, ServerMsg::GroupResult { .. }).then_some(())
    })
    .await;

    assert_eq!(result.group_id, group);
    assert_eq!(result.window_ms, up_end);
    assert_eq!(result.members.len(), 3);
    let member = |name: &str| {
        result
            .members
            .iter()
            .find(|m| m.device_name == name)
            .unwrap()
    };
    let (ma, mb, mc) = (member("Alpha"), member("Bravo"), member("Charlie"));
    assert!(mc.incomplete && !ma.incomplete && !mb.incomplete);
    let near = |v: Option<f64>, want: f64| {
        let v = v.unwrap();
        assert!((v - want).abs() / want < 0.2, "{v} vs {want}");
    };
    near(ma.download_bps, 40e6);
    near(mb.download_bps, 80e6);
    near(ma.upload_bps, 20e6);
    near(mb.upload_bps, 40e6);
    // Peak: the best second, at least the average (steady pacing: about the same).
    for (peak, avg) in [
        (ma.download_peak_bps, ma.download_bps),
        (mb.download_peak_bps, mb.download_bps),
        (ma.upload_peak_bps, ma.upload_bps),
        (mb.upload_peak_bps, mb.upload_bps),
    ] {
        let (peak, avg) = (peak.unwrap(), avg.unwrap());
        assert!(peak >= avg * 0.98 && peak <= avg * 1.4, "{peak} vs {avg}");
    }
    let agg = result.aggregate_download_bps.unwrap();
    assert!((agg - ma.download_bps.unwrap() - mb.download_bps.unwrap()).abs() < 1.0);
    let shares = ma.download_share.unwrap() + mb.download_share.unwrap();
    assert!((shares - 1.0).abs() < 1e-9);
    assert!((ma.upload_share.unwrap() + mb.upload_share.unwrap() - 1.0).abs() < 1e-9);
    assert_eq!(mc.download_share, None);
    let fair = result.fairness_download.unwrap();
    assert!(fair > 0.0 && fair <= 1.0);
    // 40 vs 80 Mbps → (120)² / (2 · (40² + 80²)) = 0.9.
    assert!((fair - 0.9).abs() < 0.05, "{fair}");
    assert_eq!(ma.down_loaded_latency_ms, Some(12.5));
    assert!(!ma.series.is_empty());
    assert_eq!(result.server_link_bps, Some(1e10));
    assert!(!result.bottleneck_warning);

    // History: the group and its member runs.
    for id in [run_a, run_b] {
        let r = get_run(&server, id).await;
        assert_eq!(r.group_id, Some(group));
        // They overlapped each other.
        assert!(r.concurrency.peers_max >= 1, "{:?}", r.concurrency);
    }
    let res = reqwest::get(server.url(&format!("/api/groups/{group}")))
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let v: serde_json::Value = serde_json::from_slice(&res.bytes().await.unwrap()).unwrap();
    assert_eq!(v["runs"].as_array().unwrap().len(), 2, "{v}");
    assert_eq!(v["detail"]["members"].as_array().unwrap().len(), 3, "{v}");
}

#[tokio::test]
async fn posted_runs_get_server_side_overlap() {
    let server = Server::start(true, true).await;
    let (s1, s2) = (Uuid::new_v4(), Uuid::new_v4());
    let mut d1 = server.open(Role::Data, s1, "One").await;
    let mut d2 = server.open(Role::Data, s2, "Two").await;
    let start = Instant::now();
    let end = start + Duration::from_millis(800);
    // Session 2 overlaps the second half of session 1's run.
    let back_to_back = Duration::ZERO;
    tokio::join!(
        every(&mut d1, true, 1_000_000, back_to_back, start, end),
        every(
            &mut d2,
            true,
            1_000_000,
            back_to_back,
            start + Duration::from_millis(400),
            end
        )
    );
    // The client claims no overlap; the server knows better.
    let id = post_run(&server, Some(s1), &minimal_run("standard", 800, 0)).await;
    let r = get_run(&server, id).await;
    assert_eq!(r.concurrency.peers_max, 1, "{:?}", r.concurrency);
    assert!(
        r.concurrency.overlap_ratio > 0.2 && r.concurrency.overlap_ratio < 0.8,
        "{:?}",
        r.concurrency
    );
    assert_eq!(r.group_id, None);

    // Unknown session: left as sent.
    let id = post_run(
        &server,
        Some(Uuid::new_v4()),
        &minimal_run("standard", 800, 3),
    )
    .await;
    assert_eq!(get_run(&server, id).await.concurrency.peers_max, 3);
    // No session: left as sent.
    let id = post_run(&server, None, &minimal_run("standard", 800, 2)).await;
    assert_eq!(get_run(&server, id).await.concurrency.peers_max, 2);
    // Malformed session.
    let res = reqwest::Client::new()
        .post(server.url("/api/runs?session=nope"))
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&minimal_run("standard", 1, 0)).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
}
