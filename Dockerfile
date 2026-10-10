# syntax=docker/dockerfile:1
#
# Local image build (SPEC §10.2): `docker build -t speed-test-rs .`
# Release images are built by .github/workflows/release.yml from the prebuilt musl binaries
# instead (Dockerfile.release), so Docker and binary users run byte-identical code.
#
# Why musl: the server is linked statically for x86_64/aarch64-unknown-linux-musl, the same
# targets the release ships. The runtime stage then needs no libc at all, so it can be
# distroless/static (no shell, no package manager, ~2 MB base) and match the release image.
# Works for linux/amd64 and linux/arm64 (TARGETARCH picks the musl triple).

# ---- Toolchain ---------------------------------------------------------------
FROM rust:1-trixie AS chef

ARG CARGO_LEPTOS_VERSION=0.3.11
ARG CARGO_CHEF_VERSION=0.1.78
ARG TARGETARCH

# Use the image's toolchain rather than letting rust-toolchain.toml download "stable" again.
ENV RUSTUP_TOOLCHAIN=${RUST_VERSION} \
    CC_x86_64_unknown_linux_musl=musl-gcc \
    CC_aarch64_unknown_linux_musl=musl-gcc \
    CARGO_INCREMENTAL=0 \
    LEPTOS_OUTPUT_NAME=speed-test-rs

RUN apt-get update \
    && apt-get install -y --no-install-recommends musl-tools \
    && rm -rf /var/lib/apt/lists/*

RUN set -eux; \
    case "${TARGETARCH:-amd64}" in \
      amd64) arch=x86_64 ;; \
      arm64) arch=aarch64 ;; \
      *) echo "unsupported TARGETARCH: ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    echo "${arch}-unknown-linux-musl" > /rust-target; \
    rustup target add wasm32-unknown-unknown "$(cat /rust-target)"; \
    curl -fsSL "https://github.com/leptos-rs/cargo-leptos/releases/download/v${CARGO_LEPTOS_VERSION}/cargo-leptos-${arch}-unknown-linux-gnu.tar.gz" \
      | tar xz --strip-components=1 -C /usr/local/cargo/bin "cargo-leptos-${arch}-unknown-linux-gnu/cargo-leptos"; \
    cargo install --locked cargo-chef --version "${CARGO_CHEF_VERSION}"; \
    rm -rf /usr/local/cargo/registry

WORKDIR /app

# ---- Dependency recipe (changes only when Cargo.toml/Cargo.lock change) --------
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ---- Build ---------------------------------------------------------------------
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
# The cargo config sets `--cfg erase_components` (and LEPTOS_OUTPUT_NAME); the cached
# dependencies must be built with the same flags as the real build or they are rebuilt.
COPY .cargo .cargo
# Cache dependencies for both halves of the two-step build: the WASM front end
# (wasm-release profile, hydrate) and the musl server (ssr).
RUN cargo chef cook --recipe-path recipe.json --profile wasm-release \
      --target wasm32-unknown-unknown --no-default-features --features hydrate
RUN cargo chef cook --recipe-path recipe.json --release \
      --target "$(cat /rust-target)" --no-default-features --features ssr

COPY . .
# 1. Front end (WASM/JS/CSS + public/) into target/site
RUN cargo leptos build --release --frontend-only
# 2. Server with target/site embedded (separate cargo build, SPEC §10.1)
RUN set -eux; \
    target="$(cat /rust-target)"; \
    cargo build --locked --release --no-default-features --features ssr,embed-assets --target "$target"; \
    install -D -m 0755 "target/$target/release/speed-test-rs" /out/app/speed-test-rs; \
    mkdir -p /out/data

# ---- Runtime -------------------------------------------------------------------
FROM gcr.io/distroless/static-debian12:nonroot AS runtime

COPY --from=builder /out/app/speed-test-rs /app/speed-test-rs
# Pre-create /data owned by the nonroot user so named volumes inherit writable ownership.
COPY --from=builder --chown=65532:65532 /out/data /data

# LEPTOS_SITE_ADDR is the legacy alias of SPEEDTEST_LISTEN (SPEC §9.2); also read by the
# healthcheck subcommand to find the port.
ENV SPEEDTEST_DATA_DIR=/data \
    SPEEDTEST_LISTEN=0.0.0.0:8080 \
    LEPTOS_SITE_ADDR=0.0.0.0:8080 \
    RUST_LOG=info

USER nonroot:nonroot
WORKDIR /app
VOLUME ["/data"]
EXPOSE 8080/tcp 3478/udp

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD ["/app/speed-test-rs", "healthcheck"]

ENTRYPOINT ["/app/speed-test-rs"]
