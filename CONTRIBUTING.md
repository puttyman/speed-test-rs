# Contributing

Thanks for helping! Bug reports, measurements from unusual setups and pull requests are all
welcome.

## Before you start

- Read [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the module map and
  [`docs/SPEC.md`](docs/SPEC.md) for the design. For anything larger than a fix, open an
  issue first so we can agree on the approach.
- The measurement method follows Cloudflare's speed test on purpose (§5 of the spec).
  Changes to it should explain why, ideally with before/after numbers.

## Setup

```bash
rustup show                         # installs the toolchain from rust-toolchain.toml
cargo install cargo-leptos --locked # 0.3.11, the version CI uses
cargo leptos watch                  # http://127.0.0.1:3000 with live reload
```

[`just`](https://just.systems) wraps the common commands (`just --list`). For the Playwright
tests, run `npm ci` in `end2end/` once.

## Checks

CI runs these on every pull request; run them locally first:

```bash
just lint     # cargo fmt --check + clippy for ssr, hydrate (wasm32) and ssr,embed-assets
just test     # unit and server integration tests
just e2e      # Playwright (Chromium, Firefox, WebKit)
just smoke    # standalone binary + scripts/smoke-binary.sh
```

Clippy runs with `-D warnings`. Without `just`, the commands are in the `justfile`.

## Guidelines

- **Shared logic goes in `model/`**: it is compiled into both the server and the WASM
  bundle, has no I/O and is unit tested natively. Browser-only code goes in `engine/`
  (behind `hydrate`), server-only code in `server/` (behind `ssr`).
- **Keep the measurement path lean.** UI components only read signals; the engine never
  waits on the UI.
- **Tests:** new server behaviour gets an integration test (see `tests/` and the
  `#[cfg(test)]` modules), new UI behaviour a Playwright check where practical.
- **No outbound network calls**, telemetry or external assets.
- **Dependencies:** keep the WASM bundle small (check `target/site/pkg/speed-test-rs.wasm`
  after `cargo leptos build --release`) and avoid new C dependencies, which complicate the
  cross builds.
- **Builds by hand:** the front end and server must both be built with
  `--cfg erase_components`. `.cargo/config.toml` sets it; if you set `RUSTFLAGS`, include it.
- Commit messages: imperative mood, a short summary line, and a body that says why.

## Releasing (maintainers)

1. Update `version` in `Cargo.toml` (and `Cargo.lock`), commit and push to `main`.
2. Start the release: **Actions → Release → Run workflow** with the version (`X.Y.Z`), or push
   a tag `vX.Y.Z`.
3. `release.yml` first runs the whole CI suite (lint, unit and integration tests, Playwright
   e2e in Chromium, Firefox and WebKit, musl smoke test). Only if it passes does it build every
   target, the Docker image (`puttyman/speed-test-rs:X.Y.Z` on Docker Hub, when the
   `DOCKERHUB_USERNAME`/`DOCKERHUB_TOKEN` secrets are set, and `ghcr.io/puttyman/speed-test-rs`,
   plus `X.Y` and `latest`) and the GitHub Release with `SHA256SUMS`, creating the tag if
   needed. Versions with a `-` (e.g. `0.2.0-rc.1`) are published as pre-releases.

## License

By contributing you agree that your contributions are dual licensed under MIT or Apache-2.0,
like the rest of the project.
