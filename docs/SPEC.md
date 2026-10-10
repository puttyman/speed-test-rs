# speed-test-rs — Product & Technical Specification

| | |
|---|---|
| **Status** | Draft v0.1 |
| **Last updated** | 2026-10-10 (v0.4: multi-device, captured design tokens; v0.3: Unloan-inspired design, motion, auto-saved labelled history, units toggle, PWA, GitHub Actions release) |
| **Stack** | Rust 2024 edition (stable, MSRV 1.94) · Leptos 0.8 (SSR + hydration/WASM) · Axum 0.8 · Tokio · WebSocket · WebRTC (str0m) · SQLite |
| **License** | MIT OR Apache-2.0 |

---

## 1. Summary

**speed-test-rs** is a self-hosted network speed test that looks and behaves like
[speed.cloudflare.com](https://speed.cloudflare.com). It ships two ways: as a **single
self-contained binary** (download, run, done) and as a **Docker image**. Either way you run it
on a machine in your home or office network, then open it in a browser on any device to measure
the link between that device and the server: download, upload, latency, jitter, latency under
load, and packet loss.

Public speed tests measure the path to the internet. This one measures **your LAN and Wi-Fi**.
The main use is comparing access points, bands, mesh nodes, cables and switches. Results are
saved on the server and tagged by location or access point, so you can compare them over time.

Cloudflare's test uses plain HTTP requests. This project's main engine runs over **WebSocket**
instead (an HTTP engine is available for cross-checking), and measures packet loss with a real
UDP **WebRTC data channel** served by the app itself, so no external TURN server is needed.

## 2. Goals and non-goals

### 2.1 Goals

1. **Sleek, modern and animated**: Cloudflare's information layout (headline number, metric
   tiles, quality score, per-phase charts) in an **Unloan-inspired visual style** (color,
   fonts, look and feel), with purposeful motion that shows the test as it runs (§7).
2. **Accurate at LAN speeds**: correct from about 1 Mbps up to at least 2.5 Gbps on one
   stream, and up to 10 Gbps with parallel streams on capable hardware.
3. **WebSocket-first measurement**: latency, download, upload and loaded latency all run over
   WebSocket.
4. **True packet loss** over UDP via a WebRTC data channel (unreliable and unordered).
5. **History and comparison with zero effort**: every run is saved automatically with a
   **prefilled label** (e.g. "Office – AP-2 – 5 GHz"), which is easy to change. History
   groups runs by label, and compare shows labels or runs side by side (§7.7).
6. **Multi-device awareness**: every open browser appears live in every other one. Devices
   can run a **synchronized group test** to measure how an access point shares bandwidth
   under concurrent load (§5.11).
7. **Installable** as a PWA on phones and desktops (§7.9).
8. **One-command deploy, two ways**:
   - **Standalone binary**: one executable per platform with the Leptos UI (HTML shell, WASM,
     JS, CSS, icons) embedded. `./speed-test-rs` starts the server and serves the UI, with no
     `site/` folder, runtime, or installer needed. Builds for Linux (x86_64, aarch64, armv7),
     macOS (Intel, Apple Silicon) and Windows (x86_64). See §10.1.
   - **Docker**: `docker run -p 8080:8080 -p 3478:3478/udp ghcr.io/…/speed-test-rs`. The image
     wraps the same Linux binary and targets amd64 and arm64 (Raspberry Pi, NAS boxes).
9. **Private by default**: no telemetry and no external calls. All assets are served locally.
10. **Open source and easy to contribute to**: clear module boundaries, tests, CI.

### 2.2 Non-goals (v1)

- Measuring internet speed (WAN/ISP). That can be added later by pointing at a public server,
  but it is out of scope.
- Multi-server selection or geo-distributed servers.
- User accounts, multi-tenancy, or public internet exposure. Authentication is optional and minimal (§11).
- A native mobile app or CLI client. The browser is the client. A headless CLI client is a
  possible later addition (§15).

## 3. Users and key scenarios

| # | Scenario | Notes |
|---|---|---|
| S1 | A home user walks around the house with a phone, runs a test in each room, labels each run with the room or AP, and compares them. | Primary scenario. Mobile layout matters. |
| S2 | A homelab user checks that a new 2.5 GbE switch or cable really delivers multi-gig to a desktop. | Needs parallel streams and large payloads. |
| S3 | Someone diagnoses "laggy Wi-Fi" while gaming or on video calls. | Loaded latency (bufferbloat), jitter, packet loss and quality scores matter more than raw throughput. |
| S4 | A user sees odd numbers and switches to HTTP mode to cross-check against the WebSocket engine. | Transport toggle (§5.8). |
| S5 | A user shares a run with a link (`/runs/{id}`) with someone else on the LAN. | Run detail page. |
| S6 | Two or more people (phone, laptop, TV) test **at the same time** to see how one access point shares its capacity, and each sees the others' live speeds. | Live devices panel, group test (§5.11, §7.11) |

## 4. Architecture

```
┌────────────────────────── Browser ──────────────────────────┐
│  Leptos app (SSR HTML, hydrated WASM)                        │
│   ├─ UI components (tiles, charts as inline SVG)             │
│   └─ Measurement engine (Rust → WASM, web-sys)               │
│        ├─ Control WebSocket   /ws/control  (JSON)            │
│        ├─ Data WebSocket(s)   /ws/data     (binary)          │
│        ├─ Probe WebSocket     /ws/probe    (loaded latency)  │
│        ├─ HTTP mode           /api/__down, /api/__up         │
│        └─ RTCPeerConnection + DataChannel (UDP, unreliable)  │
└──────────────────────────────┬──────────────────────────────┘
                               │ TCP :8080   UDP :3478
┌──────────────────────────────┴──────────────────────────────┐
│  speed-test-rs server (single binary, Tokio)                 │
│   ├─ Axum router: Leptos SSR routes, API, WS                 │
│   ├─ assets          /pkg/* + public/* embedded (rust-embed) │
│   ├─ measure::ws     download/upload/echo handlers           │
│   ├─ measure::http   __down / __up handlers                  │
│   ├─ measure::rtc    str0m WebRTC endpoint (UDP echo)        │
│   ├─ storage         SQLite (sqlx), migrations               │
│   └─ config          env vars / CLI flags                    │
└─────────────────────────────────────────────────────────────┘
```

### 4.1 Key decisions

| Decision | Choice | Rationale |
|---|---|---|
| Web framework | **Leptos 0.8** with SSR and hydration via `cargo-leptos` | Shares Rust types (protocol messages, result model, scoring) between client and server. Server-rendered first paint. |
| HTTP server | **Axum 0.8** (`ws` feature) | The standard pairing for `leptos_axum`. First-class WebSocket support on Tokio. |
| UI kit | **None (hand-written SCSS)**. The Thaw dependency is removed. | Matching Cloudflare's look needs custom styling anyway, and fewer dependencies mean a smaller WASM bundle. |
| Charts | **Inline SVG Leptos components** | No JS charting library. Works with SSR. Easy to theme for dark mode. |
| Measurement engine location | **Client-side Rust compiled to WASM** (`web-sys`, `js-sys`) | The browser is the measurement endpoint. Rust lets the client and server share protocol types. |
| Throughput transport | **WebSocket** (HTTP as a cross-check). WebRTC is **not** used for download/upload. | Browser data-channel (SCTP) throughput depends on the browser's implementation and CPU more than on the link, and would under-read fast LANs. WebRTC is used only for packet loss (§5.7). *Decided 2026-10-10.* |
| WebRTC | **`str0m`** (sans-IO WebRTC). Alternative: `webrtc` (webrtc-rs). | Only data channels are needed. str0m is small, actively maintained, and runs on one UDP socket, which keeps Docker port mapping simple. |
| Storage | **SQLite via `sqlx`** (compile-time-checked queries, embedded migrations), with the `sqlite` bundled build | Zero-ops, a single file in a Docker volume or the user's data directory. Bundled SQLite means no system library is needed, so the binary stays self-contained. |
| Static assets | **Embedded in the server binary** with `rust-embed` behind an `embed-assets` feature (on for release builds). Dev builds read `target/site` from disk. | Single-file distribution (§10.1). Dev keeps `cargo leptos watch` hot reload. |
| CLI | **`clap`** (derive) with env-var fallbacks | One parser for binary users (flags) and Docker users (env vars). |
| Shared types | `src/model/` module compiled for both `ssr` and `hydrate` | Protocol messages, result structs and AIM scoring live in one place. |
| Dependency policy | **Latest stable releases**. No pre-releases on `main`. `Cargo.lock` is committed. Dependabot (or Renovate) opens weekly PRs for cargo, npm, GitHub Actions and Docker base images, and CI must pass before merge. MSRV = the highest `rust-version` among locked dependencies (currently 1.94, from `sqlx`). | Stay current without surprise breakage. **Leptos 0.9** is in beta (`0.9.0-beta2`, Oct 2026). The upgrade is planned once 0.9.0 is stable and `leptos_axum` and `cargo-leptos` support it. |

## 5. Measurement methodology

The methodology follows Cloudflare's open-source engine
([`@cloudflare/speedtest`](https://github.com/cloudflare/speedtest), which powers speed.cloudflare.com),
adapted for WebSocket transport and LAN speeds.

### 5.1 Timing sources

- **Client**: `performance.now()` (monotonic, sub-millisecond).
- **Server**: `tokio::time::Instant`.
- Clocks are never compared across hosts. Every duration is measured on one side only.
  Download durations are measured on the client and upload durations on the server (§5.5).

### 5.2 Payload

- On startup the server fills one buffer (default 4 MiB) with random bytes. Data frames are
  slices of it. Random content means compression can't inflate the numbers.
- WebSocket `permessage-deflate` is **disabled** on measurement sockets.
- The client also generates one random upload buffer, using `crypto.getRandomValues` in chunks.
- **Zero-copy on the client**: received binary frames are counted by `ArrayBuffer.byteLength`
  and never copied into WASM memory. Copying would make the client CPU the bottleneck at
  multi-gigabit speeds.

### 5.3 Test sequence (default "standard" profile)

The order mirrors Cloudflare's: latency first, then download and upload ramps that alternate,
with packet loss in the middle.

| # | Phase | Parameters |
|---|---|---|
| 1 | Latency (warm-up) | 1 probe |
| 2 | Download (estimate) | 100 kB × 1, bypass min-duration |
| 3 | Latency (idle) | 20 probes |
| 4 | Download | 100 kB × 9 |
| 5 | Download | 1 MB × 8 |
| 6 | Upload | 100 kB × 8 |
| 7 | Packet loss | 1000 packets, batch 10, 10 ms between batches, 3000 ms wait |
| 8 | Upload | 1 MB × 6 |
| 9 | Download | 10 MB × 6 |
| 10 | Upload | 10 MB × 4 |
| 11 | Download | 25 MB × 4 |
| 12 | Upload | 25 MB × 4 |
| 13 | Download | 100 MB × 3 |
| 14 | Upload | 50 MB × 3 |
| 15 | Download | 250 MB × 2 |
| 16 | Download *(LAN extension)* | 1 GB × 2 |
| 17 | Upload *(LAN extension)* | 250 MB × 2 |

**Ramp halting** (as in Cloudflare): within one direction, once any request in a size set takes
≥ `bandwidth_finish_request_ms` (default **1000 ms**), larger sizes in that direction are
skipped. A slow Wi-Fi client stops early. A multi-gig wired client continues to the 1 GB steps.

**Time cap**: one test is capped at **60 s** of measurement time (configurable). When the
cap is reached, the remaining phases are skipped and marked `skipped` in the result.

**Profiles**: `quick` (phases 1–6 and 8, about 5 s), `standard` (default), `extended`
(standard plus 4 parallel streams for phases 13–17). Profiles are defined as data in
`model::profile`, so a custom JSON sequence can be passed through the API.

### 5.4 Latency and jitter (idle)

- Over the **probe WebSocket**: the client sends `{"t":"ping","seq":n}` as a text frame and the
  server answers immediately with `{"t":"pong","seq":n}`. RTT is the client's
  `performance.now()` delta.
- WebSocket protocol-level ping/pong is not used, because browsers don't expose it.
- **Latency** = median (p50) of the RTTs.
- **Jitter** = mean absolute difference between consecutive RTTs (same as Cloudflare).
- HTTP mode sends `GET /api/__down?bytes=0` instead and uses time-to-first-byte minus the
  `Server-Timing` duration.

### 5.5 Download and upload over WebSocket

The **data WebSocket** carries one request at a time:

- **Download**
  1. Client sends `{"t":"download","id":k,"bytes":N}`.
  2. Server streams binary frames of `chunk_size` bytes (default 64 KiB, last one shorter) until
     it has sent N bytes, then sends `{"t":"download_done","id":k,"server_ms":…}`.
  3. Client timing runs from the **first byte** to the **last byte** received, which excludes
     request latency. Throughput = `N × 8 / duration` bps.
- **Upload**
  1. Client sends `{"t":"upload","id":k,"bytes":N}`, then N bytes of binary frames.
  2. The server times from first byte to last byte received and replies
     `{"t":"upload_done","id":k,"bytes":N,"server_ms":d}`.
  3. **Upload throughput uses the server-side duration**. In browsers `WebSocket.send()` returns
     before data reaches the wire, so client-side timing would be inflated by buffering.
  4. The client paces sends with `bufferedAmount`, keeping ≤ 8 MiB queued, so it doesn't
     exhaust memory on slow links.
- A sample counts only if its duration is ≥ `bandwidth_min_request_ms` (default 10 ms). The
  first-phase estimate bypasses this check.
- **Parallel streams** (`streams`, default 1, max 8): the client opens S data sockets and splits
  each request across them. The sample's duration runs from the first byte on any socket to the
  last byte on all sockets.
- **Reported bandwidth** = 90th percentile (p90) of the valid samples, separately for each direction.

### 5.6 Loaded latency (bufferbloat)

- While a download or upload request runs, the client sends a ping on the **probe WebSocket**
  every `loaded_latency_throttle_ms` (default 400 ms). The probe socket is a separate TCP
  connection, so pings don't queue behind data on the same socket.
- A probe counts only while the concurrent transfer has been running for ≥
  `loaded_request_min_ms` (default 250 ms).
- Only the latest `loaded_latency_max_points` (default 20) are kept for each direction.
- Reported values:
  - **Download-loaded latency and jitter**, and **upload-loaded latency and jitter**, using the
    same reducers as §5.4.
  - **Loaded latency increase** = `max(down_loaded, up_loaded) − idle_latency`. This feeds the AIM scores.

### 5.7 Packet loss over WebRTC

1. The client asks for a session over the control socket: `{"t":"rtc_offer","sdp":…}`.
2. The server's str0m endpoint answers `{"t":"rtc_answer","sdp":…}`. ICE candidates are
   exchanged as `{"t":"ice","candidate":…}`. Only **host candidates** are needed on a LAN, so
   no STUN or TURN is involved.
3. The data channel is created with `ordered: false, maxRetransmits: 0`: unreliable,
   unordered SCTP over DTLS over UDP.
4. The client sends `num_packets` (default 1000) small messages `{seq}` in batches of 10, with
   10 ms between batches. The server echoes each message straight back.
5. After the last echo arrives, the client waits `responses_wait_ms` (default 3000 ms).
   Unreturned messages count as lost.
6. **Packet loss** = `lost / sent`. The result records `{sent, received, lost, ratio}`.
7. All WebRTC traffic uses **one UDP port** (default **3478/udp**, configurable). The server
   advertises the host IP from `SPEEDTEST_PUBLIC_IP`, or auto-detects it if unset. Under
   Docker bridge networking this variable must be set to the host's LAN IP (§10.3).
8. If WebRTC fails (UDP blocked, ICE timeout of 5 s), packet loss is reported as
   **unavailable**, not 0%. The scores then treat packet loss as 0 points, as Cloudflare does.

### 5.8 HTTP mode (cross-check)

The transport toggle in the settings switches latency, download and upload to Cloudflare-style
HTTP:

- `GET /api/__down?bytes=N` returns N random bytes with
  `Cache-Control: no-store` and a `Server-Timing: app;dur=<ms>` header.
- `POST /api/__up` reads and discards the body, then returns `Server-Timing`.
- The client times each request with the `PerformanceResourceTiming` API
  (`responseStart` → `responseEnd` for downloads, `requestStart` → `responseStart` minus server
  time for uploads).
- Loaded latency in HTTP mode uses `GET /api/__down?bytes=0` probes.
- Packet loss always uses WebRTC, whichever transport is selected.

Each run records its transport, so the history page can compare WebSocket and HTTP runs.

### 5.9 Network quality (AIM) scores

These follow Cloudflare's [Aggregated Internet Measurement](https://developers.cloudflare.com/fundamentals/speed/aim/)
model. Each metric maps to points through thresholds; a value below the first threshold gets
the first point value.

| Metric | Thresholds | Points per band |
|---|---|---|
| Packet loss (ratio) | 0.01, 0.05, 0.25, 0.5 | 10, 5, 0, −10, −20 |
| Latency (ms) | 10, 20, 50, 100, 500 | 20, 10, 5, 0, −10, −20 |
| Loaded latency increase (ms) | 10, 20, 50, 100, 500 | 20, 10, 5, 0, −10, −20 |
| Jitter (ms) | 10, 20, 100, 500 | 10, 5, 0, −10, −20 |
| Download (bps) | 1e6, 1e7, 5e7, 1e8 | 0, 5, 10, 20, 30 |
| Upload (bps) | 1e6, 1e7, 5e7, 1e8 | 0, 5, 10, 20, 30 |

| Use case | Inputs | Class thresholds (sum of points, floored at 0) → bad / poor / average / good / great |
|---|---|---|
| Video streaming | latency, packet loss, download, loaded latency increase | 15, 20, 40, 60 |
| Online gaming | latency, packet loss, loaded latency increase | 5, 15, 25, 30 |
| Video chatting | latency, jitter, packet loss, loaded latency increase | 5, 15, 25, 40 |

The scoring is a pure function in `model::scores` with table-driven unit tests.

### 5.10 Statistics helpers

`model::stats` provides:
- `percentile(values, p)` using linear interpolation (same as d3 `quantile`)
- `median`
- `jitter` (mean absolute difference of consecutive values)
- `box_plot` (min, q1, median, q3, max) for the latency charts

### 5.11 Concurrent tests from multiple devices

Wi-Fi capacity is shared. One phone getting 600 Mbps says little about what three devices get
together. The app makes concurrency visible and measurable in two ways.

**1. Live presence (always on).** Every open browser keeps its `/ws/control` socket open
from page load, not only during a test, and appears in every other browser's **Devices**
panel (§7.11) with its device name, label, state (idle, or the phase it is testing) and live
speed.
- **Live speed comes from the server.** The server counts the bytes it sends and receives on
  each session's data sockets and computes a per-device rate every 250 ms. A slow or
  backgrounded client therefore can't misreport, and peers see consistent numbers. Clients
  only report their phase and, at the end, their summary.
- The server broadcasts a single combined `Peers` frame at most **4 times per second** to all
  control sockets. The control socket is separate from the data sockets, so presence traffic
  (a few hundred bytes per frame) never queues behind measurement data.

**2. Group test (synchronized).** To measure shared capacity on purpose:
1. Any device taps **Test together**. This creates a group, and every other open device shows
   an invite ("Office Mac wants to test together · Join").
2. Devices join. The host sees the member list and taps **Start**.
3. The server sends `GroupStart { starts_in_ms: 3000, plan }` to all members, and each shows a
   3-2-1 countdown. LAN delivery jitter (a few ms) is negligible against phase lengths of seconds.
4. Group mode uses the **time-based `concurrent` profile** instead of the size ramp, so all
   devices load the network at the same moments:
   1. idle latency for 3 s
   2. download for 10 s (continuous stream, loaded-latency probes running)
   3. 2 s pause
   4. upload for 10 s
   5. packet loss, optional
5. **The server computes the results** from its own byte counters over the common window:
   - per-device throughput
   - **aggregate** throughput (sum of all devices)
   - **each device's share**
   - **fairness**, using Jain's index `(Σx)² / (n·Σx²)`: 1.0 means a perfectly even split
   - per-device loaded latency (from that device's probes)
6. A device that drops out mid-test is marked `incomplete`, and the aggregate covers the
   devices that finished the window.

**3. Overlap flagging (solo tests).** If another session moves data while a solo test runs,
the server records the overlap. Each result gets
`concurrency: { peers_max, overlap_ms, overlap_ratio }`. The UI shows a badge ("Tested while
2 other devices were active"), and history and compare can filter overlapped runs out, so a
loaded result isn't mistaken for the access point's capacity. Before a solo test starts while
others are testing, the UI asks: **Wait** (queue until they finish) or **Test anyway**.

**Server bottleneck warning.** Concurrent tests can saturate the *server's* link rather than
the Wi-Fi. On Linux the server reads its interface speed (`/sys/class/net/<iface>/speed`) for
the interface serving each client. If the aggregate reaches ≥ 90% of it, the result reads
"Total is close to the server's 1 Gbps link: the server may be the bottleneck, not the Wi-Fi".
Where the speed can't be read, the server can be told with `--server-link-mbps`.

**Privacy.** Presence shares device name, device type and label, not IP addresses (IPs appear
only in run details, as today). A device can set **Hide me** in settings, after which others
see "1 hidden device testing" with its speed but no name. `--no-presence` turns the feature
off server-wide.

## 6. Protocols and APIs

### 6.1 WebSocket endpoints

| Path | Frames | Purpose |
|---|---|---|
| `/ws/control` | JSON text | Opened on page load and kept open: presence (§5.11), session start and stop, progress events, group tests, WebRTC signalling |
| `/ws/data` | JSON text (requests, acks) + binary (payload) | Download and upload transfers. Several may be open for parallel streams. |
| `/ws/probe` | JSON text | Ping/pong for idle and loaded latency |

Every connection opens with `{"t":"hello","session":"<uuid>","role":"control|data|probe"}`, so
the server can group sockets into one test session.

All JSON messages are tagged by `"t"`. They are defined once as `#[serde(tag = "t")]` enums in
`model::protocol` and shared by client and server:

```rust
enum ClientMsg {
    Hello { session: Uuid, role: Role, device: Option<DeviceInfo> }, // device on the control socket
    Status { phase: Option<Phase>, label: Option<String> },       // presence updates
    SetHidden { hidden: bool },
    GroupCreate,
    GroupJoin { group: Uuid },
    GroupLeave,
    GroupStart,                                                   // host only
    Ping { seq: u32 },
    Download { id: u32, bytes: u64 },
    Upload { id: u32, bytes: u64 },
    RtcOffer { sdp: String },
    Ice { candidate: String },
    Abort,
}
enum ServerMsg {
    Welcome { server_version: String, server_name: String, limits: Limits },
    Pong { seq: u32 },
    DownloadDone { id: u32, server_ms: f64 },
    UploadDone { id: u32, bytes: u64, server_ms: f64 },
    RtcAnswer { sdp: String },
    Ice { candidate: String },
    Peers { peers: Vec<Peer>, hidden_active: u32 },  // full snapshot, ≤ 4 Hz
    GroupInvite { group: Uuid, host: String },
    GroupUpdate { group: Uuid, members: Vec<Peer>, state: GroupState },
    GroupStart { group: Uuid, starts_in_ms: u32, plan: Profile },
    GroupResult { group: Uuid, result: GroupResult },
    Error { code: ErrorCode, message: String },
}
// Peer { id, device_name, device_kind, label, is_you, state: Idle | Testing { phase, down_bps, up_bps, latency_ms } }
```

**Server limits**, advertised in `Welcome` and enforced:
- `max_bytes_per_request` (default 2 GB)
- `max_streams` (8)
- `max_concurrent_sessions` (default **16**). Concurrency is a feature (§5.11), so this is
  only a resource guard. Beyond it, the server returns `Error{code:Busy}` and the UI shows
  "Server busy: 16 tests are already running".
- Presence connections (control sockets with no active test) don't count toward the limit.

### 6.2 HTTP API

| Method & path | Description |
|---|---|
| `GET /api/health` | `{"status":"ok","version":"x.y.z"}`. Used by the Docker HEALTHCHECK. *(Implemented.)* |
| `GET /api/info` | Server name, version, limits, client IP as seen by the server, configured UDP port |
| `GET /api/__down?bytes=N` | HTTP-mode download (§5.8) |
| `POST /api/__up` | HTTP-mode upload (§5.8) |
| `POST /api/runs` | Save a completed run (body: `RunResult`). Returns `{id}`. |
| `GET /api/runs?label=&device=&from=&to=&limit=&cursor=` | List runs, newest first, cursor-paginated |
| `GET /api/runs/{id}` | Full run, including raw points |
| `PATCH /api/runs/{id}` | Update the label or notes |
| `DELETE /api/runs/{id}` | Delete a run |
| `GET /api/labels?device=` | Distinct labels with run counts, last-used time and the median of the last 5 runs. Ordered by last use, which feeds the label chips (§7.7). |
| `GET /api/groups/{id}` | A group test: members, per-device results, aggregate, shares, fairness |
| `GET /api/compare?labels=a,b` or `?ids=…` | Data for the compare view: per-label medians or individual runs |
| `GET /api/runs/export?format=csv\|json` | Export history |

Leptos server functions can wrap the run endpoints for the UI, but the REST shape above is the
public contract, so scripts can use it too.

### 6.3 Result model (`model::result::RunResult`)

```text
RunResult {
  id, created_at, label?, notes?,
  transport: "websocket" | "http", profile, streams,
  client: { ip, user_agent, device_id, device_name },
  tls: bool,
  group_id?, concurrency: { peers_max, overlap_ms, overlap_ratio },
  summary: {
    download_bps?, upload_bps?,
    latency_ms?, jitter_ms?,
    down_loaded_latency_ms?, down_loaded_jitter_ms?,
    up_loaded_latency_ms?,   up_loaded_jitter_ms?,
    packet_loss_ratio?,      // None = unavailable
  },
  scores: { streaming?, gaming?, rtc? : { points, class } },
  points: {
    latency: [ms],
    download: [{ bytes, bps, duration_ms, server_ms }],
    upload:   [{ bytes, bps, duration_ms, server_ms }],
    down_loaded_latency: [ms], up_loaded_latency: [ms],
    packet_loss: { sent, received, lost }?,
  },
  phases: [{ kind, bytes?, count, status: done|skipped|failed }],
  duration_ms,
}
```

## 7. UI / UX

### 7.1 Design direction

Three sources, each used for a different thing:

| Aspect | Source | What it means |
|---|---|---|
| **Information layout** | speed.cloudflare.com | Big headline number, metric tiles, quality scores, detailed per-phase charts |
| **Visual style**: color, typography, shape, look and feel | **Unloan** (unloan.com.au), as inspiration | Palette, type scale and weights, corner radii, spacing rhythm, surface and depth treatment, button and card style |
| **Language and copywriting** | **Our own** (§7.5). Unloan's copy and tone are *not* used. | Short, plain, technical-friendly |

**Inspired by, not copied.** Unloan is a CommBank brand. The project does not use its name,
logo, illustrations or proprietary font files, and nothing in the UI suggests a connection to
it. Where Unloan uses a licensed typeface, we use the closest open-licensed equivalent
(OFL / Apache), **self-hosted** inside the binary (§2.1 goal 9: no external requests).

Goal: **sleek, modern, calm**. Generous whitespace, confident type, few colors used
deliberately, and motion that explains what the test is doing rather than decorating it.

### 7.2 Design tokens

All visual decisions live in `style/tokens.scss` as CSS custom properties on `:root`. Dark
values are set under `@media (prefers-color-scheme: dark)` and `[data-theme="dark"]` for the
manual toggle. Components reference tokens only and never raw values.

| Group | Tokens | Notes |
|---|---|---|
| Brand color | `--color-brand`, `--color-brand-strong`, `--color-brand-soft`, `--color-on-brand` | Taken from Unloan's primary and secondary colors |
| Neutrals | `--color-bg`, `--color-surface`, `--color-surface-raised`, `--color-border`, `--color-text`, `--color-text-muted` | Light and dark sets |
| Data series | `--series-download`, `--series-upload`, `--series-latency`, `--series-loss` | Derived from the brand palette. Must stay distinguishable for color-blind users and meet 3:1 against the surface. |
| Status | `--status-great`, `--status-good`, `--status-average`, `--status-poor`, `--status-bad` | Quality badges and bufferbloat |
| Typography | `--font-display`, `--font-body`, `--font-mono`; `--text-xs … --text-hero` (fluid `clamp()` scale); `--weight-*` | Display font for numbers and headings. Tabular numerals for all metrics. |
| Shape | `--radius-sm`, `--radius-md`, `--radius-lg`, `--radius-pill` | Matches Unloan's corner language |
| Space | `--space-1 … --space-10` (4 px base) | |
| Depth | `--shadow-1`, `--shadow-2`, `--blur-glass` | Cards and the raised hero |
| Motion | `--ease-out`, `--ease-spring`, `--dur-fast` (120 ms), `--dur-base` (240 ms), `--dur-slow` (480 ms) | §7.3 |

**Values (captured 2026-10-10, see `docs/design/tokens.md`; implemented in `style/_tokens.scss`):**
- **Palette**: monochrome brand (`#101010` on `#ffffff`), with `#f7f7f7` surfaces and `#dedede` /
  `#cccccc` hairlines. Accents are used sparingly: a pink→amber gradient (`#ff7bad → #ffba2f`,
  used for the brand mark) and lime `#f2ff00` (never as text on light).
- **Fonts**: Unloan's proprietary display font is replaced by **IBM Plex Mono 500** for
  display headings. **Inter** (variable, 100–900) is used for UI, body and numbers. Numbers are
  Inter 700 with tabular numerals. Both fonts are OFL and self-hosted in `public/fonts/`.
- **Tracking**: headings −3%.
- **Shape**: cards 16–24 px, inputs and chips 8 px, every button a 32 px pill.
- **Depth**: flat fills, with a diffuse `0 3px 40px rgba(0,0,0,.07)` only on floating
  controls. The nav uses frosted glass (`blur(20px)`).
- **Motion**: color-only button feedback at 200 ms, segmented toggle at 350 ms, reveals at
  600 ms `cubic-bezier(.25,1,.5,1)`.
- **Data series**, validated with the dataviz color checks (all-pairs CVD and contrast, both
  themes): download `#0066cc`, upload `#d6457f`, latency `#bf7400` (dark theme: `#3987e5`,
  `#d55181`, `#c98500`). **Per-device colors** in group tests use the validated 8-slot
  categorical palette in fixed order. Above 3 devices, charts add direct labels (identity is
  never carried by color alone).

### 7.3 Motion

Motion shows the test's progress and direction. The rules:

| Element | Behavior |
|---|---|
| **Packet flow** (signature visual) | A canvas behind the hero where particles stream **down** during download and **up** during upload, tinted with the series color. Particle speed and density follow live throughput on a log scale. During latency phases, single pulses travel out and back, one per probe. Idle state: a slow ambient drift. |
| **Number count-up** | The hero number and tile values tween to each new value with a spring (`--ease-spring`, about 400 ms). They never jump. Tabular numerals keep the width stable. |
| **Live throughput chart** | An area chart under the hero draws in real time (one point per completed sample plus interpolated live points). Payload-size changes are marked with thin ticks. |
| **Phase timeline** | A stepper (Latency → Download → Upload → Packet loss) where the active step fills like a progress bar, completed steps get a check, and skipped steps are dimmed. |
| **Result reveal** | When the run finishes, the tiles settle in turn (staggered 60 ms, translate and fade), the quality badges flip in, and the detail charts draw their lines. |
| **Page transitions** | The View Transitions API is used for navigation between test, history, compare and run pages, with the hero number morphing into its history row. It does nothing in browsers without support. |
| **Micro-interactions** | Button press scale, hover lift on cards, and a pulsing ring on the Start button while idle |

**Performance rules.** The animation must never affect the measurement:
- Only `transform` and `opacity` are animated in CSS. The packet-flow canvas runs on `requestAnimationFrame`.
- The particle count is capped, scaled to device pixel ratio and hardware concurrency.
- If frame time exceeds 20 ms for 1 s, the particles drop to a lightweight mode. They also pause while the tab is hidden.
- Measurement code never waits on animation. Animation reads from a signal fed by the engine, never the other way round.

**Reduced motion.** With `prefers-reduced-motion: reduce` (or the in-app toggle), particles are
replaced by a static gradient, count-ups become crossfades, and view transitions are turned off.

### 7.4 Layout and components

- **Theme**: **light-first**, like Unloan (white page, `#f7f7f7` cards, near-black type and pill buttons). The dark theme is derived from Unloan's dark sections (`#171d26`), since Unloan has no dark mode. Follows `prefers-color-scheme` by default, with a manual toggle.
- **Hero**:
  - a large display-weight headline number (current or final download speed) over the packet-flow canvas
  - the phase timeline
  - the **Start / Stop / Test again** pill button
  - the label field (§7.7)
- **Metric tiles**: Download, Upload, Latency, Jitter, Packet loss. Each tile has a series-color
  accent, a large tabular value with its unit, and a small sparkline. Each tile updates live while its phase runs.
- **Network quality** card: Video streaming, Online gaming and Video chatting, each with a
  status badge that shows the class name as text.
- **Server / connection** card: server name, version, client IP, transport, streams and profile.
- **Detail section**, one card per metric:
  - Latency: box plot of idle, download-loaded and upload-loaded RTTs, with jitter values.
  - Download / Upload: a dot plot of every sample per payload size (100 kB … 1 GB), with the p90 line.
  - Packet loss: sent / received / lost, as a bar.

### 7.5 Voice and copy

Our own voice, unrelated to Unloan's:
- **Plain and short**: "Download", "Latency under load", "Test again". No marketing language,
  puns or exclamation marks.
- **Explain jargon once**: an ⓘ tooltip on each metric gives a one-sentence explanation
  (e.g. "Jitter: how much latency varies between measurements").
- Numbers carry the message. The copy only frames them.

### 7.6 Units

- A **units toggle** in the header switches between **Mbps** (default, as on Cloudflare) and **MB/s**.
- Values auto-scale: ≥ 1000 Mbps shows as Gbps, ≥ 1000 MB/s as GB/s.
- The choice is stored per device in `localStorage`. Results are always stored in bps, so the
  toggle only changes how they are displayed (history, compare, exports, share cards included).

### 7.7 Labels, history and compare

**What labels are for.** A label records *where or what* a run measured: a room, an access
point, a band or a cable (e.g. "Bedroom", "Office AP – 5 GHz", "Desk – wired"). History groups
runs by label, so you can answer "which access point is best?" or "did moving the mesh node
help?" by comparing labels instead of reading individual runs.

**Easy by default: zero typing needed.**
- **Every finished run is saved automatically** when history is on. There is no Save step.
- The label field is **prefilled** before the test starts:
  1. with the **last label used on this device** (stored in `localStorage`), so repeat tests in the same spot need no typing;
  2. otherwise with a generated default, **`<device> · <date>`** (e.g. "iPhone · 10 Oct"). The device name comes from User-Agent Client Hints, falling back to User-Agent parsing.
- To change it, tap the label. Recent labels appear as one-tap **chips** (from
  `GET /api/labels`), with free-text entry for a new one. A label can be edited before the run
  or afterwards, on the result screen or in history (`PATCH /api/runs/{id}`).
- Each browser gets a random **device ID** (in `localStorage`) and a friendly **device name**,
  so history can be filtered to "this device" and runs from different devices stay apart.

**History** (`/history`):
- Runs grouped by label (collapsible), newest first. Each group header shows the median of that
  label's last 5 runs.
- Filters: label, device, date range.
- Inline label rename. Delete. Export (CSV / JSON).

**Compare** (`/compare`):
- Pick **2–4 runs or labels** from history. A label stands for the median of its last 5 runs.
- Side-by-side tiles with **delta badges** against the first selection (e.g. "+23% ↑",
  "−8 ms ↓"), using status colors for better and worse.
- Overlaid charts (throughput dot plots, latency box plots) with one series color per selection.
- The best value per metric is highlighted.
- Shareable through the URL (`/compare?labels=Bedroom,Office` or `?ids=…`).

### 7.8 Pages

| Route | Content |
|---|---|
| `/` | Test page. The test does **not** auto-start, so users can check the label first. `?autostart=1` starts it on load. |
| `/history` | §7.7 |
| `/compare` | §7.7 |
| `/runs/{id}` | A read-only result page with the same layout as the finished test page. Shareable. |
| Settings drawer | Transport (WebSocket/HTTP), profile, parallel streams, units, theme, reduced motion, device name |

### 7.9 Progressive Web App

- `public/manifest.webmanifest`:
  - `name`, `short_name`, `display: standalone`, `start_url: /`, and `theme_color` / `background_color` from the tokens
  - icons at 192, 512 and maskable 512
  - shortcuts: "Run test" (`/?autostart=1`) and "History"
- `public/sw.js` service worker:
  - **Caches the app shell**: hashed `/pkg/*` assets (cache-first, immutable), icons, fonts, and an offline page that says "Can't reach the speed-test server on this network".
  - **Never caches or intercepts** `/api/*`, `/ws/*` or measurement traffic (network-only, bypassed early).
  - When a new version is waiting, a toast reads "Update available, reload".
- iOS: `apple-touch-icon` and the `apple-mobile-web-app-*` meta tags. Standalone mode respects safe-area insets.
- **Secure context requirement.** Browsers only register service workers and offer "Install"
  on **HTTPS** (or `localhost`). On plain `http://192.168.x.x`:
  - the app works fully;
  - the manifest still allows "Add to Home Screen" on some browsers;
  - there is no offline shell or install prompt.

  For the full PWA, serve over HTTPS with a certificate the phone trusts: the built-in TLS
  listener (§9.2, `--tls-cert`/`--tls-key`, e.g. from `mkcert` or a home CA) or a reverse
  proxy (Caddy, Traefik, Nginx Proxy Manager) with a real domain. The README covers both.
  Self-signed certificates are not enough, because browsers refuse to register service workers
  on pages with certificate errors.
- **Measuring over HTTPS** encrypts everything (`wss://`), which costs CPU on slow hosts at
  multi-gigabit speeds. The settings drawer shows the transport ("WebSocket · TLS"), and history
  records it, so TLS and non-TLS runs aren't compared without noticing.

### 7.10 Accessibility and responsiveness

- Works at 360 px width with a 16 px gutter and no horizontal scroll. Tiles reflow to 2 columns, then 1.
- Every chart has a text alternative (a values table in `<details>`).
- Text meets WCAG AA contrast. Badges and deltas also use text and arrows, not just color.
- Live values are announced politely (`aria-live="polite"`), throttled to phase completion.
- Full keyboard support, and visible focus rings in the brand color.

### 7.11 Devices panel and group test

- **Devices panel** on the test page: a horizontal strip of device cards (it wraps to a list on
  mobile). Each card shows:
  - device-type icon, name and label
  - "You" on the current device
  - state: *Idle*, or *Testing · Download* with a live speed and a tiny sparkline in the
    device's categorical color

  New devices slide in, and leaving devices fade out (`--dur-slow`). When nobody else is
  connected, the panel collapses to a single line: "Only this device is connected".
- **While others are testing**, the packet-flow canvas also shows their traffic as faint
  particles in their device colors, so it is visible that the network is shared.
- **Before a solo test** while others are active, an inline prompt offers **Wait for them** or
  **Test anyway** (§5.11). Results that overlapped carry a "Shared network" badge.
- **Test together**:
  - The host taps it, and every other device sees an invite toast with **Join**.
  - The host sees members join live, then taps **Start**, and all devices count down 3-2-1.
  - During the test, each device shows one lane per member (live bar and speed) plus a big
    **Total** number.
  - The **result** shows:
    - a stacked area chart of each device's throughput over time, with direct labels
    - the aggregate download and upload
    - each device's share as a percentage
    - a fairness meter (Jain's index shown as "Even / Uneven split")
    - the server-bottleneck warning, when it applies
  - Group results appear in history as one expandable row.

## 8. Storage

SQLite at `SPEEDTEST_DB_PATH` (default `/data/speedtest.db`). Migrations are embedded with
`sqlx::migrate!`.

```sql
CREATE TABLE runs (
  id            TEXT PRIMARY KEY,          -- UUIDv7 (time-ordered)
  created_at    TEXT NOT NULL,             -- RFC 3339 UTC
  label         TEXT,
  notes         TEXT,
  transport     TEXT NOT NULL,
  profile       TEXT NOT NULL,
  streams       INTEGER NOT NULL,
  client_ip     TEXT,
  user_agent    TEXT,
  device_id     TEXT,                      -- random per-browser ID (§7.7)
  device_name   TEXT,
  tls           INTEGER NOT NULL DEFAULT 0,
  group_id      TEXT REFERENCES groups(id),
  peers_max     INTEGER NOT NULL DEFAULT 0,  -- other sessions active during the run
  overlap_ratio REAL NOT NULL DEFAULT 0,
  download_bps  REAL, upload_bps REAL,
  latency_ms    REAL, jitter_ms  REAL,
  down_loaded_latency_ms REAL, up_loaded_latency_ms REAL,
  packet_loss_ratio REAL,
  score_streaming TEXT, score_gaming TEXT, score_rtc TEXT,
  duration_ms   INTEGER NOT NULL,
  detail_json   TEXT NOT NULL              -- full RunResult (points, phases)
);
CREATE INDEX runs_created_at ON runs(created_at DESC);
CREATE TABLE groups (
  id TEXT PRIMARY KEY, created_at TEXT NOT NULL, label TEXT,
  device_count INTEGER NOT NULL,
  aggregate_download_bps REAL, aggregate_upload_bps REAL,
  fairness_download REAL, fairness_upload REAL,
  detail_json TEXT NOT NULL                -- per-device time series and shares
);
CREATE INDEX runs_label      ON runs(label, created_at DESC);
CREATE INDEX runs_device     ON runs(device_id, created_at DESC);
```

- Summary columns are denormalized so the history and compare queries stay fast. The full
  detail lives in `detail_json`.
- **Retention**: `SPEEDTEST_RETENTION_DAYS` (default unset, meaning keep forever). A daily task
  prunes older runs.
- `SPEEDTEST_HISTORY=false` turns persistence off entirely, giving a stateless, Cloudflare-like mode.

## 9. Configuration

Every setting can be given as a CLI flag or an environment variable. Precedence:
**flag > env var > default**. The binary needs **no** configuration to start: no `LEPTOS_*`
variables, no `site/` directory and no config file.

### 9.1 Command line

```text
speed-test-rs [serve] [OPTIONS]     Run the server and serve the UI (default command)
speed-test-rs healthcheck           Exit 0 if the local server answers /api/health (implemented)
speed-test-rs --version | --help
```

### 9.2 Options

| Flag | Env var | Default | Description |
|---|---|---|---|
| `-l, --listen <ADDR>` | `SPEEDTEST_LISTEN` (alias `LEPTOS_SITE_ADDR`) | `0.0.0.0:8080` **and** `[::]:8080` (all interfaces, IPv4 and IPv6) | HTTP/WS listen address. Listening on every interface is the default for both the binary and Docker, so phones on the LAN can reach it without extra flags (*decided 2026-10-10*). Pass e.g. `127.0.0.1:8080` to restrict it. If IPv6 is unavailable, the server logs a warning and continues on IPv4 only. `cargo leptos watch` uses `127.0.0.1:3000`. |
| `--udp-port <PORT>` | `SPEEDTEST_UDP_PORT` | `3478` | WebRTC UDP port |
| `--public-ip <IP>` | `SPEEDTEST_PUBLIC_IP` | auto | IP advertised in ICE candidates. Set it under Docker bridge networking. |
| `--server-name <NAME>` | `SPEEDTEST_SERVER_NAME` | hostname | Shown in the UI and the history |
| `--data-dir <DIR>` | `SPEEDTEST_DATA_DIR` | see §9.3 | Directory for the database |
| `--db-path <FILE>` | `SPEEDTEST_DB_PATH` | `<data-dir>/speedtest.db` | SQLite file (overrides `--data-dir`) |
| `--no-history` | `SPEEDTEST_HISTORY=false` | history on | Turns run persistence off |
| `--retention-days <N>` | `SPEEDTEST_RETENTION_DAYS` | – | Auto-prune age |
| `--max-sessions <N>` | `SPEEDTEST_MAX_SESSIONS` | `16` | Concurrent test sessions (a resource guard; §6.1) |
| `--no-presence` | `SPEEDTEST_PRESENCE=false` | presence on | Turns off the live devices panel and group tests (§5.11) |
| `--server-link-mbps <N>` | `SPEEDTEST_SERVER_LINK_MBPS` | auto (Linux) | Server link speed, for the bottleneck warning |
| `--max-bytes <N>` | `SPEEDTEST_MAX_BYTES` | `2000000000` | Per-request byte cap |
| – | `SPEEDTEST_AUTH_TOKEN` | – | If set, the API, WebSocket and history require it (§11). Env only, so it doesn't show in `ps`. |
| `--open` | – | off | Open the UI in the default browser after startup (desktop use) |
| `--tls-listen <ADDR>` | `SPEEDTEST_TLS_LISTEN` | `0.0.0.0:8443`, `[::]:8443` when a cert is given | HTTPS/WSS listener, needed for the full PWA (§7.9). Runs alongside the plain HTTP listener. |
| `--tls-cert <FILE>`, `--tls-key <FILE>` | `SPEEDTEST_TLS_CERT`, `SPEEDTEST_TLS_KEY` | – | PEM certificate and key (e.g. from `mkcert` or a home CA). TLS is off unless both are set. Uses `rustls`. |
| `--site-root <DIR>` | `LEPTOS_SITE_ROOT` | embedded | Serve UI assets from disk instead of the embedded copy (development, theming) |
| `--log <FILTER>` | `RUST_LOG` | `info` | Log filter (`tracing-subscriber`) |

### 9.3 Data directory defaults

| Environment | Default `data-dir` |
|---|---|
| Linux binary | `$XDG_DATA_HOME/speed-test-rs` (usually `~/.local/share/speed-test-rs`) |
| macOS binary | `~/Library/Application Support/speed-test-rs` |
| Windows binary | `%APPDATA%\speed-test-rs` |
| Docker image | `/data` (set through `SPEEDTEST_DATA_DIR` in the image; mount a volume there) |

Resolved with the `directories` crate. The directory is created on first start.

### 9.4 Startup output

On start the server prints its version, the data directory, and a reachable URL for **every
non-loopback IPv4/IPv6 interface** (e.g. `http://192.168.1.10:8080`), because "which address do
I type on my phone?" is the first question in scenario S1. It also prints a QR code of the
first LAN URL in the terminal (`qrcode` crate, can be turned off with `--no-qr`) so a phone can
scan it.

## 10. Packaging and deployment

The **standalone binary is the primary artifact**. The Docker image is that same binary in a
minimal container.

### 10.1 Standalone binary

**What's inside.** The server, with the `target/site` output of `cargo leptos build --release`
(`pkg/*.wasm`, `pkg/*.js`, `pkg/*.css`, and everything in `public/`) compiled in through
`rust-embed`. SSR HTML is rendered by the server as usual, so no files are read from disk at
runtime. SQLite is bundled. Expected size is about 10–15 MB (the WASM bundle is about 300 KB
today).

**Serving embedded assets.**
- An Axum handler under `/pkg/*` and for `public/` files replaces `leptos_axum`'s disk-based
  `file_and_error_handler`. Requests that match no asset fall through to the Leptos 404 page.
- `Content-Type` comes from the file extension (`mime_guess`). `.wasm` must be
  `application/wasm` so browsers can stream-compile it.
- Assets are stored pre-compressed (Brotli and gzip, via the `rust-embed` `compression`
  feature, or by compressing at build time) and served by `Accept-Encoding`.
- Caching: cargo-leptos `hash-files = true` gives content-hashed asset names, which are
  served with `Cache-Control: public, max-age=31536000, immutable`. HTML is served with
  `no-cache`. An `ETag` comes from the embedded file hash.
- `--site-root` (or `LEPTOS_SITE_ROOT`) switches back to serving from disk, which is useful
  when developing or theming.

**Build pipeline.** WASM is platform-independent, so the front end is built **once** and
embedded into every platform's server build:

```bash
# 1. Build the front end (WASM/JS/CSS) and the site folder
cargo leptos build --release
# 2. Build the server for each target with the site embedded
LEPTOS_OUTPUT_NAME=speed-test-rs \
  cargo build --release --no-default-features --features ssr,embed-assets --target <triple>
```

Step 2 is a separate `cargo build`, so `rust-embed` always sees a finished `target/site` and
never races cargo-leptos. A `just dist` / `cargo xtask dist` recipe wraps both steps.
`build.rs` fails the `embed-assets` build with a clear message if `target/site/pkg` is missing.

**Release targets** (built in GitHub Actions on tags; `cross` or `cargo-zigbuild` for the
Linux cross targets):

| Target triple | Platform | Notes |
|---|---|---|
| `x86_64-unknown-linux-musl` | Linux x86_64 | Static. Runs on any distro, NAS, Proxmox LXC. |
| `aarch64-unknown-linux-musl` | Linux arm64 | Raspberry Pi 4/5 (64-bit OS), arm NAS |
| `armv7-unknown-linux-musleabihf` | Linux armv7 | Raspberry Pi 2/3 (32-bit OS) |
| `x86_64-apple-darwin`, `aarch64-apple-darwin` | macOS | Unsigned in v1 (see notes) |
| `x86_64-pc-windows-msvc` | Windows | `.exe` |

Artifacts are named `speed-test-rs-<version>-<target>.tar.gz` (`.zip` on Windows), with a
`SHA256SUMS` file, attached to the GitHub Release.

**Release workflow: hand-written GitHub Actions** (*decided 2026-10-10*). It gives full
control over the two-step Leptos build. `.github/workflows/release.yml` runs on tags `v*`:

1. **`site`** (ubuntu): `cargo leptos build --release`, then uploads `target/site` as an artifact.
2. **`binaries`**: a matrix over the targets above. Each job:
   - downloads the `site` artifact into `target/site`;
   - runs `cargo build --release --features ssr,embed-assets --target <triple>`. Linux musl
     targets use `cargo-zigbuild` (or `cross`) on ubuntu, macOS uses `macos-latest` (arm64, with
     the x86_64 target cross-compiled), and Windows uses `windows-latest`;
   - runs the binary smoke test (§12) where the runner can execute the target;
   - packages `tar.gz` / `zip` with README and licenses.
3. **`docker`**: `docker buildx` builds `linux/amd64` and `linux/arm64` from the musl binaries
   (`Dockerfile.release`) and pushes to GHCR with the version tag and `latest`.
4. **`publish`**: generates `SHA256SUMS` and creates the GitHub Release with all archives and
   notes generated from commits since the last tag.

`.github/workflows/ci.yml` runs on every PR: fmt, clippy (`ssr`, `hydrate`,
`ssr,embed-assets`), unit and integration tests, Playwright e2e, and a Linux `dist` build with
the smoke test. Rust and cargo caches use `Swatinem/rust-cache`. Action versions are pinned by SHA.

An `install.sh` (detects OS/arch, downloads the archive, verifies its checksum) may be added later.

`cargo install speed-test-rs` is **not** supported in v1. The WASM front end needs
`cargo-leptos` and a pre-build step that plain `cargo install` can't run. Prebuilt binaries
are the supported path.

**Platform notes** (for the README):
- **Windows**: the first run triggers a Windows Defender Firewall prompt. Allow it on
  *Private* networks for TCP 8080 and UDP 3478.
- **macOS**: unsigned binaries are quarantined. Run `xattr -d com.apple.quarantine
  speed-test-rs` once, or right-click → Open. Code signing and notarization come later.
- **Linux**: binding to ports below 1024 needs `CAP_NET_BIND_SERVICE`. The defaults avoid that.

**Running as a service.** `contrib/` ships:
- `speed-test-rs.service`, a systemd unit running as a dedicated `DynamicUser` with
  `StateDirectory=speed-test-rs`
- `com.speed-test-rs.plist`, a launchd agent
- a short Windows note on `sc.exe create` or NSSM

### 10.2 Docker image

- Multi-stage `Dockerfile`:
  - **Build stage**: `rust:1-trixie` with the `wasm32-unknown-unknown` target and a
    pinned `cargo-leptos` release binary. It runs the same two-step build as §10.1, with
    `embed-assets` enabled.
  - **Runtime stage**: today this is `debian:trixie-slim` with a non-root user, the server
    binary and `site/`. *(Implemented in Milestone 0.)* Once assets are embedded (M7), it
    becomes `gcr.io/distroless/static` (or `scratch`) with only the musl binary, about 15 MB in total.
- `HEALTHCHECK` runs `speed-test-rs healthcheck`, a built-in subcommand that calls `/api/health`. The runtime image needs no curl or apt packages, so this works in distroless or scratch images.
- The release workflow can skip the in-container Rust build and `COPY` the already-built musl
  binaries from §10.1 (one `Dockerfile.release` per architecture through `TARGETARCH`).
  Docker and binary users then run byte-identical code.
- `cargo-chef` keeps local `docker build` caching fast.
- Published to **GHCR** as `linux/amd64` and `linux/arm64` by a GitHub Actions workflow on tags.
  `latest` tracks the newest release.

### 10.3 Running

```bash
# Standalone binary (any OS): serves UI + API on 0.0.0.0:8080, history in the user data dir
./speed-test-rs
./speed-test-rs --listen 0.0.0.0:9000 --udp-port 3478 --open
```


```bash
# Simplest: host networking (best for WebRTC; Linux only)
docker run -d --name speedtest --network host \
  -v speedtest-data:/data ghcr.io/puttyman/speed-test-rs:latest

# Bridge networking: publish both ports and advertise the host's LAN IP
docker run -d --name speedtest \
  -p 8080:8080 -p 3478:3478/udp \
  -e SPEEDTEST_PUBLIC_IP=192.168.1.10 \
  -v speedtest-data:/data ghcr.io/puttyman/speed-test-rs:latest
```

`docker-compose.yml` ships with both Docker variants commented.

The standalone binary doesn't have Docker's NAT issues: it binds to the host's interfaces
directly, so WebRTC host candidates work without `--public-ip`.

**Performance note for the docs**: Docker's userland proxy and bridge NAT can cap throughput
on slow hosts (e.g. a Raspberry Pi). For multi-gig testing, recommend `--network host` or the
standalone binary.
The server host also needs a NIC as fast as the link being tested. The README will say this
plainly: the test measures the slowest hop between the browser and the server.

## 11. Security and privacy

- Designed for a **trusted LAN**. No auth by default. Optional TLS (§9.2, §7.9) with a user-supplied certificate. The project never generates or installs CAs.
- The server **listens on all interfaces by default** (§9.2). The startup log says so in
  one line (`listening on all interfaces (0.0.0.0:8080, [::]:8080); use --listen 127.0.0.1:8080 to restrict`).
  The README tells laptop users on untrusted networks (cafés, hotels) to restrict it or stop the
  server.
- Optional `SPEEDTEST_AUTH_TOKEN`:
  - The UI asks for the token once and keeps it in a cookie.
  - The API accepts `Authorization: Bearer`.
  - WebSocket connections pass it in the first `hello` message, because browsers can't set
    headers on WebSocket connections.
- Abuse limits from §6.1 (max bytes, streams and sessions) stop one client from saturating the
  host forever. Each session has an idle timeout (30 s without messages).
- **No outbound network calls**: no telemetry, CDN fonts or analytics. Fonts are system stacks
  or self-hosted.
- Client IP and user agent are stored with each run to identify devices. This is documented,
  and `SPEEDTEST_STORE_CLIENT_INFO=false` turns it off.
- Never expose the service to the internet without the token and a reverse proxy with TLS.
  The README warns about this.

## 12. Testing strategy

| Layer | What | Tooling |
|---|---|---|
| Unit | `model::stats`, `model::scores` (table tests mirroring Cloudflare's thresholds), protocol serde round-trips, profile and ramp-halting logic | `cargo test` |
| Server integration | WebSocket download/upload correctness (byte counts, acks), limits and busy errors, HTTP `__down`/`__up`, run CRUD against a temp SQLite file | `cargo test --features ssr` with `tokio-tungstenite` as the client |
| WebRTC | str0m echo against a headless Chromium data channel | Playwright |
| End-to-end | Full `quick` profile in Chromium. Asserts all tiles populate, the run saves and appears in history. | `cargo leptos end-to-end` (Playwright) |
| UI and motion | Visual regression screenshots (light, dark, 360 px, reduced motion). Checks that the animation stays off the measurement path: a `quick` run with particles on and off gives results within 5%. | Playwright |
| PWA | Manifest is valid, service worker registers over HTTPS (mkcert certificate in CI), the app shell loads offline, and `/api` and `/ws` requests are never served from cache | Playwright |
| Multi-device | Simulated clients (tokio-tungstenite) for presence fan-out, group lifecycle, server byte-counter rates, fairness math (unit), overlap flagging; Playwright with 3 browser contexts for the panel and group flow | `cargo test --features ssr` + Playwright |
| Labels and history | First-run default label, last-label prefill, auto-save, chip selection, rename, compare deltas | Playwright + server integration |
| Accuracy | Run behind `tc netem` / `tc tbf` in a test container (e.g. 100 Mbit, 20 ms delay, 1% loss) and assert results within ±10% | A CI job (Linux runner, privileged) or a manual script |
| Binary smoke test | For each release target that can run on a CI runner (linux x86_64/arm64, macOS, Windows): copy the binary **alone** into an empty temp directory, start it, and check that `/`, `/api/health`, `/pkg/speed-test-rs.wasm` (`application/wasm`) and the CSS all return 200. Then run `healthcheck`. This catches missing embedded assets. | CI matrix |
| Lint | `cargo fmt --check`, `cargo clippy -D warnings` for both feature sets (and `ssr,embed-assets`) | CI |

## 13. Project structure (target)

```
src/
  main.rs              # server entry (ssr)
  lib.rs               # hydrate entry
  app.rs               # router, shell, layout
  pages/               # test.rs, history.rs, compare.rs, run.rs
  components/          # hero, metric_tile, quality_card, phase_timeline, label_picker,
                       #   units_toggle, charts/{box_plot,dot_plot,sparkline,live_area},
                       #   motion/{packet_flow (canvas), count_up, reveal}
  engine/              # (hydrate) client measurement engine: ws.rs, http.rs, rtc.rs, runner.rs
  model/               # (shared) protocol.rs, result.rs, profile.rs, stats.rs, scores.rs
  server/              # (ssr) api.rs, ws.rs, http_measure.rs, rtc.rs, storage.rs,
                       #       config.rs (clap), assets.rs (embedded /pkg + public), healthcheck.rs
build.rs               # checks target/site exists when `embed-assets` is on
migrations/            # sqlx migrations
contrib/               # systemd unit, launchd plist, Windows service notes
xtask/ or justfile     # `dist`: two-step build (§10.1) for each target
style/                 # tokens.scss, motion.scss, components/*.scss, main.scss
public/                # manifest.webmanifest, sw.js, icons/, fonts/ (self-hosted, OFL)
docs/design/tokens.md  # captured Unloan-inspired values + open font mapping
.github/workflows/     # ci.yml, release.yml
Dockerfile.release     # runtime image from prebuilt musl binaries
end2end/               # Playwright tests
docs/SPEC.md
Dockerfile, docker-compose.yml
```

## 14. Milestones

| # | Milestone | Scope | Exit criteria |
|---|---|---|---|
| **M0** | **Modernize** *(done in this change)* | Upgrade Leptos 0.6 → 0.8 and Axum 0.7 → 0.8. Drop Thaw. Rename the crate `leptops-axum` → `speed-test-rs`. Fix the broken build. Commit `Cargo.lock`. Add `/api/health`, a Cloudflare-style placeholder layout, the Dockerfile, compose, and the license. | `cargo leptos build` succeeds. The page hydrates with no console errors. The Docker image builds and serves. |
| M1 | WebSocket engine and design foundation | `model::{protocol,stats,profile}`. `/ws/probe` and `/ws/data`. Client engine for idle latency, jitter, download and upload with ramp halting. **Design tokens** (§7.2, Unloan-inspired values captured in `docs/design/tokens.md`), self-hosted open fonts, base components, live tiles with count-up, phase timeline, units toggle (§7.6). | The `quick` profile completes on localhost and over Wi-Fi with plausible numbers. Unit and integration tests pass. The UI uses tokens only (no raw colors in components). |
| M2 | Loaded latency, scores, charts, motion | Probe during load. AIM scores. Box plot, dot plot, sparkline, live area chart. Detail section. **Motion system** (§7.3): packet-flow canvas, result reveal, micro-interactions, reduced-motion mode, performance guard. Dark and light theme toggle. | Scoring unit tests match the Cloudflare SDK. 60 fps on a mid-range phone. A `quick` run with animation on and off gives results within 5%. |
| M3 | HTTP mode | `__down`/`__up`, the client HTTP engine, the transport toggle | WebSocket and HTTP results within 10% of each other on a wired LAN |
| M4 | Packet loss (WebRTC) | str0m endpoint on a single UDP port, signalling over `/ws/control`, client DataChannel engine, a graceful "unavailable" state | netem 1% loss is measured as 1% ± 0.5%. Works with `--network host` and with bridge mode plus `SPEEDTEST_PUBLIC_IP`. |
| M5 | History, labels and compare | SQLite storage, run CRUD API, **auto-save**, **prefilled labels** (last-used, or `<device> · <date>`), label chips, device ID and name, `/history` grouped by label, `/compare` with deltas, `/runs/{id}`, export, retention, view transitions | Runs persist across restarts. A first-time user can run, save and compare two rooms **without typing anything**. Comparing 3 labels works on mobile. |
| M6 | Multi-device live view and group test | Always-on `/ws/control` presence, server-side per-session byte counters and a 4 Hz `Peers` broadcast, Devices panel, overlap flagging and the Wait / Test anyway prompt, group create/join/start with the time-based `concurrent` profile, aggregate, shares and Jain fairness, server-bottleneck warning, `groups` table, Hide me, `--no-presence` | 3 browsers (2 headless + 1 real) see each other within 1 s. A group test of 3 clients against a `tc tbf`-limited link gives an aggregate within 10% of the limit and fairness ≥ 0.9. Overlap flags appear on solo runs that overlap. |
| M7 | Standalone binary | `embed-assets` feature with `rust-embed` and the asset handler (MIME types, compression, cache headers). `clap` CLI and §9 options. Platform data dirs. LAN URL and QR printout. `--open`. `dist` recipe. `contrib/` service files. Distroless Docker runtime. | The binary, copied **alone** into an empty directory on Linux, macOS and Windows, serves a fully hydrated, styled UI. The binary smoke test (§12) passes. |
| M8 | PWA and HTTPS | Manifest, icons, service worker (app shell only, never `/api` or `/ws`), update toast, iOS meta tags. Optional `rustls` TLS listener (`--tls-*`). README guides for mkcert and reverse proxies. | Installs on Android Chrome and desktop Chrome/Edge over HTTPS. Adds to the iOS home screen. The offline shell shows the friendly offline page. |
| M9 | Release | Hand-written GitHub Actions `release.yml` (§10.1) producing all targets with `SHA256SUMS` and a multi-arch GHCR image from the same binaries. `ci.yml` (fmt, clippy, tests, e2e, smoke). README with screenshots and install instructions for both. CONTRIBUTING. v0.1.0 tag. | Pushing tag `v0.1.0` publishes a GitHub Release whose binaries run on every target, and `docker run ghcr.io/puttyman/speed-test-rs:0.1.0` works on amd64 and arm64. |

### 14.1 Implementation status (2026-10-10)

M0–M8 are implemented on `claude/hopeful-heisenberg-fvdq5d`. M9 (release) has its workflows
written; they haven't run on GitHub yet.

**Verified:**
- 205 Rust tests: 184 unit, 15 measurement integration, 6 presence and group integration.
- 14 Playwright tests in Chromium against the single release binary, run alone with no
  `LEPTOS_*` env and no site folder: test runs over WebSocket and HTTP, auto-save and rename,
  units and theme, history, compare, run page, and a 3-device Test-together flow.
- Smoke test of the embedded binary.
- x86_64, aarch64 and armv7 musl builds. Each passes the smoke test and a full WebRTC run
  (the ARM ones under qemu).
- `Dockerfile.release` in bridge and host networking modes.
- Loopback throughput: the server handles 13.5 Gbps (WebSocket download) and 11 Gbps
  (upload). Chromium's WebSocket stack caps a single page at about 0.5–0.9 Gbps here.
- Packet loss: 0/1000 on a clean link. With 2% loss injected by a UDP relay, the mean of
  five runs measured 2.0%.

**Known gaps and deviations:**

| Item | State |
|---|---|
| macOS/Windows release builds, Zig and qemu on GitHub runners, a full local `Dockerfile` build | Not run yet (no network for containers or macOS/Windows hosts in development) |
| `tc netem` accuracy test (§12) | Not possible in the dev sandbox (no netem in the kernel). Loss was validated with a userspace relay instead. |
| IPv6 listening | Code path exists. Untested (no IPv6 in the dev sandbox). |
| Binary size | 23.9 MB (x86_64 musl, stripped, fat LTO) vs the 10–15 MB estimate. Two crypto stacks are linked: ring for rustls TLS and aws-lc for str0m DTLS. Moving rustls to aws-lc-rs would remove one. |
| WASM size | 1.85 MB raw / about 485 KB Brotli (erased components, `wasm-opt -Oz`) |
| `hash-files` | Off. Leptos 0.8 needs `hash.txt` next to the binary, which conflicts with single-file distribution. Assets use ETag revalidation instead. |
| Peer `latency_ms` in presence | Always `None` (no client→server live latency report yet) |
| Leptos "erased components" | On for every build (`.cargo/config.toml` rustflags plus `always-erase-components`). Server and WASM must agree. A `RUSTFLAGS` env var overrides it, so `build.rs` guards the embed build. |

## 15. Future ideas (post-v1)

- A headless CLI client (`speed-test-rs client http://server:8080`) for scripted or cron
  tests from devices without a browser, sharing `model` and the engine logic.
- Scheduled tests and a Prometheus `/metrics` endpoint for Grafana dashboards.
- Wi-Fi metadata hints (SSID, band) where the browser or OS exposes them, or entered manually.
- An optional public "internet mode" that points the engine at another speed-test-rs instance.
- Peer-to-peer testing between two browsers through the server as a signalling relay.
- **Floor-plan heatmap**: upload a floor plan and pin each label to a spot to get a Wi-Fi heatmap.
- **Walk-test mode**: a light test repeats every few seconds while you walk, showing dead
  zones and roaming hand-offs.
- **Bufferbloat grade (A–F)** and **plain-language verdicts** ("Good for 4K on 2 TVs and a call").
- **Shareable result card** (PNG) with a QR code.
- **Home Assistant / MQTT** export and **alerts** when a label falls below its baseline.
- **TV / kiosk mode** showing the latest result per label.
- A "real-time load" phase: an unreliable WebRTC data channel sends at fixed target rates
  (e.g. 5 / 25 / 100 Mbps) and records loss and jitter at each rate, like `iperf3 -u`. It is
  a diagnostic for video calls and gaming, not a throughput measurement.

## 16. Open questions

1. **Parallel streams default**: *proposal*: 1 stream for `standard` (Cloudflare-like,
   reflects single-flow behavior), 4 for `extended`.

### Decided

| Topic | Decision |
|---|---|
| Throughput transport | WebSocket, with HTTP as a cross-check. WebRTC is used only for packet loss. |
| Bind address | All interfaces (`0.0.0.0` + `[::]`) by default |
| Release tooling | Hand-written GitHub Actions (§10.1) |
| Labels | Free text with chips, auto-saved and prefilled (§7.7) |
| Units | Mbps by default, with a toggle for MB/s and auto-scaling to Gbps / GB/s (§7.6) |
| Visual style | Unloan-inspired color, fonts and look and feel. Light-first, with a derived dark theme. IBM Plex Mono + Inter (OFL). Our own copy (§7.1, §7.2, §7.5). |
| Multi-device | Live presence over `/ws/control`, synchronized group tests, overlap flagging (§5.11, §7.11) |
| Dependencies | Latest stable releases. Leptos 0.9 adopted once stable (§4.1). |
| Motion | Packet flow, count-up, live chart, phase timeline, result reveal, view transitions (§7.3) |
| PWA | Yes. Full install and offline support over HTTPS (§7.9). |
| WebRTC library | **str0m** (ICE-lite, one UDP port). In testing, it handled 1000 unreliable messages at 10 per 10 ms with 0 loss and no head-of-line blocking, and measured 2.0% with 2% injected loss. |
