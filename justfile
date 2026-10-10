# Task runner for speed-test-rs (https://just.systems). `just` lists the recipes.

set shell := ["bash", "-euo", "pipefail", "-c"]

export LEPTOS_OUTPUT_NAME := "speed-test-rs"

host := `rustc -vV | sed -n 's/^host: //p'`
target_dir := env("CARGO_TARGET_DIR", "target")

# List recipes
default:
    @just --list

# Dev server with live reload on http://127.0.0.1:3000
dev:
    cargo leptos watch

# Release build of the site (target/site) and the disk-serving server
build:
    cargo leptos build --release

# Standalone binary with the UI embedded (SPEC §10.1). Linux musl targets use cargo-zigbuild
# (needs `zig` on PATH; CI uses Zig 0.17.0 and cargo-zigbuild 0.23.4).
dist target=host:
    # 1. Front end (WASM/JS/CSS + public/) into target/site
    cargo leptos build --release --frontend-only
    # 2. Server for {{ target }} with target/site embedded
    rustup target add {{ target }}
    cargo {{ if target =~ "-linux-musl" { "zigbuild" } else { "build" } }} --locked --release \
        --no-default-features --features ssr,embed-assets --target {{ target }}
    @echo "built {{ target_dir }}/{{ target }}/release/speed-test-rs{{ if target =~ "windows" { ".exe" } else { "" } }}"

# Unit and server integration tests
test:
    cargo test --no-default-features --features ssr

# rustfmt + clippy for every feature set CI checks
lint:
    cargo fmt --all --check
    cargo clippy --no-default-features --features ssr --all-targets -- -D warnings
    cargo clippy --lib --target wasm32-unknown-unknown --no-default-features --features hydrate -- -D warnings
    cargo leptos build --frontend-only
    cargo clippy --no-default-features --features ssr,embed-assets --all-targets -- -D warnings

# Playwright end-to-end tests (run `npm ci` in end2end/ once first)
e2e:
    cargo leptos end-to-end

# Build the dist binary for a target and run the binary smoke test against it
smoke target=host: (dist target)
    bash scripts/smoke-binary.sh "{{ target_dir }}/{{ target }}/release/speed-test-rs{{ if target =~ "windows" { ".exe" } else { "" } }}"
