# speed-test-rs: full guide

The short version is in the [README](../README.md).

A self-hosted, [Cloudflare-style](https://speed.cloudflare.com) speed test for your **local
network**. Run one small binary (or a Docker container) on a machine at home or in the office,
open it from any phone or laptop, and measure the link between them: download, upload,
latency, jitter, latency under load and packet loss.

Built in Rust with [Leptos](https://github.com/leptos-rs/leptos) (server rendering plus a
WebAssembly front end), [Axum](https://github.com/tokio-rs/axum),
[str0m](https://github.com/algesten/str0m) for WebRTC and SQLite for history. Everything,
the UI included, is compiled into one static binary. Nothing leaves your network: there is
no telemetry, no CDN and no external fonts.

The full design is in [`SPEC.md`](SPEC.md).

## Contents

- [Features](#features)
- [Quick start: binary](#quick-start-binary)
- [Quick start: Docker](#quick-start-docker)
- [Configuration](#configuration)
- [HTTPS and installing the app (PWA)](#https-and-installing-the-app-pwa)
- [Getting accurate results](#getting-accurate-results)
- [Security](#security)
- [Development](#development)
- [Architecture](#architecture)
- [License](#license)

## Features

- **WebSocket engine.** Download and upload run over WebSockets with Cloudflare's size ramp
  (100 kB up to 1 GB transfers, stopping early once a size takes long enough), up to 8
  parallel streams, and percentiles rather than averages. Choose the `quick`, `standard` or
  `extended` (4 streams for the large sizes) profile in the settings. WebSockets avoid per-request overhead, so
  fast LANs are not capped by HTTP request rates.
- **HTTP cross-check.** A second transport uses Cloudflare's own method (`GET /api/__down`,
  `POST /api/__up`, timed with the browser's Resource Timing API) so you can compare both.
- **Latency, jitter and bufferbloat.** Idle latency and jitter from ping/pong on a dedicated
  socket, plus *loaded* latency measured while the download and upload run.
- **Packet loss over WebRTC.** A real unreliable UDP data channel to the server itself (no
  STUN or TURN needed on a LAN), echoed by a built-in str0m endpoint on one UDP port.
- **AIM scores.** Results are rated for streaming, gaming and video calls with Cloudflare's
  Aggregated Internet Measurement thresholds.
- **Labelled history and compare.** Every run is saved automatically to SQLite with a label
  (prefilled with your last one, or `<device> · <date>`), so "Kitchen" vs "Office" vs "Garden"
  takes no typing. Compare labels side by side, export CSV or JSON, and use the REST API from
  scripts.
- **Multi-device.** Every open browser shows up in the others' *Devices* panel with live
  speeds counted by the server. **Test together** runs a synchronized test on several devices
  and reports the aggregate, each device's share and a fairness index, so you see what the
  Wi-Fi delivers when it is shared. Solo runs that overlapped with someone else's test are
  flagged.
- **Installable PWA.** Over HTTPS the app installs to the home screen and has an offline
  shell. Measurement traffic is never cached.
- **One binary, any platform.** Linux (x86_64, arm64, armv7, statically linked), macOS and
  Windows. No runtime, no files next to it, no configuration needed.
- **Docker.** A multi-arch (`amd64`, `arm64`) image of the same binary on a distroless base,
  with a built-in healthcheck.

## Quick start: binary

Download the archive for your platform from the
[latest release](https://github.com/puttyman/speed-test-rs/releases/latest) and check it
against `SHA256SUMS`:

| Platform | Archive |
|---|---|
| Linux x86_64 (any distro, NAS, Proxmox LXC) | `speed-test-rs-<version>-x86_64-unknown-linux-musl.tar.gz` |
| Linux arm64 (Raspberry Pi 4/5 with a 64-bit OS, arm NAS) | `speed-test-rs-<version>-aarch64-unknown-linux-musl.tar.gz` |
| Linux armv7 (Raspberry Pi 2/3 with a 32-bit OS) | `speed-test-rs-<version>-armv7-unknown-linux-musleabihf.tar.gz` |
| macOS Apple silicon | `speed-test-rs-<version>-aarch64-apple-darwin.tar.gz` |
| macOS Intel | `speed-test-rs-<version>-x86_64-apple-darwin.tar.gz` |
| Windows x86_64 | `speed-test-rs-<version>-x86_64-pc-windows-msvc.zip` |

Then run it:

```bash
tar xzf speed-test-rs-*.tar.gz
cd speed-test-rs-*/
./speed-test-rs
```

It prints the URL for every network interface and a QR code for your phone:

```text
speed-test-rs 0.1.0
  data dir   /home/me/.local/share/speed-test-rs
  assets     embedded
  webrtc     udp/3478 on 192.168.1.10
  listening on all interfaces (0.0.0.0:8080, [::]:8080); use --listen 127.0.0.1:8080 to restrict
  local      http://localhost:8080
  network    http://192.168.1.10:8080

  Scan to open http://192.168.1.10:8080 on your phone:
  ...
```

Open that URL on any device on the same network and press **Start**. Useful options:

```bash
./speed-test-rs --open                       # also open it in this machine's browser
./speed-test-rs --listen 127.0.0.1:8080      # only this machine
./speed-test-rs --listen 9000                # another port, all interfaces
./speed-test-rs --help                       # every option
```

**Linux.** Ports above 1024 need no privileges. To run it as a service, see
[`contrib/speed-test-rs.service`](../contrib/speed-test-rs.service) (systemd, with a dynamic
user and `StateDirectory`).

**macOS.** The binaries are not signed or notarized yet, so Gatekeeper quarantines them.
Remove the quarantine flag once:

```bash
xattr -d com.apple.quarantine speed-test-rs
```

(or right-click the file in Finder and choose **Open**). macOS asks whether the app may accept
incoming connections: allow it. A launchd agent is in
[`contrib/com.speed-test-rs.plist`](../contrib/com.speed-test-rs.plist).

**Windows.** Run `speed-test-rs.exe`. The first start shows a Windows Defender Firewall
prompt: allow it on **Private** networks so phones can reach TCP 8080 and UDP 3478. Firewall
rules and running it as a service are covered in [`contrib/WINDOWS.md`](../contrib/WINDOWS.md).

History is stored in the platform data directory: `~/.local/share/speed-test-rs` on Linux,
`~/Library/Application Support/speed-test-rs` on macOS and `%APPDATA%\speed-test-rs` on
Windows (change it with `--data-dir`).

## Quick start: Docker

The image is `puttyman/speed-test-rs` (`linux/amd64` and `linux/arm64`). It listens on
TCP **8080** (UI, API and WebSocket tests) and UDP **3478** (WebRTC packet loss) and keeps
history in `/data`.

**Host networking** (Linux only, recommended): the container uses the host's network
directly, so there is no NAT in the measurement path and WebRTC sees the real LAN address.

```bash
docker run -d --name speedtest --restart unless-stopped --network host \
  -v speedtest-data:/data puttyman/speed-test-rs:latest
```

**Bridge networking** (works everywhere, including Docker Desktop): publish both ports and
tell the server its host's LAN IP, because inside the container it only knows its private
Docker address. Without `SPEEDTEST_PUBLIC_IP` packet loss shows as unavailable; everything
else still works.

```bash
docker run -d --name speedtest --restart unless-stopped \
  -p 8080:8080 -p 3478:3478/udp \
  -e SPEEDTEST_PUBLIC_IP=192.168.1.10 \
  -v speedtest-data:/data puttyman/speed-test-rs:latest
```

If you publish the UDP port on a different host port, set `SPEEDTEST_UDP_PORT` to the same
number inside the container (`-p 4478:4478/udp -e SPEEDTEST_UDP_PORT=4478`): the server
advertises its own port in the WebRTC candidates.

With Compose, [`docker-compose.yml`](../docker-compose.yml) has both variants:

```bash
SPEEDTEST_PUBLIC_IP=192.168.1.10 docker compose up -d     # bridge
docker compose --profile host up -d speed-test-host       # host networking
```

The image runs as a non-root user on `gcr.io/distroless/static`, and its `HEALTHCHECK` calls
the binary's own `healthcheck` subcommand. Docker's userland proxy and bridge NAT can cap
throughput on slow hosts (a Raspberry Pi, for example), so for multi-gigabit tests use host
networking or the standalone binary.

## Configuration

Everything works without configuration. Every setting is a command-line flag, an environment
variable, or both; a flag beats the environment, which beats the default. Empty environment
variables count as unset.

| Flag | Environment variable | Default | Description |
|---|---|---|---|
| `-l, --listen <ADDR>` | `SPEEDTEST_LISTEN` (or `LEPTOS_SITE_ADDR`) | `0.0.0.0:8080` and `[::]:8080` | HTTP and WebSocket listen address. Repeat it or separate with commas; a bare port (`9000`) means all interfaces. |
| `--udp-port <PORT>` | `SPEEDTEST_UDP_PORT` | `3478` | UDP port of the WebRTC packet-loss endpoint. |
| `--public-ip <IP>` | `SPEEDTEST_PUBLIC_IP` | auto | IP advertised in WebRTC ICE candidates. Set it under Docker bridge networking. |
| `--server-name <NAME>` | `SPEEDTEST_SERVER_NAME` | hostname | Shown in the UI and stored with each run. |
| `--data-dir <DIR>` | `SPEEDTEST_DATA_DIR` | platform data dir (`/data` in Docker) | Where the database lives. |
| `--db-path <FILE>` | `SPEEDTEST_DB_PATH` | `<data-dir>/speedtest.db` | SQLite file; overrides `--data-dir`. |
| `--no-history` | `SPEEDTEST_HISTORY=false` | history on | Don't save runs (stateless, Cloudflare-like). |
| `--retention-days <N>` | `SPEEDTEST_RETENTION_DAYS` | keep forever | Delete runs older than N days (checked daily). |
| `--max-sessions <N>` | `SPEEDTEST_MAX_SESSIONS` | `16` | Concurrent test sessions; more get a "server busy" error. |
| `--max-bytes <N>` | `SPEEDTEST_MAX_BYTES` | `2000000000` | Largest single download or upload request, in bytes. |
| `--no-presence` | `SPEEDTEST_PRESENCE=false` | presence on | Turn off the live Devices panel and group tests. |
| `--server-link-mbps <N>` | `SPEEDTEST_SERVER_LINK_MBPS` | auto (Linux) | The server's link speed, for the "server may be the bottleneck" warning. |
| `--group-test-seconds <N>` | `SPEEDTEST_GROUP_TEST_SECONDS` | `10` | Seconds of download and of upload in a Test together run (2–300). |
| `--tls-cert <FILE>` | `SPEEDTEST_TLS_CERT` | – | PEM certificate (chain). Together with `--tls-key` it turns on HTTPS. |
| `--tls-key <FILE>` | `SPEEDTEST_TLS_KEY` | – | PEM private key. |
| `--tls-listen <ADDR>` | `SPEEDTEST_TLS_LISTEN` | `0.0.0.0:8443` and `[::]:8443` | HTTPS/WSS listen address, next to the plain HTTP one. Needs a certificate. |
| `--site-root <DIR>` | `LEPTOS_SITE_ROOT` | embedded | Serve the UI files from a directory instead of the embedded copy (development, theming). |
| `--log <FILTER>` | `RUST_LOG` | `info` | Log filter ([`tracing` `EnvFilter`](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html) syntax), e.g. `debug` or `info,speed_test_rs=debug`. |
| `--open` | – | off | Open the UI in the default browser after startup. |
| `--no-qr` | – | QR on | Don't print the QR code at startup. |
| – | `SPEEDTEST_AUTH_TOKEN` | – | Require this token for `/api/*` (except `/api/health`) and the WebSockets. Environment only, so it doesn't show up in `ps`. |
| – | `SPEEDTEST_STORE_CLIENT_INFO=false` | on | Don't store the client IP and User-Agent with each run. |

Boolean variables accept `true`/`false`, `1`/`0`, `yes`/`no` and `on`/`off`.

Subcommands:

```text
speed-test-rs [serve] [OPTIONS]                 run the server (the default)
speed-test-rs healthcheck [--listen <ADDR>]     exit 0 if the local server answers /api/health
speed-test-rs --version | --help
```

`healthcheck` reads `--listen` / `SPEEDTEST_LISTEN` the same way the server does, so it
finds the port without extra arguments (this is what the Docker `HEALTHCHECK` runs).

## HTTPS and installing the app (PWA)

The app works fully over plain `http://192.168.x.x:8080`. Browsers only offer **Install** and
run the offline shell (service worker) on a secure origin, though, so for the full PWA serve
it over HTTPS with a certificate your devices trust. A self-signed certificate is not enough:
browsers refuse service workers on pages with certificate errors.

### Built-in TLS with mkcert

[mkcert](https://github.com/FiloSottile/mkcert) creates a local certificate authority and
certificates signed by it:

```bash
mkcert -install                                   # creates and trusts the local CA on this machine
mkcert speedtest.lan 192.168.1.10                 # writes speedtest.lan+1.pem and speedtest.lan+1-key.pem
./speed-test-rs --tls-cert speedtest.lan+1.pem --tls-key speedtest.lan+1-key.pem
```

The server now also listens on `https://192.168.1.10:8443` (change it with `--tls-listen`);
plain HTTP on 8080 keeps working. Other devices must trust the mkcert CA: copy
`"$(mkcert -CAROOT)/rootCA.pem"` to them and install it as a CA certificate.

- **iOS/iPadOS:** open the file to install the profile (Settings → General → VPN & Device
  Management), then turn on full trust under Settings → General → About → Certificate Trust
  Settings.
- **Android:** Settings → Security → Encryption & credentials → Install a certificate → CA
  certificate.

Anyone holding the mkcert CA key can impersonate any site to those devices, so keep
`rootCA-key.pem` private. The project never generates or installs certificates itself.

In Docker, mount the files and pass the paths:

```bash
docker run -d --network host -v speedtest-data:/data -v "$PWD/certs:/certs:ro" \
  -e SPEEDTEST_TLS_CERT=/certs/speedtest.lan+1.pem \
  -e SPEEDTEST_TLS_KEY=/certs/speedtest.lan+1-key.pem \
  puttyman/speed-test-rs:latest
```

### Behind a reverse proxy

A reverse proxy with a real domain and a public certificate (Let's Encrypt via DNS challenge,
for example) needs no changes on the devices. Proxy everything, including WebSocket upgrades
on `/ws/*`, to port 8080. With [Caddy](https://caddyserver.com):

```caddyfile
speedtest.example.com {
    reverse_proxy 192.168.1.10:8080
}
```

Traefik, Nginx Proxy Manager and plain nginx work too (for nginx, forward the `Upgrade` and
`Connection` headers and turn off `proxy_buffering` and `proxy_request_buffering`). Keep in
mind that the proxy is part of what you measure: it adds a hop and its own CPU cost, and at
multi-gigabit speeds it can become the bottleneck. The WebRTC packet-loss test does not go
through the proxy; browsers connect straight to UDP 3478 on the server's LAN address.

Measuring over HTTPS encrypts all test traffic, which costs CPU on slow servers. The UI shows
the transport (for example "WebSocket · TLS") and history records it, so TLS and plain runs
are not compared by accident.

## Getting accurate results

A speed test measures the **slowest hop** between the browser and the server. Check these
before trusting a number:

- **The server's own link.** A server on 1 GbE can never show more than about 940 Mbps,
  however fast your Wi-Fi is. For a Wi-Fi 6/7 access point, run the server on a machine with
  a 2.5 or 10 GbE port, wired to the same switch as the access point. The group-test result
  warns when the total gets close to the server's link speed (detected on Linux, or set with
  `--server-link-mbps`).
- **The server's CPU.** Small boards (Raspberry Pi 2/3, low-end NAS) can run out of CPU before
  the network is saturated, especially over HTTPS or Docker bridge networking.
- **Wi-Fi is shared and variable.** Distance, walls, the channel, other devices and the client
  radio all matter. Run a few tests per spot and compare labels rather than single runs. The
  Devices panel shows when someone else is testing at the same time, and such runs are flagged.
- **Browsers have ceilings.** A single browser tab tops out somewhere between about 1 and
  5 Gbps depending on the browser, the CPU and the transport. WebSocket is usually faster
  than the HTTP mode; Chromium-based browsers are usually fastest. On a phone, a hot or
  low-power device throttles. If the result looks capped, try more parallel streams in the
  settings, another browser, or a wired client to see whether the limit is the client.
- **Docker bridge networking** adds NAT and, for some setups, a userland proxy. Use host
  networking or the standalone binary for multi-gigabit tests.
- **VPNs and proxies** on the client route traffic away from the LAN. Turn them off while
  testing.

## Security

speed-test-rs is designed for a **trusted LAN**:

- By default it listens on **all interfaces** so phones can reach it. On a laptop in an
  untrusted network (a café or hotel), stop it or run it with `--listen 127.0.0.1:8080`.
- There is no login by default. Set `SPEEDTEST_AUTH_TOKEN` to require a token
  (`Authorization: Bearer <token>` or the `speedtest_token` cookie for the API; WebSockets send
  it in their first message).
- Each test session is limited (bytes per request, parallel streams, concurrent sessions and
  an idle timeout) so one client cannot keep the server saturated.
- **Never expose it to the internet** without the token and a TLS reverse proxy. It is a
  bandwidth generator by design.
- The client IP and User-Agent are stored with each run to tell devices apart; turn that off
  with `SPEEDTEST_STORE_CLIENT_INFO=false`.
- The server makes **no outbound connections**.

## Development

Requirements:

- Rust stable (1.94 or newer). `rust-toolchain.toml` installs the toolchain and the
  `wasm32-unknown-unknown` target.
- [`cargo-leptos`](https://github.com/leptos-rs/cargo-leptos) 0.3.11
  (`cargo install cargo-leptos --locked`). It downloads `sass`, `wasm-bindgen` and `wasm-opt`
  itself.
- Optional: [`just`](https://just.systems) for the recipes below, Node.js 22 for the
  Playwright tests, and [`cargo-zigbuild`](https://github.com/rust-cross/cargo-zigbuild) plus
  [Zig](https://ziglang.org) for the static Linux builds.
- A C compiler. SQLite is bundled and compiled from C, and str0m's DTLS uses
  [aws-lc](https://github.com/aws/aws-lc) through `aws-lc-sys`. On Windows that needs
  Visual Studio Build Tools and [NASM](https://www.nasm.us) on `PATH` (or set
  `AWS_LC_SYS_PREBUILT_NASM=1` to use aws-lc-sys's bundled NASM objects).

```bash
cargo leptos watch       # dev server with live reload on http://127.0.0.1:3000
```

| Recipe | What it does |
|---|---|
| `just dev` | `cargo leptos watch` |
| `just build` | Release site (`target/site`) and a server that serves it from disk |
| `just dist [target]` | The standalone binary: builds the front end once, then the server with the site embedded (`--features ssr,embed-assets`) for the host or the given target triple. Linux musl targets use `cargo zigbuild`. |
| `just smoke [target]` | `dist`, then [`scripts/smoke-binary.sh`](../scripts/smoke-binary.sh): copies the binary alone into an empty directory, starts it and checks the page, the assets and `healthcheck`. |
| `just test` | Unit and server integration tests (`cargo test --no-default-features --features ssr`) |
| `just lint` | `cargo fmt --check` and clippy for the `ssr`, `hydrate` (wasm32) and `ssr,embed-assets` builds |
| `just e2e` | Playwright end-to-end tests (`cargo leptos end-to-end`; run `npm ci` in `end2end/` once) |

The crate builds two ways from the same source: the server with `--features ssr` and the
browser bundle (`cdylib`, wasm32) with `--features hydrate`. cargo-leptos runs both builds;
the standalone binary is a separate `cargo build --features ssr,embed-assets` after the front
end exists, because `build.rs` compresses and embeds `target/site`.

Two build settings matter when you build by hand:

- **`--cfg erase_components`.** The front end is built in Leptos' erased-components mode in
  every profile (`always-erase-components` in `Cargo.toml`): it makes the WASM bundle about
  17% smaller. The server must be compiled the same way or hydration fails in the browser.
  `.cargo/config.toml` sets it for plain `cargo` builds; if you set `RUSTFLAGS` yourself, add
  `--cfg erase_components` to it (the `embed-assets` build stops with an error otherwise).
- **`LEPTOS_OUTPUT_NAME=speed-test-rs`** names the bundle; `.cargo/config.toml` sets it too.

Pull requests run [`ci.yml`](../.github/workflows/ci.yml): rustfmt, clippy (`ssr`, `hydrate`,
`ssr,embed-assets`), tests, Playwright in Chromium, Firefox and WebKit, and a static
x86_64 musl build with the smoke test. Tags `v*` run
[`release.yml`](../.github/workflows/release.yml), which builds every target and the Docker
image. See [CONTRIBUTING.md](../CONTRIBUTING.md) for the workflow.

## Architecture

```text
browser (WASM, `hydrate`)                         server (`ssr`, Axum + Tokio)
───────────────────────────                       ─────────────────────────────────────
pages/ + components/  ── SSR HTML, hydration ───  Leptos routes, embedded assets
engine/ (runner)      ── /ws/control (JSON) ───── presence, groups, sessions, WebRTC signalling
   ws_transport       ── /ws/data, /ws/probe ──── byte streams, upload acks, ping/pong
   http_transport     ── /api/__down, __up ────── HTTP mode
   rtc                ── UDP 3478 (DTLS/SCTP) ─── str0m echo endpoint
pages/api             ── /api/runs, labels… ───── SQLite (sqlx), history REST API
            model/ (shared): protocol, profiles, stats, scores, results
```

The browser does the measuring and computes the summary with the same `model` code the
server uses. The server streams bytes, times uploads, counts every session's traffic for
presence and group tests, echoes WebRTC packets, and stores the results.
[`docs/ARCHITECTURE.md`](ARCHITECTURE.md) has the module map and the life of a test;
[`docs/SPEC.md`](SPEC.md) has the full design.

## License

Licensed under either of [Apache License, Version 2.0](../LICENSE-APACHE) or
[MIT license](../LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion
in this project by you, as defined in the Apache-2.0 license, shall be dual licensed as above,
without any additional terms or conditions.
