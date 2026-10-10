# speed-test-rs — Product & Technical Specification

| | |
|---|---|
| **Status** | Draft v0.1 |
| **Last updated** | 2026-10-10 (v0.2: standalone binary distribution) |
| **Stack** | Rust · Leptos 0.8 (SSR + hydration/WASM) · Axum 0.8 · Tokio · WebSocket · WebRTC (str0m) · SQLite |
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

1. **Look and feel close to speed.cloudflare.com**: a big headline number, metric tiles, a
   network quality score, detailed per-phase charts, and live updates while the test runs.
2. **Accurate at LAN speeds**: correct from about 1 Mbps up to at least 2.5 Gbps on one
   stream, and up to 10 Gbps with parallel streams on capable hardware.
3. **WebSocket-first measurement**: latency, download, upload and loaded latency all run over
   WebSocket.
4. **True packet loss** over UDP via a WebRTC data channel (unreliable and unordered).
5. **History and comparison**: runs are saved in SQLite, each with an optional label
   (e.g. "Office – AP-2 – 5 GHz"). A history page lists runs and compares them side by side.
6. **One-command deploy, two ways**:
   - **Standalone binary**: one executable per platform with the Leptos UI (HTML shell, WASM,
     JS, CSS, icons) embedded. `./speed-test-rs` starts the server and serves the UI, with no
     `site/` folder, runtime, or installer needed. Builds for Linux (x86_64, aarch64, armv7),
     macOS (Intel, Apple Silicon) and Windows (x86_64). See §10.1.
   - **Docker**: `docker run -p 8080:8080 -p 3478:3478/udp ghcr.io/…/speed-test-rs`. The image
     wraps the same Linux binary and targets amd64 and arm64 (Raspberry Pi, NAS boxes).
7. **Private by default**: no telemetry and no external calls. All assets are served locally.
8. **Open source and easy to contribute to**: clear module boundaries, tests, CI.

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
| WebRTC | **`str0m`** (sans-IO WebRTC). Alternative: `webrtc` (webrtc-rs). | Only data channels are needed. str0m is small, actively maintained, and runs on one UDP socket, which keeps Docker port mapping simple. |
| Storage | **SQLite via `sqlx`** (compile-time-checked queries, embedded migrations), with the `sqlite` bundled build | Zero-ops, a single file in a Docker volume or the user's data directory. Bundled SQLite means no system library is needed, so the binary stays self-contained. |
| Static assets | **Embedded in the server binary** with `rust-embed` behind an `embed-assets` feature (on for release builds). Dev builds read `target/site` from disk. | Single-file distribution (§10.1). Dev keeps `cargo leptos watch` hot reload. |
| CLI | **`clap`** (derive) with env-var fallbacks | One parser for binary users (flags) and Docker users (env vars). |
| Shared types | `src/model/` module compiled for both `ssr` and `hydrate` | Protocol messages, result structs and AIM scoring live in one place. |

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
   Docker bridge networking this variable must be set to the host's LAN IP (§10.2).
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

## 6. Protocols and APIs

### 6.1 WebSocket endpoints

| Path | Frames | Purpose |
|---|---|---|
| `/ws/control` | JSON text | Session start and stop, progress events, WebRTC signalling |
| `/ws/data` | JSON text (requests, acks) + binary (payload) | Download and upload transfers. Several may be open for parallel streams. |
| `/ws/probe` | JSON text | Ping/pong for idle and loaded latency |

Every connection opens with `{"t":"hello","session":"<uuid>","role":"control|data|probe"}`, so
the server can group sockets into one test session.

All JSON messages are tagged by `"t"`. They are defined once as `#[serde(tag = "t")]` enums in
`model::protocol` and shared by client and server:

```rust
enum ClientMsg {
    Hello { session: Uuid, role: Role },
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
    Error { code: ErrorCode, message: String },
}
```

**Server limits**, advertised in `Welcome` and enforced:
- `max_bytes_per_request` (default 2 GB)
- `max_streams` (8)
- `max_concurrent_sessions` (default 4). Beyond that, the server returns `Error{code:Busy}` and
  the UI shows "Server busy, another test is running". This stops concurrent tests from
  skewing each other.

### 6.2 HTTP API

| Method & path | Description |
|---|---|
| `GET /api/health` | `{"status":"ok","version":"x.y.z"}`. Used by the Docker HEALTHCHECK. *(Implemented.)* |
| `GET /api/info` | Server name, version, limits, client IP as seen by the server, configured UDP port |
| `GET /api/__down?bytes=N` | HTTP-mode download (§5.8) |
| `POST /api/__up` | HTTP-mode upload (§5.8) |
| `POST /api/runs` | Save a completed run (body: `RunResult`). Returns `{id}`. |
| `GET /api/runs?label=&from=&to=&limit=&cursor=` | List runs, newest first, cursor-paginated |
| `GET /api/runs/{id}` | Full run, including raw points |
| `PATCH /api/runs/{id}` | Update the label or notes |
| `DELETE /api/runs/{id}` | Delete a run |
| `GET /api/labels` | Distinct labels with run counts and the latest run per label |
| `GET /api/runs/export?format=csv\|json` | Export history |

Leptos server functions can wrap the run endpoints for the UI, but the REST shape above is the
public contract, so scripts can use it too.

### 6.3 Result model (`model::result::RunResult`)

```text
RunResult {
  id, created_at, label?, notes?,
  transport: "websocket" | "http", profile, streams,
  client: { ip, user_agent, device_hint? },
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

### 7.1 Visual language

The look follows speed.cloudflare.com without copying its branding: no Cloudflare logo, name,
or trade dress.

- Light, airy layout with white cards on a light grey page, and a dark theme through
  `prefers-color-scheme` (plus a manual toggle).
- **Hero**: a large thin-weight headline number (current or final download, Mbps) and a
  **Start / Stop / Retest** pill button in the accent color.
- **Metric tiles**: Download, Upload, Latency, Jitter, Packet loss. Each has a colored top
  rule and a large tabular-nums value with its unit. Each tile updates live while its phase runs.
- **Network quality score** card with Video streaming, Online gaming and Video chatting, each
  showing a colored badge (bad / poor / average / good / great).
- **Server / connection** card with server name, version, client IP, transport, streams and profile.
- **Detail section** under the hero, one card per metric:
  - Latency: box plot of idle, download-loaded and upload-loaded RTTs, with jitter values.
  - Download / Upload: a dot plot of every sample per payload size (100 kB … 1 GB), with the
    p90 line. Same idea as Cloudflare's per-size measurement chart.
  - Packet loss: sent / received / lost, as a bar.
- **Live progress**: a thin progress bar under the header, a phase label ("Measuring upload,
  10 MB × 4"), and a sparkline of instantaneous throughput in the hero.
- **Label prompt**: after a run, an inline "Where are you testing from?" field (autocompletes
  previous labels), with a Save button.

Design tokens (`style/main.scss`):

| Token | Light | Use |
|---|---|---|
| `--accent` | `#f6821f` | Start button, highlights |
| `--download` | `#7b5cd6` | Download series |
| `--upload` | `#e8478b` | Upload series |
| `--latency` | `#2c7cb0` | Latency series |
| `--great … --bad` | green → red ramp | Quality badges |

### 7.2 Pages

| Route | Content |
|---|---|
| `/` | Test page. The test does **not** auto-start, so users can set the label, transport and streams first. `?autostart=1` starts it on load. |
| `/history` | Runs table: date, label, download, upload, latency, loss, scores. Filter by label or date. Multi-select to compare. |
| `/compare?ids=a,b,c` | Side-by-side tiles plus overlaid charts. A per-label summary shows the median of that label's last N runs. |
| `/runs/{id}` | A read-only result page with the same layout as the finished test page. Shareable. |
| `/settings` (drawer) | Transport (WebSocket/HTTP), profile, parallel streams, units (Mbps/MB/s), theme |

### 7.3 Accessibility and responsiveness

- Works at 360 px width with a 16 px gutter and no horizontal scroll. Tiles reflow to 2
  columns, then 1.
- Every chart has a text alternative (a values table in `<details>`).
- Colors meet WCAG AA contrast, and badges also show the class name as text.
- Live values are announced politely (`aria-live="polite"`), throttled to phase completion.

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
  download_bps  REAL, upload_bps REAL,
  latency_ms    REAL, jitter_ms  REAL,
  down_loaded_latency_ms REAL, up_loaded_latency_ms REAL,
  packet_loss_ratio REAL,
  score_streaming TEXT, score_gaming TEXT, score_rtc TEXT,
  duration_ms   INTEGER NOT NULL,
  detail_json   TEXT NOT NULL              -- full RunResult (points, phases)
);
CREATE INDEX runs_created_at ON runs(created_at DESC);
CREATE INDEX runs_label      ON runs(label, created_at DESC);
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
| `-l, --listen <ADDR>` | `SPEEDTEST_LISTEN` (alias `LEPTOS_SITE_ADDR`) | `0.0.0.0:8080` | HTTP/WS listen address. `cargo leptos watch` uses `127.0.0.1:3000`. |
| `--udp-port <PORT>` | `SPEEDTEST_UDP_PORT` | `3478` | WebRTC UDP port |
| `--public-ip <IP>` | `SPEEDTEST_PUBLIC_IP` | auto | IP advertised in ICE candidates. Set it under Docker bridge networking. |
| `--server-name <NAME>` | `SPEEDTEST_SERVER_NAME` | hostname | Shown in the UI and the history |
| `--data-dir <DIR>` | `SPEEDTEST_DATA_DIR` | see §9.3 | Directory for the database |
| `--db-path <FILE>` | `SPEEDTEST_DB_PATH` | `<data-dir>/speedtest.db` | SQLite file (overrides `--data-dir`) |
| `--no-history` | `SPEEDTEST_HISTORY=false` | history on | Turns run persistence off |
| `--retention-days <N>` | `SPEEDTEST_RETENTION_DAYS` | – | Auto-prune age |
| `--max-sessions <N>` | `SPEEDTEST_MAX_SESSIONS` | `4` | Concurrent test sessions |
| `--max-bytes <N>` | `SPEEDTEST_MAX_BYTES` | `2000000000` | Per-request byte cap |
| – | `SPEEDTEST_AUTH_TOKEN` | – | If set, the API, WebSocket and history require it (§11). Env only, so it doesn't show in `ps`. |
| `--open` | – | off | Open the UI in the default browser after startup (desktop use) |
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
`SHA256SUMS` file, attached to the GitHub Release. `cargo-dist` is the preferred tool for
generating the release workflow and an install script
(`curl --proto '=https' -sSf https://…/install.sh | sh`). It is evaluated during M6.

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
  - **Build stage**: `rust:1-bookworm` with the `wasm32-unknown-unknown` target and a
    pinned `cargo-leptos` release binary. It runs the same two-step build as §10.1, with
    `embed-assets` enabled.
  - **Runtime stage**: today this is `debian:bookworm-slim` with a non-root user, the server
    binary and `site/`. *(Implemented in Milestone 0.)* Once assets are embedded (M6), it
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

- Designed for a **trusted LAN**. No auth by default.
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
  components/          # hero, metric_tile, quality_card, charts/{box_plot,dot_plot,sparkline}
  engine/              # (hydrate) client measurement engine: ws.rs, http.rs, rtc.rs, runner.rs
  model/               # (shared) protocol.rs, result.rs, profile.rs, stats.rs, scores.rs
  server/              # (ssr) api.rs, ws.rs, http_measure.rs, rtc.rs, storage.rs,
                       #       config.rs (clap), assets.rs (embedded /pkg + public), healthcheck.rs
build.rs               # checks target/site exists when `embed-assets` is on
migrations/            # sqlx migrations
contrib/               # systemd unit, launchd plist, Windows service notes
xtask/ or justfile     # `dist`: two-step build (§10.1) for each target
style/main.scss
end2end/               # Playwright tests
docs/SPEC.md
Dockerfile, docker-compose.yml
```

## 14. Milestones

| # | Milestone | Scope | Exit criteria |
|---|---|---|---|
| **M0** | **Modernize** *(done in this change)* | Upgrade Leptos 0.6 → 0.8 and Axum 0.7 → 0.8. Drop Thaw. Rename the crate `leptops-axum` → `speed-test-rs`. Fix the broken build. Commit `Cargo.lock`. Add `/api/health`, a Cloudflare-style placeholder layout, the Dockerfile, compose, and the license. | `cargo leptos build` succeeds. The page hydrates with no console errors. The Docker image builds and serves. |
| M1 | WebSocket engine | `model::{protocol,stats,profile}`. `/ws/probe` and `/ws/data`. Client engine for idle latency, jitter, download and upload with ramp halting. Live tiles and progress. | The `quick` profile completes on localhost and over Wi-Fi with plausible numbers. Unit and integration tests pass. |
| M2 | Loaded latency, scores, charts | Probe during load. AIM scores. Box plot, dot plot, sparkline. Detail section. Dark mode toggle. | Matches the Cloudflare SDK's result shape for the same inputs (scoring unit tests). |
| M3 | HTTP mode | `__down`/`__up`, the client HTTP engine, the transport toggle | WebSocket and HTTP results within 10% of each other on a wired LAN |
| M4 | Packet loss (WebRTC) | str0m endpoint on a single UDP port, signalling over `/ws/control`, client DataChannel engine, a graceful "unavailable" state | netem 1% loss is measured as 1% ± 0.5%. Works with `--network host` and with bridge mode plus `SPEEDTEST_PUBLIC_IP`. |
| M5 | History and compare | SQLite storage, run CRUD API, label prompt, `/history`, `/compare`, `/runs/{id}`, export, retention | Runs persist across container restarts (volume). Comparing 3 labelled runs works on mobile. |
| M6 | Standalone binary | `embed-assets` feature with `rust-embed` and the asset handler (MIME types, compression, cache headers). `clap` CLI and §9 options. Platform data dirs. LAN URL and QR printout. `--open`. `dist` recipe. `contrib/` service files. Distroless Docker runtime. | The binary, copied **alone** into an empty directory on Linux, macOS and Windows, serves a fully hydrated, styled UI. The binary smoke test (§12) passes. |
| M7 | Release | Release workflow (cargo-dist or hand-written) producing the §10.1 targets with `SHA256SUMS`. Multi-arch GHCR image built from the same binaries. CI (fmt, clippy, tests, e2e, smoke). README with screenshots and install instructions for both. CONTRIBUTING. v0.1.0 tag. | Downloading `speed-test-rs-0.1.0-<target>` from GitHub Releases and running it works. `docker run ghcr.io/puttyman/speed-test-rs:0.1.0` works on amd64 and arm64. |

## 15. Future ideas (post-v1)

- A headless CLI client (`speed-test-rs client http://server:8080`) for scripted or cron
  tests from devices without a browser, sharing `model` and the engine logic.
- Scheduled tests and a Prometheus `/metrics` endpoint for Grafana dashboards.
- Wi-Fi metadata hints (SSID, band) where the browser or OS exposes them, or entered manually.
- An optional public "internet mode" that points the engine at another speed-test-rs instance.
- Peer-to-peer testing between two browsers through the server as a signalling relay.

## 16. Open questions

1. **Parallel streams default**: keep 1 (Cloudflare-like, reflects single-flow behavior), or
   default to 4 on the `extended` profile only? *Current proposal: 1 for standard, 4 for extended.*
2. **str0m vs webrtc-rs**: str0m is preferred for its single-socket, sans-IO design. Confirm
   during M4 that str0m's SCTP handles 1000 small unreliable messages without head-of-line
   effects.
3. **Labels as free text or managed entities**: free text with autocomplete is proposed.
   Managed "locations" (with icons and an AP model) could come later.
4. **Units**: default to Mbps (like Cloudflare), with a toggle for MB/s?
5. **Default bind address for the binary**: `0.0.0.0:8080` is proposed, since the tool is
   useless if phones on the LAN can't reach it. The alternative is `127.0.0.1` with a
   `--lan` flag, which is safer on a laptop on public Wi-Fi but adds a step every time.
6. **Release tooling**: `cargo-dist` (generated workflow, installers, less to maintain) or a
   hand-written GitHub Actions matrix (full control over the two-step leptos build)?
   Confirm during M7 that cargo-dist can run the `cargo leptos build` pre-step.
7. **WebRTC as a throughput transport**: should a third, experimental transport measure
   download and upload over the WebRTC data channel (UDP/SCTP), alongside WebSocket and HTTP?
   *Current proposal*: no for the headline numbers. Browser SCTP stacks are CPU-bound and
   implement their own congestion control, so the result reflects the browser's data channel
   implementation more than the link, and it is expected to fall short of WebSocket on fast
   LANs. Instead, consider a later **"real-time load" phase**: an unreliable data channel
   sends at fixed target rates (e.g. 5 / 25 / 100 Mbps) and records loss and jitter at each
   rate, similar to `iperf3 -u`. That answers "can this AP carry a video call or game
   stream?", which TCP-based tests can't.
