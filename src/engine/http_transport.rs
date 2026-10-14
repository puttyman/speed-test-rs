//! HTTP transport (SPEC §5.8): Cloudflare-style `GET /api/__down` and
//! `POST /api/__up`, timed with `PerformanceResourceTiming`.

use std::cell::Cell;

use futures::future::try_join_all;
use js_sys::{Reflect, Uint8Array};
use uuid::Uuid;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    AbortController, Headers, PerformanceResourceTiming, ReadableStreamDefaultReader, RequestCache,
    RequestInit, Response,
};

use super::js::{EngineError, Result, js_err, now, performance, sleep, window};
use super::util::{
    HTTP_UPLOAD_CHUNK_BYTES, chunk_sizes, http_upload_duration_ms, parse_server_timing, split_bytes,
};
use super::{Sample, UploadBuffer};

/// Resource timing entries kept before the runner clears them between steps.
const TIMING_BUFFER_SIZE: u32 = 2000;

pub struct HttpTransport {
    origin: String,
    session: Uuid,
    auth: Option<String>,
    controller: AbortController,
    upload: UploadBuffer,
    rx_bytes: Cell<u64>,
    tx_bytes: Cell<u64>,
    counter: Cell<u64>,
    nonce: u32,
}

/// Timing of one finished request.
struct Timed {
    /// `PerformanceResourceTiming`, if the browser recorded it.
    entry: Option<PerformanceResourceTiming>,
    /// `performance.now()` fallbacks: before `fetch`, headers received, body done.
    t_start: f64,
    t_headers: f64,
    t_end: f64,
    server_ms: Option<f64>,
    body_bytes: u64,
    /// `X-Received-Bytes`: what the server read of an upload body.
    received: Option<u64>,
}

impl HttpTransport {
    pub fn new(
        origin: &str,
        session: Uuid,
        auth: Option<String>,
        upload: UploadBuffer,
    ) -> Result<Self> {
        let perf = performance();
        perf.set_resource_timing_buffer_size(TIMING_BUFFER_SIZE);
        perf.clear_resource_timings();
        Ok(Self {
            origin: origin.trim_end_matches('/').to_string(),
            session,
            auth,
            controller: AbortController::new().map_err(js_err)?,
            upload,
            rx_bytes: Cell::new(0),
            tx_bytes: Cell::new(0),
            counter: Cell::new(0),
            nonce: (js_sys::Math::random() * f64::from(u32::MAX)) as u32,
        })
    }

    /// Clears recorded resource timings. Call only while no request is in flight.
    pub fn between_steps(&self) {
        performance().clear_resource_timings();
    }

    /// A unique URL, so no cache can answer and each timing entry is found by name.
    fn url(&self, path: &str, query: &str) -> String {
        let n = self.counter.get();
        self.counter.set(n + 1);
        let sep = if query.is_empty() { "" } else { "&" };
        format!(
            "{}{path}?{query}{sep}session={}&r={:x}-{n}",
            self.origin, self.session, self.nonce
        )
    }

    fn init(&self, method: &str) -> Result<RequestInit> {
        let init = RequestInit::new();
        init.set_method(method);
        init.set_cache(RequestCache::NoStore);
        init.set_signal(Some(&self.controller.signal()));
        if let Some(auth) = &self.auth {
            let headers = Headers::new().map_err(js_err)?;
            headers
                .append("Authorization", &format!("Bearer {auth}"))
                .map_err(js_err)?;
            init.set_headers_headers(&headers);
        }
        Ok(init)
    }

    async fn request(&self, url: &str, init: &RequestInit, count_rx: bool) -> Result<Timed> {
        let t_start = now();
        let resp: Response = JsFuture::from(window().fetch_with_str_and_init(url, init))
            .await
            .map_err(|e| EngineError::Connect(js_err(e).to_string()))?
            .unchecked_into();
        let t_headers = now();
        if !resp.ok() {
            return Err(EngineError::Protocol(format!(
                "HTTP {} from {}",
                resp.status(),
                url.split('?').next().unwrap_or(url)
            )));
        }
        let server_ms = resp
            .headers()
            .get("Server-Timing")
            .ok()
            .flatten()
            .and_then(|h| parse_server_timing(&h));
        let received = resp
            .headers()
            .get("X-Received-Bytes")
            .ok()
            .flatten()
            .and_then(|h| h.trim().parse().ok());
        // Read the body without copying it into WASM: only byteLength.
        let mut body_bytes = 0u64;
        if let Some(body) = resp.body() {
            let reader: ReadableStreamDefaultReader = body.get_reader().unchecked_into();
            let done_key = JsValue::from_str("done");
            let value_key = JsValue::from_str("value");
            loop {
                let chunk = JsFuture::from(reader.read()).await.map_err(js_err)?;
                if Reflect::get(&chunk, &done_key)
                    .map(|d| d.is_truthy())
                    .unwrap_or(true)
                {
                    break;
                }
                if let Ok(v) = Reflect::get(&chunk, &value_key) {
                    let n = u64::from(v.unchecked_into::<Uint8Array>().byte_length());
                    body_bytes += n;
                    if count_rx {
                        self.rx_bytes.set(self.rx_bytes.get() + n);
                    }
                }
            }
        }
        let t_end = now();
        Ok(Timed {
            entry: find_entry(url).await,
            t_start,
            t_headers,
            t_end,
            server_ms,
            body_bytes,
            received,
        })
    }

    /// Idle or loaded latency: `GET __down?bytes=0`, time to first byte minus
    /// the server time.
    pub async fn ping(&self) -> Result<f64> {
        let url = self.url("/api/__down", "bytes=0");
        let t = self.request(&url, &self.init("GET")?, false).await?;
        let ttfb = match &t.entry {
            Some(e) if e.request_start() > 0.0 => e.response_start() - e.request_start(),
            _ => t.t_headers - t.t_start,
        };
        Ok((ttfb - t.server_ms.unwrap_or(0.0)).max(0.0))
    }

    pub fn progress(&self, download: bool) -> u64 {
        if download {
            self.rx_bytes.get()
        } else {
            self.tx_bytes.get()
        }
    }

    /// Downloads `bytes` over `streams` parallel requests:
    /// `responseStart → responseEnd` (earliest start to latest end).
    pub async fn download(&self, bytes: u64, streams: u8) -> Result<Sample> {
        let parts = split_bytes(bytes, streams);
        let reqs = parts.iter().map(|&n| async move {
            let url = self.url("/api/__down", &format!("bytes={n}"));
            let t = self.request(&url, &self.init("GET")?, true).await?;
            if t.body_bytes != n {
                return Err(EngineError::Protocol(format!(
                    "download: received {} of {n} bytes",
                    t.body_bytes
                )));
            }
            Ok(t)
        });
        let timed = try_join_all(reqs).await?;
        let mut first = f64::INFINITY;
        let mut last = f64::NEG_INFINITY;
        let mut server_ms: f64 = 0.0;
        for t in &timed {
            let (a, b) = match &t.entry {
                Some(e) if e.response_start() > 0.0 => (e.response_start(), e.response_end()),
                _ => (t.t_headers, t.t_end),
            };
            first = first.min(a);
            last = last.max(b);
            server_ms = server_ms.max(t.server_ms.unwrap_or(0.0));
        }
        let duration = last - first;
        Ok(Sample {
            duration_ms: if duration > 0.0 { duration } else { server_ms },
            server_ms: timed
                .iter()
                .any(|t| t.server_ms.is_some())
                .then_some(server_ms),
            client_ms: None,
            round_trips: 0,
        })
    }

    /// Uploads `bytes` over `streams` parallel streams. Each stream sends its share as
    /// back-to-back requests of at most [`HTTP_UPLOAD_CHUNK_BYTES`], built from the same
    /// random buffer, so memory stays small at any size (a request body is a Blob, which
    /// browsers copy in full; 1 GB in one Blob fails with `ERR_BLOB_OUT_OF_MEMORY`).
    ///
    /// Duration: per stream, the sum of the server's body-read times from `Server-Timing`
    /// (see [`http_upload_duration_ms`]), so the gaps between requests don't count; the
    /// longest stream when parallel.
    pub async fn upload(&self, bytes: u64, streams: u8) -> Result<Sample> {
        let parts = split_bytes(bytes, streams);
        let reqs = parts.iter().map(|&n| async move {
            let mut duration = 0.0;
            let mut server: Option<f64> = None;
            let mut client = 0.0;
            let mut requests = 0u32;
            for size in chunk_sizes(n, HTTP_UPLOAD_CHUNK_BYTES) {
                let url = self.url("/api/__up", "");
                let init = self.init("POST")?;
                let blob = self.upload.blob(size)?;
                init.set_body(&blob);
                let t = self.request(&url, &init, false).await?;
                // A body cut short must not count as a fast upload.
                if let Some(got) = t.received
                    && got != size
                {
                    return Err(EngineError::Protocol(format!(
                        "upload: server received {got} of {size} bytes"
                    )));
                }
                self.tx_bytes.set(self.tx_bytes.get() + size);
                let span = match &t.entry {
                    Some(e) if e.request_start() > 0.0 => e.response_start() - e.request_start(),
                    _ => t.t_headers - t.t_start,
                };
                duration += http_upload_duration_ms(span, t.server_ms);
                client += span;
                requests += 1;
                if let Some(ms) = t.server_ms {
                    server = Some(server.unwrap_or(0.0) + ms);
                }
            }
            Ok::<_, EngineError>((duration, server, client, requests))
        });
        let per_stream = try_join_all(reqs).await?;
        let duration = per_stream.iter().map(|s| s.0).fold(0.0, f64::max);
        let server_max = per_stream.iter().filter_map(|s| s.1).reduce(f64::max);
        // The client's view of the slowest stream; each request's `requestStart →
        // responseStart` includes one round trip.
        let (client_ms, round_trips) = per_stream
            .iter()
            .map(|s| (s.2, s.3))
            .fold((0.0, 0), |a, b| if b.0 > a.0 { b } else { a });
        Ok(Sample {
            duration_ms: duration,
            server_ms: server_max,
            client_ms: (round_trips > 0).then_some(client_ms),
            round_trips,
        })
    }
}

impl Drop for HttpTransport {
    fn drop(&mut self) {
        // Cancel anything still in flight (abort, error).
        self.controller.abort();
    }
}

/// The resource timing entry of `url`; it can be queued a moment after the
/// body completes, so retry briefly.
async fn find_entry(url: &str) -> Option<PerformanceResourceTiming> {
    let perf = performance();
    for _ in 0..5 {
        let entries = perf.get_entries_by_name_with_entry_type(url, "resource");
        let n = entries.length();
        if n > 0 {
            return entries.get(n - 1).dyn_into().ok();
        }
        sleep(0).await;
    }
    None
}
