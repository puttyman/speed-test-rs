# syntax=docker/dockerfile:1

# ---- Build stage -----------------------------------------------------------
FROM rust:1-bookworm AS builder

ARG CARGO_LEPTOS_VERSION=0.3.11

RUN rustup target add wasm32-unknown-unknown \
    && curl -fsSL "https://github.com/leptos-rs/cargo-leptos/releases/download/v${CARGO_LEPTOS_VERSION}/cargo-leptos-x86_64-unknown-linux-gnu.tar.gz" \
       | tar xz --strip-components=1 -C /usr/local/cargo/bin cargo-leptos-x86_64-unknown-linux-gnu/cargo-leptos

WORKDIR /app
COPY . .
RUN cargo leptos build --release

# ---- Runtime stage ---------------------------------------------------------
FROM debian:bookworm-slim AS runtime

RUN useradd --system --uid 10001 --home /app speedtest

WORKDIR /app
COPY --from=builder /app/target/release/speed-test-rs /app/speed-test-rs
COPY --from=builder /app/target/site /app/site

ENV LEPTOS_OUTPUT_NAME="speed-test-rs" \
    LEPTOS_SITE_ROOT="site" \
    LEPTOS_SITE_PKG_DIR="pkg" \
    LEPTOS_SITE_ADDR="0.0.0.0:8080" \
    LEPTOS_ENV="PROD" \
    RUST_LOG="info"

USER speedtest
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s \
    CMD ["/app/speed-test-rs", "healthcheck"]

ENTRYPOINT ["/app/speed-test-rs"]
