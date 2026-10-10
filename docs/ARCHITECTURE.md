# Architecture

This is a map of the code for contributors. The design and its reasons are in
[`SPEC.md`](SPEC.md); section numbers below (§) refer to it.

## One crate, two builds

speed-test-rs is a single Leptos crate compiled twice from the same source:

| Build | Features | Target | Output |
|---|---|---|---|
| Server | `ssr` (+ `embed-assets` for the standalone binary) | host / release triple | `speed-test-rs` binary: Axum, server rendering, measurement endpoints, SQLite |
| Front end | `hydrate` | `wasm32-unknown-unknown` (`cdylib`, `wasm-release` profile) | `target/site/pkg/speed-test-rs.{wasm,js,css}`: hydrates the server-rendered page and runs the measurement engine |

cargo-leptos drives both (`cargo leptos watch`, `cargo leptos build`). The standalone binary
is built in two steps (§10.1): `cargo leptos build --release --frontend-only` writes
`target/site`, then `cargo build --release --features ssr,embed-assets` lets `build.rs`
pre-compress the site (Brotli and gzip) and `rust-embed` compiles it in.

Both builds use Leptos' erased-components mode (`--cfg erase_components`, set in
`.cargo/config.toml` and by cargo-leptos through `always-erase-components`). Server and
front end must agree on it, or hydration fails; `build.rs` checks it for `embed-assets`.

## Module map

```text
src/
  main.rs          server entry: CLI, startup, listeners (ssr)
  lib.rs           WASM entry: hydrate() (hydrate)
  app.rs           HTML shell, router, header
  ui_state.rs      app-wide UI state persisted in localStorage (units, theme, profile, device)
  model/           shared, platform-free logic (compiled into both builds, unit tested natively)
  engine/          browser measurement engine (hydrate; util.rs is platform-free)
  pages/           routed pages and the small REST client
  components/      UI building blocks: tiles, charts, motion, drawers, toggles
  server/          everything server-side (ssr)
build.rs           embed-assets: checks, compresses and locates target/site
migrations/        sqlx migrations (embedded)
public/            static files copied into the site: manifest, service worker, icons, fonts
style/             SCSS (tokens, components) → pkg/speed-test-rs.css
end2end/           Playwright tests
contrib/           systemd unit, launchd agent, Windows notes
```

### `model/` (shared)

| Module | Contents |
|---|---|
| `protocol` | Every WebSocket message (`ClientMsg`, `ServerMsg`), tagged by `"t"` (§6.1), plus `Limits`, `ErrorCode`, `Peer`, group types |
| `profile` | Test profiles as data (`quick`, `standard`, `extended`, `concurrent`), `Step`, and `RampState`, the deterministic ramp planner (§5.3) |
| `stats` | Percentiles, median, jitter, NaN-safe helpers (§5.10) |
| `result` | `RunResult` and the reducers that turn raw points into a `Summary`; group results, shares and Jain's fairness (§5.11, §6.3) |
| `scores` | Cloudflare AIM scoring (§5.9) |
| `history` | Response shapes of the history REST API |
| `device`, `units` | Device names and default labels; Mbps/MB/s formatting |

### `engine/` (browser)

| Module | Contents |
|---|---|
| `mod.rs` | `engine::start(options, callback)` → `EngineHandle`; the event stream (`PhaseStarted`, `Bandwidth`, `Live`, `Finished`, …) |
| `runner` | Walks a `Profile` with `RampState`, collects measurements, builds the `RunResult` |
| `socket` | Async wrapper around `web_sys::WebSocket` (zero-copy byte counting) |
| `ws_transport` | Ping/pong latency, downloads timed by the client, uploads timed by the server, parallel streams |
| `http_transport` | Cloudflare-style `__down`/`__up` timed with `PerformanceResourceTiming` |
| `loaded_latency` | Probes and the live meter that run alongside a transfer |
| `rtc` | WebRTC packet-loss probe: unordered, no-retransmit data channel, LAN host candidates only |
| `presence` | The page-wide `/ws/control` client: peers, invites, group tests |

### `server/`

| Module | Contents |
|---|---|
| `config` | clap flags + environment + defaults → `Config` (§9) |
| `startup`, `listen` | Logging, banner with LAN URLs and QR code; HTTP and optional rustls listeners, dual-stack, graceful shutdown |
| `router` | Axum router: API routes, Leptos SSR routes, asset fallback, auth and cache middleware |
| `assets` | `/pkg/*` and `public/` files from the embedded copy or `--site-root`: MIME types, pre-compressed variants, ETags, cache headers |
| `ws` | `/ws/control`, `/ws/data`, `/ws/probe` handlers (§6.1) |
| `http_measure` | `/api/info`, `/api/__down`, `/api/__up`, the auth-token middleware |
| `sessions` | Session registry: sockets per session, byte counters and live rates, overlap log, the `max_sessions` guard |
| `presence` | Builds and broadcasts `Peers` snapshots (≤ 4 Hz) |
| `groups` | Group test lifecycle and server-side group results |
| `rtc`, `rtc_hook` | str0m WebRTC echo endpoint on one UDP port; signalling hook used by the control socket |
| `storage`, `runs_api` | SQLite (WAL, sqlx) and the history REST API: runs, labels, compare, groups, export, retention |
| `healthcheck` | The `healthcheck` subcommand used by Docker |

### `pages/` and `components/`

`pages/test.rs` is the main page: hero with the packet-flow canvas and phase timeline, label
picker and Start button, live chart, metric tiles, network quality and server cards, the
Devices panel, and the detail charts after a run. It starts the engine, turns engine events
into signals, and saves finished runs. The other pages:

| Page | Route | Contents |
|---|---|---|
| `pages/history.rs` | `/history` | Saved runs grouped by label, filters, rename, delete, export, selection to compare; group tests as one expandable row |
| `pages/compare.rs` | `/compare?labels=…` or `?ids=…` | Two to four labels (median of the last 5 runs) or runs side by side, with deltas |
| `pages/run.rs` | `/runs/{id}` | A read-only, shareable result page |
| `pages/api.rs` | – | The JSON client for the REST API |

Notable components: `presence` (the page-wide presence context: one `/ws/control` client per
page, provided by `App`), `devices_panel` and `group_view` (Devices panel and the
"Test together" countdown, live lanes and result), and `result_view` (pieces shared by the
test and run pages). Components only read signals and tokens (`style/`); they never measure
anything themselves, so the animation stays off the measurement path.

## Life of a test

1. **Page load.** The server renders the page (SSR) and the browser hydrates it with the WASM
   bundle. `engine::presence` opens the long-lived `/ws/control` socket with
   `Hello{session, role: control, device}` and starts receiving `Peers` snapshots.
2. **Start.** The test page calls `engine::start` with the chosen transport, profile and
   streams, under the presence session ID. The engine opens its own control socket (status
   updates and WebRTC signalling), a `/ws/probe` socket and one or more `/ws/data` sockets,
   each starting with `Hello`. The server checks the token and `max_sessions` and answers
   `Welcome{limits}`.
3. **Steps.** The runner walks the profile:
   - *Latency*: `Ping{seq}` → `Pong{seq}` on the probe socket, timed with
     `performance.now()`.
   - *Download*: `Download{id, bytes}` → the server streams random 64 KiB binary frames and
     `DownloadDone{id, server_ms}`; the client times it.
   - *Upload*: `Upload{id, bytes}` + binary frames → `UploadDone{id, bytes, server_ms}`; the
     server's timing is used.
   - While transfers run, loaded-latency probes go over the probe socket.
   - `RampState` stops a size early once it takes long enough (§5.3).
   - *Packet loss*: the engine creates an `RTCPeerConnection`, sends `RtcOffer{sdp}` on its
     control socket, gets `RtcAnswer` from the str0m endpoint (`server::rtc`), and sends
     numbered messages over UDP; the server echoes each one and the client counts what
     comes back.
   - In HTTP mode, `GET /api/__down` and `POST /api/__up` replace the data socket.
4. **Live view.** The server counts every byte on the session's sockets (`sessions`), so the
   Devices panel on other browsers shows this device's speed from server-side counters.
5. **Finish.** The engine reduces the points into a `Summary` (`model::result`) and AIM
   scores (`model::scores`) and emits `Finished(RunResult)`. The page saves it with
   `POST /api/runs`; the server adds the overlap with other sessions (`peers_max`,
   `overlap_ratio`) and the client IP and User-Agent (unless turned off), and stores it.

## Presence and group tests

- **Presence** (`server::presence`): one `Peer` per session with a control socket. Name and
  kind come from the control `Hello`, label and phase from the last `Status`, and live rates
  from the registry's 250 ms samples. A snapshot goes to every control socket at most 4 times
  a second, and only when something changed. Hidden devices are left out of other lists and
  only counted.
- **Groups** (`server::groups`): `GroupCreate` → invites to every other control socket →
  `GroupJoin` → the host's `GroupStart` sends every member
  `GroupStart{starts_in_ms: 3000, plan: concurrent}`. All members run the time-based
  `concurrent` profile (3 s latency, 10 s download, 2 s pause, 10 s upload). The server
  measures each member from its own byte counters over aligned windows, waits briefly for the
  members' posted runs (for loaded latency), computes the aggregate, shares, Jain's fairness
  and the server-bottleneck flag, sends `GroupResult` and stores the group.
- **Overlap** (`server::sessions`): every session logs the spans in which it moved data. When
  a solo run is saved, its overlap with other sessions becomes `peers_max` and
  `overlap_ratio`.

## Storage

SQLite through sqlx with the bundled library, in WAL mode with foreign keys on. Migrations in
`migrations/` are embedded with `sqlx::migrate!` and run at startup. `runs` holds the
denormalized summary columns (fast history and compare queries) and the full `RunResult` in
`detail_json`; `groups` holds group results. Queries are checked at runtime, so building needs
no database. Retention, when set, prunes old runs daily. With `--no-history` there is no
database: the history endpoints answer 404, and `POST /api/runs` validates the run and
answers 202 without storing it, so clients can always post.

## Assets and caching

With `embed-assets`, `build.rs` stores each compressible file with Brotli and gzip variants
(kept only when at least 10% smaller), and `server::assets` serves the best one for
`Accept-Encoding`, with `Content-Type` from the extension and an ETag from the embedded hash.
Content-hashed file names get `immutable` caching; everything else, HTML and `sw.js`
included, is `no-cache` and revalidates cheaply with the ETag. The service worker
(`public/sw.js`) caches the app shell only and never touches `/api/*` or `/ws/*`.

## Builds and releases

- `ci.yml`: fmt, clippy for `ssr`, `hydrate` (wasm32) and `ssr,embed-assets`, tests,
  Playwright, and a static x86_64 musl build run through `scripts/smoke-binary.sh`.
- `release.yml` (tags `v*`): builds the site once, then the server for every target with the
  site embedded (Linux musl targets with `cargo-zigbuild`), smoke-tests the binaries the
  runner can execute, packages archives with `SHA256SUMS`, and builds the multi-arch Docker
  image from the same musl binaries (`Dockerfile.release`).
- C code: SQLite (`libsqlite3-sys`, bundled) and aws-lc (`aws-lc-sys`, through str0m's DTLS)
  are compiled from source for each target. Zig is the C cross compiler for the musl targets;
  macOS and Windows use the runner's native toolchain (on Windows, aws-lc uses its bundled
  pre-assembled NASM objects).
