//! WebSocket transport (SPEC §5.4, §5.5): ping/pong latency on the probe
//! socket, downloads timed on the client, uploads timed by the server, and
//! parallel streams over several data sockets.

use std::cell::Cell;

use futures::future::{join_all, try_join_all};
use uuid::Uuid;

use super::js::{EngineError, Result, now, sleep, timeout};
use super::socket::Socket;
use super::util::{CHUNK_BYTES, MAX_BUFFERED_BYTES, split_bytes, ws_url};
use super::{Sample, UploadBuffer};
use crate::model::protocol::{ClientMsg, Limits, Role, ServerMsg};

const CONNECT_TIMEOUT_MS: u32 = 5000;
const PING_TIMEOUT_MS: u32 = 10_000;

pub struct WsTransport {
    probe: Socket,
    data: Vec<Socket>,
    upload: UploadBuffer,
    next_id: Cell<u32>,
    next_seq: Cell<u32>,
}

/// Opens a socket on `path` and greets the server.
pub async fn open(
    origin: &str,
    path: &str,
    session: Uuid,
    role: Role,
    auth: Option<String>,
    device: Option<crate::model::protocol::DeviceInfo>,
) -> Result<Socket> {
    let hello = ClientMsg::Hello {
        session,
        role,
        device,
        auth,
    };
    Socket::connect(&ws_url(origin, path), &hello, CONNECT_TIMEOUT_MS).await
}

impl WsTransport {
    /// Opens the probe socket and `streams` data sockets in parallel.
    pub async fn connect(
        origin: &str,
        session: Uuid,
        auth: Option<String>,
        streams: u8,
        upload: UploadBuffer,
    ) -> Result<Self> {
        let probe = open(
            origin,
            "/ws/probe",
            session,
            Role::Probe,
            auth.clone(),
            None,
        );
        let data = try_join_all(
            (0..streams.max(1))
                .map(|_| open(origin, "/ws/data", session, Role::Data, auth.clone(), None)),
        );
        let (probe, data) = futures::future::try_join(probe, data).await?;
        Ok(Self {
            probe,
            data,
            upload,
            next_id: Cell::new(1),
            next_seq: Cell::new(1),
        })
    }

    pub fn limits(&self) -> Limits {
        self.probe.welcome.limits
    }

    /// Number of open data sockets.
    pub fn max_streams(&self) -> u8 {
        self.data.len() as u8
    }

    fn id(&self) -> u32 {
        let id = self.next_id.get();
        self.next_id.set(id.wrapping_add(1));
        id
    }

    /// One ping/pong round trip on the probe socket, in ms.
    pub async fn ping(&self) -> Result<f64> {
        let seq = self.next_seq.get();
        self.next_seq.set(seq.wrapping_add(1));
        let sent = now();
        self.probe.send(&ClientMsg::Ping { seq })?;
        timeout(PING_TIMEOUT_MS, "a pong", async {
            loop {
                let m = self.probe.recv().await?;
                match m.msg {
                    ServerMsg::Pong { seq: s } if s == seq => return Ok(m.at - sent),
                    ServerMsg::Error { code, message } => {
                        return Err(EngineError::Server { code, message });
                    }
                    // Late pongs of abandoned probes, presence, …
                    _ => {}
                }
            }
        })
        .await
    }

    /// Cumulative progress counter for the live meter.
    pub fn progress(&self, download: bool) -> u64 {
        if download {
            self.data.iter().map(Socket::rx_bytes).sum()
        } else {
            self.data
                .iter()
                .map(|s| s.tx_bytes().saturating_sub(u64::from(s.buffered_amount())))
                .sum()
        }
    }

    /// Downloads `bytes` split over `streams` sockets. Duration: first byte
    /// on any socket → last byte on all sockets, timed on the client.
    pub async fn download(&self, bytes: u64, streams: u8) -> Result<Sample> {
        let parts = split_bytes(bytes, streams.min(self.max_streams()));
        let sockets = &self.data[..parts.len()];
        let mut ids = Vec::with_capacity(parts.len());
        let mut start_rx = Vec::with_capacity(parts.len());
        for (s, &n) in sockets.iter().zip(&parts) {
            s.reset_rx_timing();
            start_rx.push(s.rx_bytes());
            let id = self.id();
            ids.push(id);
            s.send(&ClientMsg::Download { id, bytes: n })?;
        }
        let server = join_all(
            sockets
                .iter()
                .zip(&ids)
                .map(|(s, &id)| wait_download_done(s, id)),
        )
        .await;
        let mut server_ms: f64 = 0.0;
        let mut first = f64::INFINITY;
        let mut last = f64::NEG_INFINITY;
        for (i, r) in server.into_iter().enumerate() {
            server_ms = server_ms.max(r?);
            let s = &sockets[i];
            let got = s.rx_bytes() - start_rx[i];
            if got != parts[i] {
                return Err(EngineError::Protocol(format!(
                    "download {}: received {got} of {} bytes",
                    ids[i], parts[i]
                )));
            }
            if let Some((a, b)) = s.rx_timing() {
                first = first.min(a);
                last = last.max(b);
            }
        }
        let client_ms = last - first;
        // A tiny transfer can land within one timer tick; fall back to the
        // server's send time rather than reporting an infinite rate.
        let duration_ms = if client_ms.is_finite() && client_ms > 0.0 {
            client_ms
        } else {
            server_ms
        };
        Ok(Sample {
            duration_ms,
            server_ms: Some(server_ms),
            client_ms: None,
            round_trips: 0,
        })
    }

    /// Uploads `bytes` split over `streams` sockets, paced by
    /// `bufferedAmount`. Duration: the server-measured time (the longest
    /// stream when parallel).
    pub async fn upload(&self, bytes: u64, streams: u8) -> Result<Sample> {
        let parts = split_bytes(bytes, streams.min(self.max_streams()));
        let sockets = &self.data[..parts.len()];
        let mut ids = Vec::with_capacity(parts.len());
        for (s, &n) in sockets.iter().zip(&parts) {
            let id = self.id();
            ids.push(id);
            s.send(&ClientMsg::Upload { id, bytes: n })?;
        }
        let t_start = now();
        let mut remaining = parts.clone();
        let mut offsets = vec![0usize; parts.len()];
        let buf_len = self.upload.len();
        loop {
            let mut pending = false;
            for (i, s) in sockets.iter().enumerate() {
                while remaining[i] > 0 && s.buffered_amount() < MAX_BUFFERED_BYTES {
                    let n = remaining[i].min(CHUNK_BYTES as u64) as usize;
                    let off = offsets[i];
                    if n == CHUNK_BYTES {
                        s.send_view(self.upload.chunk(off / CHUNK_BYTES))?;
                    } else {
                        s.send_view(&self.upload.view(off, n))?;
                    }
                    remaining[i] -= n as u64;
                    offsets[i] = (off + CHUNK_BYTES) % buf_len;
                }
                pending |= remaining[i] > 0;
            }
            if !pending {
                break;
            }
            // Let the socket drain (and timers run).
            sleep(1).await;
        }
        let results = join_all(
            sockets
                .iter()
                .zip(&ids)
                .map(|(s, &id)| wait_upload_done(s, id)),
        )
        .await;
        let client_ms = now() - t_start;
        let mut server_ms: f64 = 0.0;
        for (i, r) in results.into_iter().enumerate() {
            let (got, ms) = r?;
            if got != parts[i] {
                return Err(EngineError::Protocol(format!(
                    "upload {}: server received {got} of {} bytes",
                    ids[i], parts[i]
                )));
            }
            server_ms = server_ms.max(ms);
        }
        Ok(Sample {
            duration_ms: server_ms,
            server_ms: Some(server_ms),
            // `upload` request out, `upload_done` back: one round trip.
            client_ms: Some(client_ms),
            round_trips: 1,
        })
    }
}

async fn wait_download_done(s: &Socket, id: u32) -> Result<f64> {
    loop {
        match s.recv().await?.msg {
            ServerMsg::DownloadDone { id: i, server_ms } if i == id => return Ok(server_ms),
            ServerMsg::Error { code, message } => {
                return Err(EngineError::Server { code, message });
            }
            _ => {}
        }
    }
}

async fn wait_upload_done(s: &Socket, id: u32) -> Result<(u64, f64)> {
    loop {
        match s.recv().await?.msg {
            ServerMsg::UploadDone {
                id: i,
                bytes,
                server_ms,
            } if i == id => return Ok((bytes, server_ms)),
            ServerMsg::Error { code, message } => {
                return Err(EngineError::Server { code, message });
            }
            _ => {}
        }
    }
}
