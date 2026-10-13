//! Platform-free helpers of the measurement engine. Compiled natively for the
//! unit tests and in the browser build.

use std::collections::VecDeque;

/// Size of the binary frames the client sends (SPEC §5.5).
pub const CHUNK_BYTES: usize = 64 * 1024;

/// Upload pacing: keep at most this many bytes queued in a socket (SPEC §5.5).
pub const MAX_BUFFERED_BYTES: u32 = 8 * 1024 * 1024;

/// Size of the random upload buffer (a multiple of [`CHUNK_BYTES`]).
pub const UPLOAD_BUFFER_BYTES: usize = 4 * 1024 * 1024;

/// Sliding window of the live throughput meter.
pub const LIVE_WINDOW_MS: f64 = 250.0;

/// Interval between live throughput updates.
pub const LIVE_INTERVAL_MS: u32 = 100;

/// Turns an `http(s)://host[:port]` origin into the `ws(s)://` URL of `path`.
/// A trailing slash on the origin is ignored.
pub fn ws_url(origin: &str, path: &str) -> String {
    let origin = origin.trim_end_matches('/');
    let rest = if let Some(r) = origin.strip_prefix("https://") {
        format!("wss://{r}")
    } else if let Some(r) = origin.strip_prefix("http://") {
        format!("ws://{r}")
    } else {
        origin.to_string()
    };
    format!("{rest}{path}")
}

/// Splits `total` bytes over `streams` sockets; the first sockets take the
/// remainder. Never returns zero-sized parts unless `total < streams`, in
/// which case only `total` sockets get one byte each.
pub fn split_bytes(total: u64, streams: u8) -> Vec<u64> {
    let s = u64::from(streams.max(1));
    let base = total / s;
    let rem = total % s;
    (0..s)
        .map(|i| base + u64::from(i < rem))
        .filter(|&b| b > 0)
        .collect()
}

/// Extracts the `dur` of the first metric in a `Server-Timing` header,
/// e.g. `app;dur=12.5` → `12.5`. Prefers a metric named `app`.
pub fn parse_server_timing(header: &str) -> Option<f64> {
    let mut first = None;
    for metric in header.split(',') {
        let mut parts = metric.split(';').map(str::trim);
        let name = parts.next().unwrap_or("");
        let dur = parts.find_map(|p| {
            let (k, v) = p.split_once('=')?;
            if k.trim().eq_ignore_ascii_case("dur") {
                v.trim().trim_matches('"').parse::<f64>().ok()
            } else {
                None
            }
        });
        if let Some(d) = dur.filter(|d| d.is_finite() && *d >= 0.0) {
            if name == "app" {
                return Some(d);
            }
            first.get_or_insert(d);
        }
    }
    first
}

/// Instantaneous throughput over a sliding window, fed with a cumulative byte
/// counter at a fixed interval (not per frame, so the hot path stays a counter
/// increment).
#[derive(Debug, Clone, Default)]
pub struct LiveMeter {
    samples: VecDeque<(f64, u64)>,
}

impl LiveMeter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the cumulative `bytes` at time `t_ms` and returns the bps over
    /// roughly the last [`LIVE_WINDOW_MS`], once two samples exist.
    pub fn push(&mut self, t_ms: f64, bytes: u64) -> Option<f64> {
        self.samples.push_back((t_ms, bytes));
        // Keep the newest sample at or before the window start as the anchor.
        while self.samples.len() > 2 && self.samples[1].0 <= t_ms - LIVE_WINDOW_MS {
            self.samples.pop_front();
        }
        let &(t0, b0) = self.samples.front()?;
        let dt = t_ms - t0;
        if dt <= 0.0 {
            return None;
        }
        let db = bytes.saturating_sub(b0);
        Some(db as f64 * 8.0 / (dt / 1000.0))
    }
}

/// Duration of an HTTP-mode upload (SPEC §5.8). This server's `Server-Timing`
/// on `POST /api/__up` is the time it spent reading the body (first to last
/// byte), i.e. the transfer itself, so it is used directly, as in WebSocket
/// mode. Without it, `requestStart → responseStart` is the client estimate.
pub fn http_upload_duration_ms(request_to_response_ms: f64, server_ms: Option<f64>) -> f64 {
    match server_ms {
        Some(s) if s > 0.0 => s,
        _ => request_to_response_ms,
    }
}

/// Largest HTTP upload request; bigger uploads are sent as several back-to-back requests.
pub const HTTP_UPLOAD_CHUNK_BYTES: u64 = 16 * 1024 * 1024;

/// `total` bytes as consecutive pieces of at most `chunk` bytes (nothing for 0).
pub fn chunk_sizes(total: u64, chunk: u64) -> impl Iterator<Item = u64> {
    let chunk = chunk.max(1);
    (0..total.div_ceil(chunk)).map(move |i| chunk.min(total - i * chunk))
}

/// Timed steps grow the request size while requests are shorter than this.
pub const TIMED_GROW_BELOW_MS: f64 = 250.0;

/// Smallest request size of a timed transfer.
pub const TIMED_MIN_BYTES: u64 = 64 * 1024;

/// Size of the next request of a timed transfer: double while requests take
/// less than [`TIMED_GROW_BELOW_MS`], but never more than the last request's
/// rate can move in the time left (with a [`TIMED_MIN_BYTES`] floor).
pub fn next_timed_size(size: u64, duration_ms: f64, left_ms: f64, cap: u64) -> u64 {
    let mut next = size;
    if duration_ms < TIMED_GROW_BELOW_MS {
        next = next.saturating_mul(2);
    }
    if duration_ms > 0.0 && left_ms > 0.0 {
        let fits = (size as f64 / duration_ms * left_ms) as u64;
        next = next.min(fits);
    }
    next.clamp(TIMED_MIN_BYTES.min(cap), cap.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_urls() {
        assert_eq!(
            ws_url("http://127.0.0.1:3400", "/ws/data"),
            "ws://127.0.0.1:3400/ws/data"
        );
        assert_eq!(
            ws_url("https://nas.lan/", "/ws/probe"),
            "wss://nas.lan/ws/probe"
        );
    }

    #[test]
    fn splits() {
        assert_eq!(split_bytes(10, 1), vec![10]);
        assert_eq!(split_bytes(10, 3), vec![4, 3, 3]);
        assert_eq!(split_bytes(2, 4), vec![1, 1]);
        assert_eq!(split_bytes(8, 0), vec![8]);
        assert_eq!(
            split_bytes(1_000_000_000, 4).iter().sum::<u64>(),
            1_000_000_000
        );
    }

    #[test]
    fn server_timing() {
        assert_eq!(parse_server_timing("app;dur=12.5"), Some(12.5));
        assert_eq!(parse_server_timing("db;dur=3, app;dur=7"), Some(7.0));
        assert_eq!(parse_server_timing("cache;desc=hit, x;dur=2"), Some(2.0));
        assert_eq!(parse_server_timing("app;dur=\"4\""), Some(4.0));
        assert_eq!(parse_server_timing("app"), None);
        assert_eq!(parse_server_timing(""), None);
        assert_eq!(parse_server_timing("app;dur=-1"), None);
    }

    #[test]
    fn live_meter_window() {
        let mut m = LiveMeter::new();
        assert_eq!(m.push(0.0, 0), None);
        // 1.25 MB per 100 ms = 100 Mbps
        let bps = m.push(100.0, 1_250_000).unwrap();
        assert!((bps - 1e8).abs() < 1.0);
        m.push(200.0, 2_500_000);
        m.push(300.0, 3_750_000);
        // Rate drops to zero: the window only reaches back ~300 ms.
        m.push(400.0, 3_750_000);
        m.push(500.0, 3_750_000);
        let bps = m.push(600.0, 3_750_000).unwrap();
        assert_eq!(bps, 0.0);
        assert!(m.samples.len() <= 5);
    }

    #[test]
    fn live_meter_same_time() {
        let mut m = LiveMeter::new();
        m.push(5.0, 10);
        assert_eq!(m.push(5.0, 20), None);
    }

    #[test]
    fn upload_duration() {
        assert_eq!(http_upload_duration_ms(100.0, Some(90.0)), 90.0);
        assert_eq!(http_upload_duration_ms(100.0, Some(0.0)), 100.0);
        assert_eq!(http_upload_duration_ms(100.0, None), 100.0);
    }

    #[test]
    fn timed_request_sizes_fit_the_time_left() {
        // Short requests double...
        assert_eq!(
            next_timed_size(1_000_000, 50.0, 10_000.0, u64::MAX),
            2_000_000
        );
        // ...but never past what fits in the time left (1 MB per 100 ms, 300 ms left).
        assert_eq!(
            next_timed_size(1_000_000, 100.0, 300.0, u64::MAX),
            2_000_000
        );
        assert_eq!(
            next_timed_size(4_000_000, 400.0, 300.0, u64::MAX),
            3_000_000
        );
        // Floor and cap.
        assert_eq!(
            next_timed_size(1_000_000, 1000.0, 1.0, u64::MAX),
            TIMED_MIN_BYTES
        );
        assert_eq!(
            next_timed_size(1_000_000, 10.0, 10_000.0, 1_500_000),
            1_500_000
        );
        assert_eq!(next_timed_size(10, 10.0, 10_000.0, 10), 10);
    }

    #[test]
    fn chunk_sizes_cover_the_total() {
        assert_eq!(chunk_sizes(0, 16).count(), 0);
        assert_eq!(chunk_sizes(10, 16).collect::<Vec<_>>(), vec![10]);
        assert_eq!(chunk_sizes(32, 16).collect::<Vec<_>>(), vec![16, 16]);
        assert_eq!(chunk_sizes(33, 16).collect::<Vec<_>>(), vec![16, 16, 1]);
        let gb = 1_000_000_000;
        let parts: Vec<u64> = chunk_sizes(gb, HTTP_UPLOAD_CHUNK_BYTES).collect();
        assert_eq!(parts.iter().sum::<u64>(), gb);
        assert!(parts.iter().all(|&p| p <= HTTP_UPLOAD_CHUNK_BYTES));
    }
}
