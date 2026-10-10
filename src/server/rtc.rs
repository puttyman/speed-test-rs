//! WebRTC echo endpoint for the packet-loss test (SPEC §5.7), built on `str0m` (sans-IO).
//!
//! The browser opens an unreliable, unordered data channel (`ordered: false,
//! maxRetransmits: 0`) and sends small numbered messages; this endpoint echoes every
//! message straight back on the same channel. The browser counts what comes back.
//!
//! # Sockets and candidates
//!
//! All WebRTC traffic uses one UDP **port** (`--udp-port`, default 3478):
//!
//! - **`--public-ip` set** (Docker bridge networking, NAT): one wildcard socket of that
//!   address family (`0.0.0.0:port` or `[::]:port`). The only advertised candidate is
//!   `public-ip:port`, and every datagram is handed to str0m as if it arrived there. The
//!   kernel picks the reply source address, which is right for the usual single-homed
//!   container.
//! - **Unset** (binary, Docker host networking): one socket per local interface address,
//!   all on the same port, and one host candidate per address. A socket per address (rather
//!   than one wildcard socket) tells us exactly which address a datagram was sent to —
//!   str0m's ICE agent requires that — and makes replies leave from the address the
//!   browser talks to, which multi-homed hosts would otherwise get wrong. This needs no
//!   platform-specific `IP_PKTINFO` code. IPv6 link-local addresses are skipped (they need a
//!   scope id). Interfaces that appear later (DHCP, VPN) are picked up when the next offer
//!   arrives.
//!
//! The browser's own candidates are not needed: the endpoint is **ICE-lite**, so it never
//! sends connectivity checks itself; it answers the browser's STUN binding requests and
//! learns the browser's address from them (a peer-reflexive candidate). That is why
//! [`RtcSignaling::remote_candidate`] keeps its no-op default, and why `.local` (mDNS)
//! candidates, which browsers use to hide host addresses, are removed from the offer.
//!
//! # Driver
//!
//! One Tokio task owns every [`Rtc`]. Socket reader tasks forward datagrams to it over a
//! channel; it demultiplexes each one to the `Rtc` that [`Rtc::accepts`] it, drives
//! timeouts, writes `Transmit`s to the matching socket and echoes `ChannelData`.
//!
//! # Limits and lifecycle
//!
//! At most `max_sessions` peers exist at once (one per measurement session; a new offer
//! for a session replaces its old peer). A peer is dropped when it closes, when ICE
//! disconnects, when it is not connected [`CONNECT_TIMEOUT`] after the offer, or after
//! [`IDLE_TIMEOUT`] without a datagram.

use std::collections::HashMap;
use std::fmt::Write;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use str0m::change::SdpOffer;
use str0m::config::{CryptoProvider, DtlsCert};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc, RtcConfig};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use super::config::Config;
use super::rtc_hook::RtcSignaling;

/// A peer that has not connected this long after its offer is dropped.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// A peer that has received nothing for this long is dropped.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// How long `answer` waits for the driver.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(5);
/// The driver wakes at least this often to sweep expired peers.
const SWEEP_INTERVAL: Duration = Duration::from_secs(1);
/// Largest datagram read (str0m keeps datagrams under ~1200 bytes).
const RECV_BUF: usize = 2048;
/// Queued datagrams between the socket readers and the driver.
const DATAGRAM_QUEUE: usize = 4096;

/// The str0m echo endpoint. Install it as `state.measure.rtc`.
pub struct RtcEcho {
    commands: mpsc::Sender<Command>,
    description: String,
}

impl RtcEcho {
    /// Binds the UDP socket(s) for `config.udp_port` and starts the driver task. Must be
    /// called inside a Tokio runtime. Fails when no socket could be bound.
    pub fn start(config: &Config) -> Result<Self, String> {
        let plan = match config.public_ip {
            Some(ip) => BindPlan::Public {
                public: SocketAddr::new(ip, config.udp_port),
            },
            None => BindPlan::Interfaces {
                port: config.udp_port,
            },
        };
        Self::start_with(plan, config.max_sessions)
    }

    /// [`start`](Self::start) with an explicit plan (tests bind `127.0.0.1:0`).
    pub fn start_with(plan: BindPlan, max_peers: usize) -> Result<Self, String> {
        let provider = Arc::new(str0m::crypto::from_feature_flags());
        let cert = provider
            .dtls_provider
            .generate_certificate()
            .ok_or("cannot generate a DTLS certificate")?;
        let (dgram_tx, dgram_rx) = mpsc::channel(DATAGRAM_QUEUE);
        let mut driver = Driver {
            plan,
            sockets: Vec::new(),
            peers: HashMap::new(),
            provider,
            cert,
            max_peers: max_peers.max(1),
            dgram_tx,
        };
        driver.refresh_sockets();
        if driver.sockets.is_empty() {
            return Err(format!("cannot bind UDP port {}", driver.plan.port()));
        }
        let description = driver.describe();
        let (commands, cmd_rx) = mpsc::channel(64);
        tokio::spawn(driver.run(cmd_rx, dgram_rx));
        Ok(Self {
            commands,
            description,
        })
    }

    /// For the startup banner, e.g. `udp/3478 on 192.168.1.10, 127.0.0.1`.
    pub fn describe(&self) -> &str {
        &self.description
    }
}

#[async_trait]
impl RtcSignaling for RtcEcho {
    async fn answer(&self, session: Uuid, offer_sdp: String) -> Result<String, String> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::Offer {
                session,
                sdp: offer_sdp,
                reply,
            })
            .await
            .map_err(|_| "packet loss test unavailable".to_owned())?;
        match tokio::time::timeout(ANSWER_TIMEOUT, rx).await {
            Ok(Ok(result)) => result,
            _ => Err("packet loss test unavailable".to_owned()),
        }
    }
}

/// Which UDP sockets to bind.
#[derive(Debug, Clone)]
pub enum BindPlan {
    /// One wildcard socket; advertise `public` (NAT / Docker bridge).
    Public { public: SocketAddr },
    /// One socket per local interface address on `port`.
    Interfaces { port: u16 },
    /// Exactly these addresses (tests). Port 0 picks a free port per socket.
    Fixed(Vec<SocketAddr>),
}

impl BindPlan {
    fn port(&self) -> u16 {
        match self {
            BindPlan::Public { public } => public.port(),
            BindPlan::Interfaces { port } => *port,
            BindPlan::Fixed(addrs) => addrs.first().map_or(0, SocketAddr::port),
        }
    }
}

enum Command {
    Offer {
        session: Uuid,
        sdp: String,
        reply: oneshot::Sender<Result<String, String>>,
    },
}

/// A datagram read by a socket reader task.
struct Datagram {
    socket: usize,
    source: SocketAddr,
    at: Instant,
    data: Vec<u8>,
}

/// A bound UDP socket and the candidate address it stands for.
struct BoundSocket {
    socket: Arc<UdpSocket>,
    /// The address the socket is bound to.
    local: SocketAddr,
    /// The address advertised in the host candidate (`local`, or the public IP).
    advertised: SocketAddr,
}

struct Peer {
    rtc: Rtc,
    created: Instant,
    last_rx: Instant,
    next_timeout: Instant,
    connected: bool,
    closed: bool,
}

struct Driver {
    plan: BindPlan,
    sockets: Vec<BoundSocket>,
    peers: HashMap<Uuid, Peer>,
    provider: Arc<CryptoProvider>,
    cert: DtlsCert,
    max_peers: usize,
    dgram_tx: mpsc::Sender<Datagram>,
}

impl Driver {
    async fn run(
        mut self,
        mut commands: mpsc::Receiver<Command>,
        mut datagrams: mpsc::Receiver<Datagram>,
    ) {
        loop {
            let now = Instant::now();
            let wake = self
                .peers
                .values()
                .map(|p| p.next_timeout)
                .min()
                .unwrap_or(now + SWEEP_INTERVAL)
                .min(now + SWEEP_INTERVAL);
            tokio::select! {
                cmd = commands.recv() => match cmd {
                    Some(Command::Offer { session, sdp, reply }) => {
                        let _ = reply.send(self.offer(session, &sdp));
                    }
                    // The endpoint was dropped.
                    None => break,
                },
                Some(d) = datagrams.recv() => self.datagram(d),
                () = tokio::time::sleep_until(tokio::time::Instant::from_std(wake)) => {}
            }
            self.timeouts(Instant::now());
            self.sweep(Instant::now());
        }
        tracing::debug!("WebRTC driver stopped");
    }

    /// Binds the sockets the plan calls for that are not bound yet.
    fn refresh_sockets(&mut self) {
        let wanted: Vec<(SocketAddr, SocketAddr)> = match &self.plan {
            BindPlan::Public { public } => {
                let any = match public.ip() {
                    IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                    IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
                };
                vec![(SocketAddr::new(any, public.port()), *public)]
            }
            BindPlan::Interfaces { port } => interface_ips()
                .into_iter()
                .map(|ip| {
                    let addr = SocketAddr::new(ip, *port);
                    (addr, addr)
                })
                .collect(),
            BindPlan::Fixed(addrs) => addrs.iter().map(|a| (*a, *a)).collect(),
        };
        for (local, advertised) in wanted {
            let already = self
                .sockets
                .iter()
                .any(|s| s.advertised == advertised || (local.port() != 0 && s.local == local));
            if already {
                continue;
            }
            match bind_udp(local) {
                Ok(socket) => {
                    let local = socket.local_addr().unwrap_or(local);
                    let advertised = if advertised.port() == 0 {
                        local
                    } else {
                        advertised
                    };
                    let socket = Arc::new(socket);
                    spawn_reader(self.sockets.len(), socket.clone(), self.dgram_tx.clone());
                    tracing::debug!(%local, %advertised, "WebRTC UDP socket bound");
                    self.sockets.push(BoundSocket {
                        socket,
                        local,
                        advertised,
                    });
                }
                Err(e) => tracing::debug!("cannot bind UDP {local}: {e}"),
            }
        }
    }

    fn describe(&self) -> String {
        let mut out = format!("udp/{}", self.plan.port());
        let mut first = true;
        for s in &self.sockets {
            let _ = write!(
                out,
                "{}{}",
                if first { " on " } else { ", " },
                s.advertised.ip()
            );
            first = false;
        }
        out
    }

    fn offer(&mut self, session: Uuid, sdp: &str) -> Result<String, String> {
        let now = Instant::now();
        self.peers.remove(&session);
        self.sweep(now);
        if self.peers.len() >= self.max_peers {
            return Err("too many packet loss tests are running; try again shortly".into());
        }
        self.refresh_sockets();

        let offer = SdpOffer::from_sdp_string(&strip_mdns_candidates(sdp))
            .map_err(|e| format!("invalid SDP offer: {e}"))?;
        let mut rtc = RtcConfig::new()
            .set_ice_lite(true)
            .set_crypto_provider(self.provider.clone())
            .set_dtls_cert(self.cert.clone())
            .build(now);
        for s in &self.sockets {
            match Candidate::host(s.advertised, "udp") {
                Ok(c) => {
                    rtc.add_local_candidate(c);
                }
                Err(e) => tracing::debug!("skipping candidate {}: {e}", s.advertised),
            }
        }
        let answer = rtc
            .sdp_api()
            .accept_offer(offer)
            .map_err(|e| format!("cannot accept the SDP offer: {e}"))?;
        let mut peer = Peer {
            rtc,
            created: now,
            last_rx: now,
            next_timeout: now,
            connected: false,
            closed: false,
        };
        drain(&mut peer, &self.sockets, session);
        self.peers.insert(session, peer);
        tracing::debug!(%session, "WebRTC peer created");
        Ok(answer.to_sdp_string())
    }

    fn datagram(&mut self, d: Datagram) {
        let Some(socket) = self.sockets.get(d.socket) else {
            return;
        };
        let Ok(receive) = Receive::new(Protocol::Udp, d.source, socket.advertised, &d.data) else {
            return;
        };
        let input = Input::Receive(d.at, receive);
        let Some((session, peer)) = self.peers.iter_mut().find(|(_, p)| p.rtc.accepts(&input))
        else {
            tracing::trace!(source = %d.source, "datagram for no WebRTC peer");
            return;
        };
        peer.last_rx = d.at;
        if let Err(e) = peer.rtc.handle_input(input) {
            tracing::debug!(%session, "WebRTC input error: {e}");
            peer.closed = true;
            return;
        }
        let session = *session;
        drain(peer, &self.sockets, session);
    }

    fn timeouts(&mut self, now: Instant) {
        for (session, peer) in &mut self.peers {
            if peer.next_timeout > now || peer.closed {
                continue;
            }
            if let Err(e) = peer.rtc.handle_input(Input::Timeout(now)) {
                tracing::debug!(%session, "WebRTC timeout error: {e}");
                peer.closed = true;
                continue;
            }
            drain(peer, &self.sockets, *session);
        }
    }

    fn sweep(&mut self, now: Instant) {
        self.peers.retain(|session, p| {
            let keep = !p.closed
                && p.rtc.is_alive()
                && now.duration_since(p.last_rx) < IDLE_TIMEOUT
                && (p.connected || now.duration_since(p.created) < CONNECT_TIMEOUT);
            if !keep {
                tracing::debug!(%session, "WebRTC peer removed");
            }
            keep
        });
    }
}

/// Polls `peer` until it wants a timeout: sends its datagrams and echoes channel data.
fn drain(peer: &mut Peer, sockets: &[BoundSocket], session: Uuid) {
    loop {
        match peer.rtc.poll_output() {
            Ok(Output::Timeout(t)) => {
                peer.next_timeout = t;
                return;
            }
            Ok(Output::Transmit(t)) => send(sockets, t.source, t.destination, &t.contents),
            Ok(Output::Event(event)) => match event {
                Event::ChannelData(d) => {
                    if let Some(mut channel) = peer.rtc.channel(d.id) {
                        // Unreliable channel: a full buffer just drops the echo.
                        let _ = channel.write(d.binary, &d.data);
                    }
                }
                Event::ChannelOpen(_, label) => {
                    tracing::debug!(%session, %label, "WebRTC data channel open");
                    peer.connected = true;
                }
                Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
                    peer.closed = true;
                }
                Event::Connected => peer.connected = true,
                _ => {}
            },
            Err(e) => {
                tracing::debug!(%session, "WebRTC error: {e}");
                peer.closed = true;
                return;
            }
        }
        if !peer.rtc.is_alive() {
            return;
        }
    }
}

fn send(sockets: &[BoundSocket], source: SocketAddr, destination: SocketAddr, data: &[u8]) {
    let socket = sockets.iter().find(|s| s.advertised == source).or_else(|| {
        sockets
            .iter()
            .find(|s| s.local.is_ipv4() == destination.is_ipv4())
    });
    if let Some(s) = socket {
        // UDP: if the send buffer is full the datagram is simply lost.
        if let Err(e) = s.socket.try_send_to(data, destination) {
            tracing::trace!("UDP send to {destination} failed: {e}");
        }
    }
}

fn spawn_reader(index: usize, socket: Arc<UdpSocket>, tx: mpsc::Sender<Datagram>) {
    tokio::spawn(async move {
        let mut buf = vec![0u8; RECV_BUF];
        loop {
            match socket.recv_from(&mut buf).await {
                Ok((n, source)) => {
                    let d = Datagram {
                        socket: index,
                        source,
                        at: Instant::now(),
                        data: buf[..n].to_vec(),
                    };
                    if tx.send(d).await.is_err() {
                        return;
                    }
                }
                // E.g. ICMP port unreachable surfacing as ECONNREFUSED: keep reading.
                Err(e) => {
                    tracing::trace!("UDP receive error: {e}");
                    if tx.is_closed() {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        }
    });
}

fn bind_udp(addr: SocketAddr) -> std::io::Result<UdpSocket> {
    use socket2::{Domain, Socket, Type};
    let socket = Socket::new(Domain::for_address(addr), Type::DGRAM, None)?;
    if addr.is_ipv6() {
        socket.set_only_v6(true)?;
    }
    // Room for bursts of small messages from several browsers.
    let _ = socket.set_recv_buffer_size(1 << 20);
    let _ = socket.set_send_buffer_size(1 << 20);
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    UdpSocket::from_std(socket.into())
}

/// Local addresses that can carry a host candidate: every interface address except IPv4
/// link-local, IPv6 link-local (would need a scope id) and multicast. Loopback addresses
/// are kept (and sorted last) so a browser on the server itself works.
fn interface_ips() -> Vec<IpAddr> {
    let mut ips: Vec<IpAddr> = match if_addrs::get_if_addrs() {
        Ok(ifaces) => ifaces.into_iter().map(|i| i.ip()).collect(),
        Err(e) => {
            tracing::warn!("cannot list network interfaces: {e}");
            vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]
        }
    };
    ips.retain(|ip| match ip {
        IpAddr::V4(v) => !v.is_link_local() && !v.is_multicast() && !v.is_unspecified(),
        IpAddr::V6(v) => {
            (v.segments()[0] & 0xffc0) != 0xfe80 && !v.is_multicast() && !v.is_unspecified()
        }
    });
    ips.sort_by_key(|ip| (ip.is_loopback(), ip.is_ipv6()));
    let mut unique = Vec::with_capacity(ips.len());
    for ip in ips {
        if !unique.contains(&ip) {
            unique.push(ip);
        }
    }
    unique
}

/// Removes `a=candidate` lines whose address is an mDNS name (`<uuid>.local`): str0m can't
/// resolve them, and an ICE-lite server learns the browser's address from its STUN
/// requests anyway.
pub fn strip_mdns_candidates(sdp: &str) -> String {
    let mut out = String::with_capacity(sdp.len());
    for line in sdp.split_inclusive('\n') {
        let is_mdns = line
            .strip_prefix("a=candidate:")
            .and_then(|rest| rest.split_whitespace().nth(4))
            .is_some_and(|addr| addr.ends_with(".local"));
        if !is_mdns {
            out.push_str(line);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use str0m::change::SdpAnswer;
    use str0m::channel::{ChannelConfig, Reliability};

    /// An offer as Chromium sends it (data channel only, one mDNS host candidate).
    const CHROME_OFFER: &str = "v=0\r\n\
o=- 4215775240449191010 2 IN IP4 127.0.0.1\r\n\
s=-\r\n\
t=0 0\r\n\
a=group:BUNDLE 0\r\n\
a=extmap-allow-mixed\r\n\
a=msid-semantic: WMS\r\n\
m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n\
c=IN IP4 0.0.0.0\r\n\
a=candidate:2909302429 1 udp 2113937151 0c9fe3a5-0a1d-4f55-9c35-6f1a0d7b4f0e.local 52739 typ host generation 0 network-cost 999\r\n\
a=candidate:842163049 1 udp 1677729535 192.0.2.7 52739 typ srflx raddr 0.0.0.0 rport 0 generation 0 network-cost 999\r\n\
a=ice-ufrag:R9GW\r\n\
a=ice-pwd:MHWShO6jlcgbF5x3w0wrhR7N\r\n\
a=ice-options:trickle\r\n\
a=fingerprint:sha-256 6B:8B:5D:EA:59:04:20:23:29:C8:87:1C:CC:87:32:BE:DD:8C:66:A5:8E:50:55:EA:8C:D3:B6:5C:09:5E:D6:BC\r\n\
a=setup:actpass\r\n\
a=mid:0\r\n\
a=sctp-port:5000\r\n\
a=max-message-size:262144\r\n";

    #[test]
    fn strips_only_mdns_candidates() {
        let out = strip_mdns_candidates(CHROME_OFFER);
        assert!(!out.contains(".local"));
        assert!(out.contains("192.0.2.7 52739 typ srflx"));
        assert_eq!(
            out.lines().count(),
            CHROME_OFFER.lines().count() - 1,
            "exactly one line removed"
        );
    }

    #[tokio::test]
    async fn answers_a_chrome_offer_with_host_candidates() {
        let echo =
            RtcEcho::start_with(BindPlan::Fixed(vec!["127.0.0.1:0".parse().unwrap()]), 4).unwrap();
        assert!(echo.describe().contains("127.0.0.1"));
        let answer = echo
            .answer(Uuid::new_v4(), CHROME_OFFER.to_owned())
            .await
            .unwrap();
        assert!(answer.contains("a=ice-lite"), "{answer}");
        assert!(answer.contains("webrtc-datachannel"), "{answer}");
        assert!(answer.contains("a=fingerprint:sha-256"), "{answer}");
        assert!(
            answer
                .lines()
                .any(|l| l.starts_with("a=candidate:") && l.contains("127.0.0.1")),
            "{answer}"
        );
        // A second session also gets an answer; a garbage offer is an error.
        assert!(
            echo.answer(Uuid::new_v4(), CHROME_OFFER.to_owned())
                .await
                .is_ok()
        );
        assert!(echo.answer(Uuid::new_v4(), "v=0".into()).await.is_err());
    }

    #[tokio::test]
    async fn fails_when_the_port_is_taken() {
        let taken = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = taken.local_addr().unwrap();
        assert!(RtcEcho::start_with(BindPlan::Fixed(vec![addr]), 4).is_err());
    }

    #[tokio::test]
    async fn caps_concurrent_peers() {
        let echo =
            RtcEcho::start_with(BindPlan::Fixed(vec!["127.0.0.1:0".parse().unwrap()]), 1).unwrap();
        let a = Uuid::new_v4();
        assert!(echo.answer(a, CHROME_OFFER.into()).await.is_ok());
        let err = echo
            .answer(Uuid::new_v4(), CHROME_OFFER.into())
            .await
            .unwrap_err();
        assert!(err.contains("too many"), "{err}");
        // The same session may renegotiate: its old peer is replaced.
        assert!(echo.answer(a, CHROME_OFFER.into()).await.is_ok());
    }

    /// A native str0m client (full ICE, controlling) on its own UDP socket.
    struct Client {
        rtc: Rtc,
        socket: UdpSocket,
        local: SocketAddr,
        channel: Option<str0m::channel::ChannelId>,
        echoed: Vec<u32>,
    }

    impl Client {
        async fn connect(echo: &RtcEcho) -> (Client, str0m::change::SdpPendingOffer, String) {
            let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let local = socket.local_addr().unwrap();
            let provider = Arc::new(str0m::crypto::from_feature_flags());
            let mut rtc = RtcConfig::new()
                .set_crypto_provider(provider)
                .build(Instant::now());
            rtc.add_local_candidate(Candidate::host(local, "udp").unwrap());
            let mut api = rtc.sdp_api();
            api.add_channel_with_config(ChannelConfig {
                label: "loss".into(),
                ordered: false,
                reliability: Reliability::MaxRetransmits { retransmits: 0 },
                ..Default::default()
            });
            let (offer, pending) = api.apply().unwrap();
            let answer = echo
                .answer(Uuid::new_v4(), offer.to_sdp_string())
                .await
                .unwrap();
            let client = Client {
                rtc,
                socket,
                local,
                channel: None,
                echoed: Vec::new(),
            };
            (client, pending, answer)
        }

        /// Runs the client until `deadline`, sending `to_send` once the channel opens.
        async fn pump(&mut self, deadline: Instant, to_send: &mut Vec<u32>) {
            let mut buf = vec![0u8; RECV_BUF];
            while Instant::now() < deadline {
                let timeout = loop {
                    match self.rtc.poll_output().unwrap() {
                        Output::Timeout(t) => break t,
                        Output::Transmit(t) => {
                            self.socket
                                .send_to(&t.contents, t.destination)
                                .await
                                .unwrap();
                        }
                        Output::Event(Event::ChannelOpen(id, _)) => self.channel = Some(id),
                        Output::Event(Event::ChannelData(d)) => {
                            self.echoed
                                .push(u32::from_le_bytes(d.data[..4].try_into().unwrap()));
                        }
                        Output::Event(_) => {}
                    }
                };
                if let Some(id) = self.channel {
                    while let Some(seq) = to_send.pop() {
                        let mut ch = self.rtc.channel(id).unwrap();
                        assert!(ch.write(true, &seq.to_le_bytes()).unwrap());
                    }
                }
                let wait = timeout
                    .min(deadline)
                    .saturating_duration_since(Instant::now())
                    .max(Duration::from_millis(1));
                match tokio::time::timeout(wait, self.socket.recv_from(&mut buf)).await {
                    Ok(Ok((n, source))) => {
                        let r = Receive::new(Protocol::Udp, source, self.local, &buf[..n]).unwrap();
                        self.rtc
                            .handle_input(Input::Receive(Instant::now(), r))
                            .unwrap();
                    }
                    _ => self
                        .rtc
                        .handle_input(Input::Timeout(Instant::now()))
                        .unwrap(),
                }
                if self.echoed.len() >= 50 && to_send.is_empty() {
                    break;
                }
            }
        }
    }

    /// Two native clients talk to one endpoint through the same UDP socket; each gets
    /// exactly its own messages back.
    #[tokio::test]
    async fn echoes_data_channel_messages_for_two_peers() {
        let echo =
            RtcEcho::start_with(BindPlan::Fixed(vec!["127.0.0.1:0".parse().unwrap()]), 4).unwrap();
        let mut clients = Vec::new();
        for _ in 0..2 {
            let (mut client, pending, answer) = Client::connect(&echo).await;
            let answer = SdpAnswer::from_sdp_string(&answer).unwrap();
            client.rtc.sdp_api().accept_answer(pending, answer).unwrap();
            clients.push(client);
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        let (a, b) = clients.split_at_mut(1);
        let mut send_a: Vec<u32> = (0..50).collect();
        let mut send_b: Vec<u32> = (1000..1050).collect();
        tokio::join!(
            a[0].pump(deadline, &mut send_a),
            b[0].pump(deadline, &mut send_b)
        );
        for (client, range) in clients.iter_mut().zip([0..50u32, 1000..1050]) {
            client.echoed.sort_unstable();
            assert_eq!(client.echoed, range.collect::<Vec<_>>());
        }
    }
}
