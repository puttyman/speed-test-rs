# speed-test-rs

A self-hosted, [Cloudflare-style](https://speed.cloudflare.com) network speed test for your
**local network**, built in Rust with [Leptos](https://github.com/leptos-rs/leptos) and
[Axum](https://github.com/tokio-rs/axum).

Run it as a single self-contained binary or in Docker on a machine in your home or office,
then open it from any phone or laptop to
measure the link to that machine: download, upload, latency, jitter, latency under load and
packet loss. Label each run with the room or access point and compare them in the history.

> **Status: early development.** The project skeleton builds and runs, and the measurement
> engine is being built. See [`docs/SPEC.md`](docs/SPEC.md) for the full design and roadmap.

## How it works

- Measurements run over **WebSocket** by default. An HTTP mode, using the same method as
  Cloudflare, is available for cross-checking.
- Packet loss is measured over a real UDP **WebRTC data channel** served by the app itself.
  No external TURN server is needed.
- Results are scored with Cloudflare's AIM model (streaming, gaming, video chat) and saved to
  SQLite for history and comparison.
- Nothing leaves your network: no telemetry and no external assets.

## Quick start

### Standalone binary (planned for v0.1.0)

Download the binary for your platform from GitHub Releases and run it. The UI is embedded in
the binary, so nothing else is needed:

```bash
./speed-test-rs            # serves the UI and API on http://0.0.0.0:8080
```

> Not available yet: today the release binary still needs the `target/site` folder next to it.
> Embedding is milestone M7 in [`docs/SPEC.md`](docs/SPEC.md#101-standalone-binary).

### Docker

```bash
docker compose up -d --build
# or
docker build -t speed-test-rs .
docker run -d -p 8080:8080 --name speed-test speed-test-rs
```

Then open `http://<server-ip>:8080`.

## Development

Requirements: stable Rust (1.89 or newer; `rust-toolchain.toml` installs the WASM target for you) and `cargo-leptos`. `cargo-leptos` downloads `sass`
and `wasm-bindgen` automatically.

```bash
cargo install cargo-leptos --locked

cargo leptos watch          # dev server with hot reload on http://127.0.0.1:3000
cargo leptos build --release
```

The release build writes the server binary to `target/release/speed-test-rs` and the static
site to `target/site`.

### Tests

```bash
cargo clippy --features ssr -- -D warnings
cargo clippy --lib --target wasm32-unknown-unknown --features hydrate -- -D warnings

cd end2end && npm install && cd ..
cargo leptos end-to-end     # Playwright tests
```

### Configuration

| Variable | Default | Description |
|---|---|---|
| `LEPTOS_SITE_ADDR` | `127.0.0.1:3000` (`0.0.0.0:8080` in Docker) | Listen address |
| `LEPTOS_SITE_ROOT` | `target/site` (`site` in Docker) | Static site directory |

More options (UDP port, database path, auth token and others) are listed in
[`docs/SPEC.md` §9](docs/SPEC.md#9-configuration) and will be added as the features land.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion
in this work by you, as defined in the Apache-2.0 license, shall be dual licensed as above,
without any additional terms or conditions.
