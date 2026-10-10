// speed-test-rs service worker (SPEC §7.9).
//
// Caches only the app shell: build output under /pkg/, self-hosted fonts and icons, plus an
// offline page. Measurement traffic (/api/*, /ws/*) and every non-GET request are never
// intercepted: the handler returns early so the browser talks to the network directly and
// no service-worker overhead touches the measurement path.
//
// Bump VERSION whenever this file or the offline page changes; old caches are deleted on
// activate. The new worker waits until the page asks it to take over ({type: "SKIP_WAITING"},
// sent by window.__pwaApplyUpdate() in /pwa.js) so a running test is never interrupted.

const VERSION = "v1";
const CACHE_PREFIX = "speed-test-rs-";
const SHELL_CACHE = `${CACHE_PREFIX}shell-${VERSION}`;
const ASSET_CACHE = `${CACHE_PREFIX}assets-${VERSION}`;
const OFFLINE_URL = "/offline.html";

// Small, fixed-name files needed to render the offline page.
const PRECACHE = [OFFLINE_URL, "/favicon.svg"];

// cargo-leptos `hash-files = true` names: <name>.<hash>.<ext>
const HASHED_ASSET = /\.[A-Za-z0-9_-]{8,}\.(?:js|mjs|wasm|css)$/;

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches.open(SHELL_CACHE).then((cache) => cache.addAll(PRECACHE.map((u) => new Request(u, { cache: "reload" })))),
  );
});

self.addEventListener("activate", (event) => {
  const keep = new Set([SHELL_CACHE, ASSET_CACHE]);
  event.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((k) => k.startsWith(CACHE_PREFIX) && !keep.has(k)).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("message", (event) => {
  if (event.data && event.data.type === "SKIP_WAITING") {
    self.skipWaiting();
  }
});

function isMeasurementPath(path) {
  return path === "/api" || path.startsWith("/api/") || path === "/ws" || path.startsWith("/ws/");
}

self.addEventListener("fetch", (event) => {
  const req = event.request;
  // Never touch non-GET requests (uploads, POST /api/runs, ...).
  if (req.method !== "GET") return;

  const url = new URL(req.url);
  // Only same-origin requests; never measurement or WebSocket upgrade traffic.
  if (url.origin !== self.location.origin) return;
  if (isMeasurementPath(url.pathname)) return;
  if (req.headers.get("upgrade")) return;
  // Range requests can't be answered from a full cached body safely.
  if (req.headers.has("range")) return;

  if (req.mode === "navigate") {
    event.respondWith(networkFirstNavigation(req));
    return;
  }

  const path = url.pathname;
  if (path.startsWith("/pkg/")) {
    // Hashed build output is immutable: cache-first. Unhashed names (dev builds without
    // hash-files) may change under the same URL, so prefer the network for those.
    event.respondWith(HASHED_ASSET.test(path) ? cacheFirst(req) : networkFirst(req));
    return;
  }
  if (path.startsWith("/fonts/") || path.startsWith("/icons/")) {
    event.respondWith(cacheFirst(req));
  }
  // Everything else falls through to the network untouched.
});

async function networkFirstNavigation(req) {
  try {
    // Pages are rendered by the server per request; they are not cached.
    return await fetch(req);
  } catch (err) {
    const cache = await caches.open(SHELL_CACHE);
    const offline = await cache.match(OFFLINE_URL);
    return offline || Response.error();
  }
}

async function cacheFirst(req) {
  const cache = await caches.open(ASSET_CACHE);
  const hit = await cache.match(req);
  if (hit) return hit;
  try {
    const res = await fetch(req);
    store(cache, req, res);
    return res;
  } catch {
    // Offline, or the page navigated away and the browser aborted the request.
    return Response.error();
  }
}

/** Caches a successful same-origin response; a failed write (aborted body, quota) is ignored. */
function store(cache, req, res) {
  if (res.ok && res.type === "basic") {
    cache.put(req, res.clone()).catch(() => {});
  }
}

async function networkFirst(req) {
  const cache = await caches.open(ASSET_CACHE);
  try {
    const res = await fetch(req);
    store(cache, req, res);
    return res;
  } catch {
    return (await cache.match(req)) || Response.error();
  }
}
