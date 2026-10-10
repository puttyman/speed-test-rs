#!/usr/bin/env bash
# Binary smoke test (docs/SPEC.md §12): copies a built server binary ALONE into an empty
# temporary directory, starts it on a free loopback port, and checks that the page, the
# health endpoint and the embedded assets are served. Exits non-zero on any failure.
#
# Usage: scripts/smoke-binary.sh [path/to/speed-test-rs] [port]
#   default binary: ${CARGO_TARGET_DIR:-target}/release/speed-test-rs
#   default port:   a free port picked by the OS
set -euo pipefail

bin="${1:-${CARGO_TARGET_DIR:-target}/release/speed-test-rs}"
port="${2:-}"
name="speed-test-rs"

if [[ ! -x "$bin" ]]; then
    echo "smoke: binary not found or not executable: $bin" >&2
    exit 1
fi

if [[ -z "$port" ]]; then
    port="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
fi

work="$(mktemp -d)"
pid=""
cleanup() {
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    fi
    rm -rf "$work"
}
trap cleanup EXIT

cp "$bin" "$work/$name"
cd "$work"

base="http://127.0.0.1:$port"
failures=0
pass() { echo "ok   $*"; }
fail() { echo "FAIL $*" >&2; failures=$((failures + 1)); }

# Run with a clean environment so no LEPTOS_* / SPEEDTEST_* variable can leak in, and
# keep the data dir inside the temp dir.
env -i PATH="$PATH" HOME="$work" \
    "./$name" --listen "127.0.0.1:$port" --no-qr --data-dir "$work/data" \
    >"$work/server.log" 2>&1 &
pid=$!

# Up to 30 s: emulated targets (armv7 under qemu-user in the release workflow) start slowly.
ready=""
for _ in $(seq 1 300); do
    if curl -fsS -o /dev/null "$base/api/health" 2>/dev/null; then
        ready=1
        break
    fi
    if ! kill -0 "$pid" 2>/dev/null; then
        break
    fi
    sleep 0.1
done
if [[ -z "$ready" ]]; then
    echo "smoke: server did not become ready; log:" >&2
    cat "$work/server.log" >&2
    exit 1
fi
pass "server ready on $base"

# check <path> <expected content-type prefix>
check() {
    local path="$1" want_type="$2" out code type
    out="$(curl -sS -o /dev/null -w '%{http_code} %{content_type}' "$base$path")" || {
        fail "$path: request failed"
        return
    }
    code="${out%% *}"
    type="${out#* }"
    if [[ "$code" != "200" ]]; then
        fail "$path: status $code"
    elif [[ "$type" != "$want_type"* ]]; then
        fail "$path: content-type '$type', want '$want_type'"
    else
        pass "$path ($code, $type)"
    fi
}

check "/" "text/html"
check "/api/health" "application/json"
check "/pkg/$name.wasm" "application/wasm"
check "/pkg/$name.js" "text/javascript"
check "/pkg/$name.css" "text/css"
check "/favicon.ico" "image/"
check "/manifest.webmanifest" "application/manifest+json"
check "/sw.js" "text/javascript"

cc="$(curl -sS -o /dev/null -D - "$base/sw.js" | tr -d '\r' | awk -F': ' 'tolower($1)=="cache-control"{print $2}')"
if [[ "$cc" == "no-cache" ]]; then
    pass "/sw.js cache-control: no-cache"
else
    fail "/sw.js cache-control '$cc', want 'no-cache'"
fi

# The page must reference the stylesheet and the hydration bundle.
page="$(curl -fsS "$base/")"
for needle in "/pkg/$name.css" "$name.wasm"; do
    if [[ "$page" == *"$needle"* ]]; then
        pass "page references $needle"
    else
        fail "page does not reference $needle"
    fi
done

# Pre-compressed variant is served when accepted.
enc="$(curl -sS -o /dev/null -D - -H 'Accept-Encoding: br' "$base/pkg/$name.wasm" | tr -d '\r' | awk -F': ' 'tolower($1)=="content-encoding"{print $2}')"
if [[ "$enc" == "br" ]]; then
    pass "/pkg/$name.wasm served brotli-compressed"
else
    fail "/pkg/$name.wasm: content-encoding '$enc', want 'br'"
fi

code="$(curl -sS -o /dev/null -w '%{http_code}' "$base/definitely/not/here")"
if [[ "$code" == "404" ]]; then pass "unknown path -> 404"; else fail "unknown path -> $code"; fi

if env -i PATH="$PATH" "./$name" healthcheck --listen "127.0.0.1:$port"; then
    pass "healthcheck"
else
    fail "healthcheck exited non-zero"
fi
if env -i PATH="$PATH" SPEEDTEST_LISTEN="0.0.0.0:$port" "./$name" healthcheck; then
    pass "healthcheck (SPEEDTEST_LISTEN, unspecified address)"
else
    fail "healthcheck with SPEEDTEST_LISTEN exited non-zero"
fi

kill "$pid"
if wait "$pid"; then
    pass "graceful shutdown"
else
    fail "server exited with status $? on SIGTERM"
fi
pid=""

if [[ "$failures" -ne 0 ]]; then
    echo "smoke: $failures check(s) failed; server log:" >&2
    cat "$work/server.log" >&2
    exit 1
fi
echo "smoke: all checks passed ($(wc -c <"$work/$name") byte binary)"
