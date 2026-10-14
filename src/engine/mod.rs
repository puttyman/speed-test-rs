//! The browser measurement engine (SPEC §5), compiled for `hydrate`.
//!
//! ```ignore
//! let handle = engine::start(EngineOptions::new(Transport::Websocket, Profile::standard(), device), |ev| {
//!     // push `ev` into Leptos signals
//! });
//! handle.abort();
//! ```
//!
//! Events arrive in order: `Connected`, then per step `PhaseStarted`, any
//! number of `Latency` / `Bandwidth` / `Live` / `LoadedLatency`, and
//! `PhaseFinished`; finally exactly one of `Finished`, `Error` or `Aborted`.
//! The engine never waits on the UI: the callback should only store values.
//!
//! Module layout: [`socket`] (WebSocket wrapper), [`ws_transport`] and
//! [`http_transport`] (latency/download/upload), [`loaded_latency`],
//! [`runner`] (profile execution), [`rtc`] (WebRTC packet loss probe),
//! [`presence`] (the page-wide presence and group client, SPEC §5.11).
//! [`util`] holds the platform-free helpers.
//!
//! **Sessions.** A run opens its own control socket (status updates, WebRTC
//! signalling), a probe socket and its data sockets, all under
//! `EngineOptions::session`. A page that keeps a presence connection should
//! run with the presence session ID ([`presence::PresenceHandle::engine_options`]
//! does this): the server then counts the run's bytes for that device (live
//! speed in the Devices panel, group results, overlap flagging), and the
//! session simply has two control sockets while a test runs. The run's
//! control socket is kept, rather than borrowing the presence socket, because
//! packet loss signalling needs exclusive reads on it.

pub mod util;

#[cfg(feature = "hydrate")]
pub mod http_transport;
#[cfg(feature = "hydrate")]
pub mod js;
#[cfg(feature = "hydrate")]
pub mod loaded_latency;
#[cfg(feature = "hydrate")]
pub mod presence;
#[cfg(feature = "hydrate")]
pub mod rtc;
#[cfg(feature = "hydrate")]
pub mod runner;
#[cfg(feature = "hydrate")]
pub mod socket;
#[cfg(feature = "hydrate")]
pub mod ws_transport;

#[cfg(feature = "hydrate")]
pub use api::*;

#[cfg(feature = "hydrate")]
mod api {
    use std::future::Future;
    use std::pin::Pin;
    use std::rc::Rc;

    use js_sys::{Array, Uint8Array};
    use uuid::Uuid;
    use wasm_bindgen::JsValue;

    use super::js::{AbortFlag, Result, js_err, random_bytes};
    use super::socket::Socket;
    use super::util::{CHUNK_BYTES, HTTP_UPLOAD_CHUNK_BYTES, UPLOAD_BUFFER_BYTES};
    use crate::model::device::device_kind_and_name_from_user_agent;
    use crate::model::profile::{Config, Direction, PhaseStatus, Profile, Step};
    use crate::model::protocol::DeviceInfo;
    use crate::model::result::{BandwidthPoint, PacketLossCounts, RunResult, Transport};

    /// Everything a run needs.
    #[derive(Clone)]
    pub struct EngineOptions {
        /// Server origin, e.g. `http://192.168.1.10:8080`. Empty = `location.origin`.
        pub origin: String,
        pub transport: Transport,
        pub profile: Profile,
        pub config: Config,
        /// Shared secret sent in `Hello` (and as a bearer token in HTTP mode).
        pub auth: Option<String>,
        pub device: DeviceInfo,
        /// Groups this run's sockets on the server. Use the presence session
        /// ID when the page has one (see the module docs).
        pub session: Uuid,
        /// Label sent with the run's presence `Status` updates.
        pub label: Option<String>,
        /// WebRTC packet loss; `None` marks packet loss steps skipped.
        pub packet_loss: Option<Rc<dyn PacketLossProbe>>,
    }

    impl EngineOptions {
        /// Options with the default config, a fresh session and the page origin.
        pub fn new(transport: Transport, profile: Profile, device: DeviceInfo) -> Self {
            Self {
                origin: String::new(),
                transport,
                profile,
                config: Config::default(),
                auth: None,
                device,
                session: Uuid::new_v4(),
                label: None,
                packet_loss: None,
            }
        }
    }

    /// Engine progress, delivered through the `start` callback.
    #[derive(Debug, Clone, PartialEq)]
    pub enum EngineEvent {
        /// All sockets are open and greeted.
        Connected {
            server_name: String,
            server_version: String,
        },
        PhaseStarted {
            index: usize,
            step: Step,
        },
        /// One idle latency probe (ms).
        Latency {
            ms: f64,
        },
        /// One finished download or upload request.
        Bandwidth {
            direction: Direction,
            point: BandwidthPoint,
        },
        /// Instantaneous throughput over a ~250 ms window, at most every 100 ms.
        Live {
            direction: Direction,
            bps: f64,
        },
        /// One latency probe taken during a transfer (ms).
        LoadedLatency {
            direction: Direction,
            ms: f64,
        },
        /// Packet loss counts, when a probe ran.
        PacketLoss(PacketLossCounts),
        PhaseFinished {
            index: usize,
            status: PhaseStatus,
        },
        Finished(Box<RunResult>),
        Error(String),
        Aborted,
    }

    /// Parameters of a packet loss step (SPEC §5.7).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct PacketLossParams {
        pub num_packets: u32,
        pub batch_size: u32,
        pub batch_wait_ms: u32,
        pub responses_wait_ms: u32,
        pub connection_timeout_ms: u32,
    }

    /// What a packet loss probe gets to work with.
    #[derive(Clone)]
    pub struct PacketLossContext {
        /// The run's control socket, for `RtcOffer` / `Ice` signalling.
        /// Nothing else reads from it while the probe runs. Messages that
        /// arrived earlier (e.g. `Peers` snapshots) may be queued: call
        /// `clear_queue()` before signalling.
        pub control: Rc<Socket>,
        pub origin: String,
        pub session: Uuid,
    }

    pub type LocalBoxFuture<T> = Pin<Box<dyn Future<Output = T>>>;

    /// Injectable WebRTC packet loss measurement. Return `Err` when WebRTC is
    /// unavailable; packet loss is then reported as unavailable, not 0%.
    pub trait PacketLossProbe {
        fn run(
            &self,
            ctx: PacketLossContext,
            params: PacketLossParams,
        ) -> LocalBoxFuture<std::result::Result<PacketLossCounts, String>>;
    }

    /// Controls a running test. Dropping it does **not** abort the run.
    #[derive(Clone)]
    pub struct EngineHandle {
        abort: AbortFlag,
    }

    impl EngineHandle {
        /// Stops the run, closes its sockets and cancels HTTP requests; the
        /// callback then receives `Aborted`.
        pub fn abort(&self) {
            self.abort.abort();
        }

        pub fn is_aborted(&self) -> bool {
            self.abort.is_aborted()
        }
    }

    /// The WebRTC packet loss probe (SPEC §5.7), for `EngineOptions::packet_loss`:
    /// `opts.packet_loss = engine::default_packet_loss_probe();`. Leaving it `None` marks
    /// packet loss steps skipped.
    pub fn default_packet_loss_probe() -> Option<Rc<dyn PacketLossProbe>> {
        Some(super::rtc::packet_loss_probe())
    }

    /// Starts a run in the background (`spawn_local`).
    pub fn start(opts: EngineOptions, on_event: impl Fn(EngineEvent) + 'static) -> EngineHandle {
        spawn_run(opts, Rc::new(on_event), None)
    }

    /// Starts a run whose first step begins in `starts_in_ms` (a group test's
    /// `GroupStart`, SPEC §5.11). The sockets open right away and `Connected`
    /// is emitted during the countdown; timed steps then follow the schedule
    /// from the shared start instant. Abort works during the countdown too.
    pub fn start_at(
        opts: EngineOptions,
        starts_in_ms: u32,
        on_event: impl Fn(EngineEvent) + 'static,
    ) -> EngineHandle {
        let at = super::js::now() + f64::from(starts_in_ms);
        spawn_run(opts, Rc::new(on_event), Some(at))
    }

    fn spawn_run(
        opts: EngineOptions,
        on_event: Rc<dyn Fn(EngineEvent)>,
        start_at: Option<f64>,
    ) -> EngineHandle {
        let abort = AbortFlag::new();
        let handle = EngineHandle {
            abort: abort.clone(),
        };
        wasm_bindgen_futures::spawn_local(super::runner::run(opts, on_event, abort, start_at));
        handle
    }

    /// Device info for `Hello`, from the User-Agent and a device id kept in
    /// `localStorage` (a fresh one if storage is unavailable).
    pub fn detect_device() -> DeviceInfo {
        let window = super::js::window();
        let ua = window.navigator().user_agent().unwrap_or_default();
        let (device_kind, device_name) = device_kind_and_name_from_user_agent(&ua);
        const KEY: &str = "speedtest.device_id";
        let storage = window.local_storage().ok().flatten();
        let device_id = storage
            .as_ref()
            .and_then(|s| s.get_item(KEY).ok().flatten())
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| {
                let id = Uuid::new_v4().to_string();
                if let Some(s) = &storage {
                    let _ = s.set_item(KEY, &id);
                }
                id
            });
        DeviceInfo {
            device_id,
            device_name,
            device_kind,
        }
    }

    /// One random upload buffer (generated once with `crypto.getRandomValues`)
    /// and its 64 KiB chunk views. Cheap to clone.
    #[derive(Clone)]
    pub struct UploadBuffer(Rc<UploadInner>);

    struct UploadInner {
        buf: Uint8Array,
        chunks: Vec<Uint8Array>,
        /// One reusable HTTP upload body (built on first use; see [`UploadBuffer::blob`]).
        http_blob: std::cell::OnceCell<web_sys::Blob>,
    }

    impl UploadBuffer {
        pub fn new() -> Result<Self> {
            let buf = random_bytes(UPLOAD_BUFFER_BYTES)?;
            let chunks = (0..UPLOAD_BUFFER_BYTES / CHUNK_BYTES)
                .map(|i| {
                    let a = (i * CHUNK_BYTES) as u32;
                    buf.subarray(a, a + CHUNK_BYTES as u32)
                })
                .collect();
            Ok(Self(Rc::new(UploadInner {
                buf,
                chunks,
                http_blob: std::cell::OnceCell::new(),
            })))
        }

        pub fn len(&self) -> usize {
            UPLOAD_BUFFER_BYTES
        }

        pub fn is_empty(&self) -> bool {
            false
        }

        /// The `i`-th full chunk.
        pub fn chunk(&self, i: usize) -> &Uint8Array {
            &self.0.chunks[i]
        }

        /// A view of `len` bytes at `offset` (no copy).
        pub fn view(&self, offset: usize, len: usize) -> Uint8Array {
            self.0.buf.subarray(offset as u32, (offset + len) as u32)
        }

        /// A `Blob` of `bytes` (at most [`HTTP_UPLOAD_CHUNK_BYTES`]) for an HTTP upload body.
        ///
        /// One chunk-sized Blob is built once and every request sends it (or a `slice` of
        /// it, which shares the same storage). Blobs are immutable and reusable, so memory
        /// stays at one chunk; a new Blob per request piles up until the browser collects
        /// them and can fail with `ERR_BLOB_OUT_OF_MEMORY`.
        pub fn blob(&self, bytes: u64) -> Result<web_sys::Blob> {
            let full = match self.0.http_blob.get() {
                Some(b) => b.clone(),
                None => {
                    let b = self.build_blob(HTTP_UPLOAD_CHUNK_BYTES)?;
                    let _ = self.0.http_blob.set(b.clone());
                    b
                }
            };
            let bytes = bytes.min(HTTP_UPLOAD_CHUNK_BYTES);
            if bytes == HTTP_UPLOAD_CHUNK_BYTES {
                Ok(full)
            } else {
                full.slice_with_f64_and_f64(0.0, bytes as f64)
                    .map_err(js_err)
            }
        }

        /// A new `Blob` of `bytes` bytes made of buffer views (no copy through WASM).
        fn build_blob(&self, bytes: u64) -> Result<web_sys::Blob> {
            let parts = Array::new();
            let full = UPLOAD_BUFFER_BYTES as u64;
            let mut left = bytes;
            while left >= full {
                parts.push(&self.0.buf);
                left -= full;
            }
            if left > 0 {
                parts.push(&self.view(0, left as usize));
            }
            web_sys::Blob::new_with_u8_array_sequence(&JsValue::from(parts)).map_err(js_err)
        }
    }

    /// Duration of one request (see the transports for what is timed).
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct Sample {
        /// The duration the bandwidth is based on.
        pub duration_ms: f64,
        pub server_ms: Option<f64>,
        /// Uploads: how long the client saw the transfer take (first byte sent to the
        /// server's reply), which includes `round_trips` network round trips. The runner
        /// uses it as a lower bound for `duration_ms` (see `util::upload_duration_ms`).
        pub client_ms: Option<f64>,
        pub round_trips: u32,
    }
}
