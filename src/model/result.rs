//! Result model (SPEC §6.3) and the pure reducers that turn raw measurements
//! into a [`Summary`] (SPEC §5.4–§5.7), plus group results (SPEC §5.11).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::profile::{Config, Direction, PhaseStatus, RampState, Step};
use super::protocol::{DeviceKind, Phase};
use super::scores::{self, AimInputs, Scores};
use super::stats;

/// Measurement transport for latency, download and upload.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    #[default]
    Websocket,
    Http,
}

/// Who ran the test. Fields are `None` when client info storage is off.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientInfo {
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
}

/// Other sessions moving data during a solo run (SPEC §5.11).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Concurrency {
    /// Most other active sessions seen at once.
    pub peers_max: u32,
    /// Time during which at least one other session moved data.
    pub overlap_ms: f64,
    /// `overlap_ms / duration_ms`, in `0.0..=1.0`.
    pub overlap_ratio: f64,
}

impl Concurrency {
    /// True if any other session overlapped this run.
    pub fn overlapped(&self) -> bool {
        self.peers_max > 0 && self.overlap_ms > 0.0
    }
}

/// Reduced headline numbers. `None` = not measured / unavailable.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Summary {
    pub download_bps: Option<f64>,
    pub upload_bps: Option<f64>,
    pub latency_ms: Option<f64>,
    pub jitter_ms: Option<f64>,
    pub down_loaded_latency_ms: Option<f64>,
    pub down_loaded_jitter_ms: Option<f64>,
    pub up_loaded_latency_ms: Option<f64>,
    pub up_loaded_jitter_ms: Option<f64>,
    /// `None` = unavailable (WebRTC failed or not run), not 0%.
    pub packet_loss_ratio: Option<f64>,
    /// Timed tests only (SPEC §5.4): the highest throughput over any 1 s of the timed
    /// download, from the live byte counter ([`peak_bps`]). `None` for size-based runs.
    pub download_peak_bps: Option<f64>,
    /// Timed tests only: bytes moved over the timed download window, after its first
    /// second (ramp-up), divided by that time ([`window_avg_bps`]).
    pub download_avg_bps: Option<f64>,
    pub upload_peak_bps: Option<f64>,
    pub upload_avg_bps: Option<f64>,
}

impl Summary {
    /// `max(down_loaded, up_loaded) − idle latency`.
    pub fn loaded_latency_increase_ms(&self) -> Option<f64> {
        scores::loaded_latency_increase(
            self.latency_ms,
            self.down_loaded_latency_ms,
            self.up_loaded_latency_ms,
        )
    }

    /// Inputs for the AIM scoring.
    pub fn aim_inputs(&self) -> AimInputs {
        AimInputs {
            latency_ms: self.latency_ms,
            jitter_ms: self.jitter_ms,
            loaded_latency_increase_ms: self.loaded_latency_increase_ms(),
            packet_loss_ratio: self.packet_loss_ratio,
            download_bps: self.download_bps,
            upload_bps: self.upload_bps,
        }
    }

    /// AIM scores for this summary.
    pub fn scores(&self) -> Scores {
        scores::compute(&self.aim_inputs())
    }
}

/// One download or upload request.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BandwidthPoint {
    pub bytes: u64,
    /// `bytes × 8 / duration`.
    pub bps: f64,
    /// The duration `bps` is based on: client first-to-last byte for
    /// downloads, server time for uploads (SPEC §5.5).
    pub duration_ms: f64,
    /// Server-reported time, if any.
    #[serde(default)]
    pub server_ms: Option<f64>,
}

impl BandwidthPoint {
    /// Builds a point, computing `bps` from `bytes` and `duration_ms`
    /// (`bps` is 0 for a non-positive duration).
    pub fn new(bytes: u64, duration_ms: f64, server_ms: Option<f64>) -> Self {
        let bps = if duration_ms > 0.0 {
            bytes as f64 * 8.0 / (duration_ms / 1000.0)
        } else {
            0.0
        };
        Self {
            bytes,
            bps,
            duration_ms,
            server_ms,
        }
    }
}

/// Raw packet loss counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PacketLossCounts {
    pub sent: u32,
    pub received: u32,
    pub lost: u32,
}

impl PacketLossCounts {
    /// Counts from sent/received (`lost = sent − received`, saturating).
    pub fn new(sent: u32, received: u32) -> Self {
        Self {
            sent,
            received,
            lost: sent.saturating_sub(received),
        }
    }

    /// `lost / sent`; `None` if nothing was sent.
    pub fn ratio(&self) -> Option<f64> {
        (self.sent > 0).then(|| f64::from(self.lost) / f64::from(self.sent))
    }
}

/// Raw points kept with a run (for charts and the detail view).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Points {
    pub latency: Vec<f64>,
    pub download: Vec<BandwidthPoint>,
    pub upload: Vec<BandwidthPoint>,
    pub down_loaded_latency: Vec<f64>,
    pub up_loaded_latency: Vec<f64>,
    pub packet_loss: Option<PacketLossCounts>,
    /// Throughput over time from the live byte counter, at most
    /// [`MAX_SERIES_POINTS`] points per direction (see [`rate_series`]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub download_series: Vec<RatePoint>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub upload_series: Vec<RatePoint>,
}

/// One point of a run's throughput-over-time series.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RatePoint {
    /// Milliseconds since the run started (end of the interval).
    pub t_ms: u32,
    pub bps: f64,
}

/// One reading of a direction's cumulative byte counter during a transfer step.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ProgressSample {
    /// Milliseconds since the run started.
    pub t_ms: f64,
    /// Bytes moved in this direction since the run started.
    pub bytes: u64,
}

/// The scheduled window of a timed transfer step (ms since the run started).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TimedWindow {
    pub direction: Direction,
    pub start_ms: f64,
    pub end_ms: f64,
}

/// Most points per direction in [`Points::download_series`] / `upload_series`.
pub const MAX_SERIES_POINTS: usize = 600;
/// Narrowest interval of a stored series point.
pub const SERIES_MIN_INTERVAL_MS: f64 = 250.0;
/// Consecutive counter readings further apart than this start a new segment
/// (between steps): no rate is computed across the gap.
pub const SERIES_GAP_MS: f64 = 1000.0;
/// Window of the peak throughput.
pub const PEAK_WINDOW_MS: f64 = 1000.0;
/// The start of a timed window left out of the average (TCP ramp-up and the
/// request size growth); at most a fifth of the window.
pub const AVG_RAMP_SKIP_MS: f64 = 1000.0;

/// Downsamples cumulative counter readings into a rate series of at most
/// `max_points` points. Each point is the rate since the previous point, over at
/// least [`SERIES_MIN_INTERVAL_MS`] (wider for long runs, so the cap holds).
pub fn rate_series(samples: &[ProgressSample], max_points: usize) -> Vec<RatePoint> {
    let (Some(first), Some(last)) = (samples.first(), samples.last()) else {
        return Vec::new();
    };
    let span = (last.t_ms - first.t_ms).max(0.0);
    let width = (span / max_points.max(1) as f64).max(SERIES_MIN_INTERVAL_MS);
    let mut out = Vec::new();
    let mut anchor = *first;
    let mut prev = *first;
    for s in &samples[1..] {
        if s.t_ms - prev.t_ms > SERIES_GAP_MS || s.bytes < prev.bytes {
            anchor = *s;
        } else if s.t_ms - anchor.t_ms >= width {
            let secs = (s.t_ms - anchor.t_ms) / 1000.0;
            out.push(RatePoint {
                t_ms: s.t_ms.round().max(0.0) as u32,
                bps: s.bytes.saturating_sub(anchor.bytes) as f64 * 8.0 / secs,
            });
            anchor = *s;
        }
        prev = *s;
    }
    if out.len() > max_points {
        // Gaps can push the count slightly over the cap: thin evenly.
        let step = out.len().div_ceil(max_points.max(1));
        out = out.into_iter().step_by(step).collect();
    }
    out
}

/// The highest rate over any [`PEAK_WINDOW_MS`] (or slightly longer, at the
/// readings' spacing) within `[start_ms, end_ms]`. `None` when the readings
/// span less than one window.
pub fn peak_bps(samples: &[ProgressSample], start_ms: f64, end_ms: f64) -> Option<f64> {
    let inside: Vec<&ProgressSample> = samples
        .iter()
        .filter(|s| s.t_ms >= start_ms && s.t_ms <= end_ms)
        .collect();
    let mut best: Option<f64> = None;
    let mut j = 0;
    for i in 1..inside.len() {
        // The latest reading at least one window before reading `i`.
        while j + 1 < i && inside[i].t_ms - inside[j + 1].t_ms >= PEAK_WINDOW_MS {
            j += 1;
        }
        let dt = inside[i].t_ms - inside[j].t_ms;
        if dt >= PEAK_WINDOW_MS {
            let bytes = inside[i].bytes.saturating_sub(inside[j].bytes);
            let bps = bytes as f64 * 8.0 / (dt / 1000.0);
            best = Some(best.map_or(bps, |b| b.max(bps)));
        }
    }
    best
}

/// Bytes moved between the first reading after the ramp-up
/// (`start_ms + min(AVG_RAMP_SKIP_MS, window / 5)`) and the last reading at or
/// before `end_ms`, divided by the time between them. `None` with less than
/// half a second of readings.
pub fn window_avg_bps(samples: &[ProgressSample], start_ms: f64, end_ms: f64) -> Option<f64> {
    let skip = AVG_RAMP_SKIP_MS.min((end_ms - start_ms) / 5.0).max(0.0);
    let a = samples.iter().find(|s| s.t_ms >= start_ms + skip)?;
    let b = samples.iter().rev().find(|s| s.t_ms <= end_ms)?;
    let dt = b.t_ms - a.t_ms;
    (dt >= 500.0).then(|| b.bytes.saturating_sub(a.bytes) as f64 * 8.0 / (dt / 1000.0))
}

/// What happened to one profile step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhaseRecord {
    pub kind: Phase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    pub count: u32,
    pub status: PhaseStatus,
    /// Scheduled length of a timed step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u32>,
}

impl PhaseRecord {
    pub fn from_step(step: &Step, status: PhaseStatus) -> Self {
        Self {
            kind: step.phase(),
            bytes: step.bytes(),
            count: step.count(),
            status,
            duration_ms: step.duration_ms(),
        }
    }
}

impl RunResult {
    /// Download (and upload) time of a timed run, from its phases.
    pub fn timed_direction_ms(&self) -> Option<u32> {
        self.phases
            .iter()
            .find(|p| p.kind == Phase::Download)
            .and_then(|p| p.duration_ms)
    }
}

impl RampState {
    /// One record per step; steps without a status count as skipped.
    pub fn phase_records(&self) -> Vec<PhaseRecord> {
        self.steps()
            .iter()
            .enumerate()
            .map(|(i, s)| PhaseRecord::from_step(s, self.status(i).unwrap_or(PhaseStatus::Skipped)))
            .collect()
    }
}

/// A complete test run (SPEC §6.3), stored as `detail_json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunResult {
    /// UUIDv7 (time-ordered).
    pub id: Uuid,
    /// RFC 3339 UTC timestamp.
    pub created_at: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    pub transport: Transport,
    /// Profile name.
    pub profile: String,
    pub streams: u8,
    #[serde(default)]
    pub client: ClientInfo,
    #[serde(default)]
    pub tls: bool,
    #[serde(default)]
    pub group_id: Option<Uuid>,
    #[serde(default)]
    pub concurrency: Concurrency,
    pub summary: Summary,
    #[serde(default)]
    pub scores: Scores,
    #[serde(default)]
    pub points: Points,
    #[serde(default)]
    pub phases: Vec<PhaseRecord>,
    pub duration_ms: u64,
}

/// A bandwidth sample plus whether it bypasses the minimum-duration filter.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BandwidthSample {
    pub point: BandwidthPoint,
    #[serde(default)]
    pub bypass_min_duration: bool,
}

/// A latency probe taken during a transfer.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LoadedLatencySample {
    pub rtt_ms: f64,
    /// How long the concurrent transfer had been running when the probe was sent.
    pub transfer_elapsed_ms: f64,
}

/// Accumulates raw measurements during a run. Pure; reduce with [`summarize`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Measurements {
    /// Idle latency RTTs in arrival order.
    pub latency: Vec<f64>,
    pub download: Vec<BandwidthSample>,
    pub upload: Vec<BandwidthSample>,
    pub down_loaded_latency: Vec<LoadedLatencySample>,
    pub up_loaded_latency: Vec<LoadedLatencySample>,
    /// `None` = packet loss unavailable.
    pub packet_loss: Option<PacketLossCounts>,
    /// Cumulative byte counter readings during download / upload steps.
    pub download_progress: Vec<ProgressSample>,
    pub upload_progress: Vec<ProgressSample>,
    /// Windows of the timed transfer steps.
    pub timed_windows: Vec<TimedWindow>,
}

impl Measurements {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_latency(&mut self, rtt_ms: f64) {
        self.latency.push(rtt_ms);
    }

    pub fn push_bandwidth(
        &mut self,
        dir: Direction,
        point: BandwidthPoint,
        bypass_min_duration: bool,
    ) {
        let sample = BandwidthSample {
            point,
            bypass_min_duration,
        };
        match dir {
            Direction::Download => self.download.push(sample),
            Direction::Upload => self.upload.push(sample),
        }
    }

    pub fn push_loaded_latency(&mut self, dir: Direction, rtt_ms: f64, transfer_elapsed_ms: f64) {
        let sample = LoadedLatencySample {
            rtt_ms,
            transfer_elapsed_ms,
        };
        match dir {
            Direction::Download => self.down_loaded_latency.push(sample),
            Direction::Upload => self.up_loaded_latency.push(sample),
        }
    }

    pub fn set_packet_loss(&mut self, counts: Option<PacketLossCounts>) {
        self.packet_loss = counts;
    }

    /// Records the cumulative byte counter of `dir` at `t_ms` (since the run started).
    pub fn push_progress(&mut self, dir: Direction, t_ms: f64, bytes: u64) {
        let sample = ProgressSample { t_ms, bytes };
        match dir {
            Direction::Download => self.download_progress.push(sample),
            Direction::Upload => self.upload_progress.push(sample),
        }
    }

    /// Records the scheduled window of a timed transfer step.
    pub fn push_timed_window(&mut self, direction: Direction, start_ms: f64, end_ms: f64) {
        self.timed_windows.push(TimedWindow {
            direction,
            start_ms,
            end_ms,
        });
    }

    fn progress(&self, dir: Direction) -> &[ProgressSample] {
        match dir {
            Direction::Download => &self.download_progress,
            Direction::Upload => &self.upload_progress,
        }
    }

    /// Peak and average throughput over the first timed window of `dir`;
    /// `(None, None)` without one (size-based runs).
    pub fn timed_rates(&self, dir: Direction) -> (Option<f64>, Option<f64>) {
        let Some(w) = self.timed_windows.iter().find(|w| w.direction == dir) else {
            return (None, None);
        };
        let p = self.progress(dir);
        (
            peak_bps(p, w.start_ms, w.end_ms),
            window_avg_bps(p, w.start_ms, w.end_ms),
        )
    }

    fn samples(&self, dir: Direction) -> &[BandwidthSample] {
        match dir {
            Direction::Download => &self.download,
            Direction::Upload => &self.upload,
        }
    }

    /// Bandwidth values (bps) that count: duration ≥ `bandwidth_min_request_ms`
    /// or from a bypassing step.
    ///
    /// On a very fast link every request can finish under `bandwidth_min_request_ms`. Rather
    /// than reporting nothing, the samples of the longest-running request size are used then.
    pub fn valid_bandwidth(&self, dir: Direction, config: &Config) -> Vec<f64> {
        let samples = self.samples(dir);
        let valid: Vec<f64> = samples
            .iter()
            .filter(|s| {
                s.bypass_min_duration || s.point.duration_ms >= config.bandwidth_min_request_ms
            })
            .map(|s| s.point.bps)
            .collect();
        if !valid.is_empty() {
            return valid;
        }
        let Some(max_bytes) = samples
            .iter()
            .filter(|s| s.point.duration_ms > 0.0 && s.point.bps.is_finite())
            .map(|s| s.point.bytes)
            .max()
        else {
            return Vec::new();
        };
        samples
            .iter()
            .filter(|s| s.point.bytes == max_bytes && s.point.duration_ms > 0.0)
            .map(|s| s.point.bps)
            .filter(|bps| bps.is_finite())
            .collect()
    }

    /// Loaded RTTs that count: taken once the transfer ran for
    /// ≥ `loaded_request_min_ms`, keeping only the latest
    /// `loaded_latency_max_points`.
    pub fn loaded_latency(&self, dir: Direction, config: &Config) -> Vec<f64> {
        let src = match dir {
            Direction::Download => &self.down_loaded_latency,
            Direction::Upload => &self.up_loaded_latency,
        };
        let valid: Vec<f64> = src
            .iter()
            .filter(|s| s.transfer_elapsed_ms >= config.loaded_request_min_ms)
            .map(|s| s.rtt_ms)
            .collect();
        let skip = valid.len().saturating_sub(config.loaded_latency_max_points);
        valid[skip..].to_vec()
    }

    /// Raw points for the stored result (loaded latency already filtered).
    pub fn points(&self, config: &Config) -> Points {
        Points {
            latency: self.latency.clone(),
            download: self.download.iter().map(|s| s.point).collect(),
            upload: self.upload.iter().map(|s| s.point).collect(),
            down_loaded_latency: self.loaded_latency(Direction::Download, config),
            up_loaded_latency: self.loaded_latency(Direction::Upload, config),
            packet_loss: self.packet_loss,
            download_series: rate_series(&self.download_progress, MAX_SERIES_POINTS),
            upload_series: rate_series(&self.upload_progress, MAX_SERIES_POINTS),
        }
    }
}

/// Applies the reducers: p90 bandwidth over valid samples, p50 latency,
/// jitter, and filtered loaded latency (percentiles come from `config`).
pub fn summarize(m: &Measurements, config: &Config) -> Summary {
    let bw = |dir| stats::percentile(&m.valid_bandwidth(dir, config), config.bandwidth_percentile);
    let lat = |v: &[f64]| stats::percentile(v, config.latency_percentile);
    let down_loaded = m.loaded_latency(Direction::Download, config);
    let up_loaded = m.loaded_latency(Direction::Upload, config);
    let (download_peak_bps, download_avg_bps) = m.timed_rates(Direction::Download);
    let (upload_peak_bps, upload_avg_bps) = m.timed_rates(Direction::Upload);
    Summary {
        download_bps: bw(Direction::Download),
        upload_bps: bw(Direction::Upload),
        latency_ms: lat(&m.latency),
        jitter_ms: stats::jitter(&m.latency),
        down_loaded_latency_ms: lat(&down_loaded),
        down_loaded_jitter_ms: stats::jitter(&down_loaded),
        up_loaded_latency_ms: lat(&up_loaded),
        up_loaded_jitter_ms: stats::jitter(&up_loaded),
        packet_loss_ratio: m.packet_loss.and_then(|c| c.ratio()),
        download_peak_bps,
        download_avg_bps,
        upload_peak_bps,
        upload_avg_bps,
    }
}

/// Aggregate ≥ this fraction of the server link triggers the bottleneck warning.
pub const BOTTLENECK_RATIO: f64 = 0.9;

/// One point of a device's server-measured throughput time series.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RateSample {
    /// Milliseconds since the start of the group window.
    pub t_ms: u32,
    pub down_bps: f64,
    pub up_bps: f64,
}

/// Server-side measurements of one group member, input to [`GroupResult::compute`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupMemberInput {
    pub session: Uuid,
    pub device_name: String,
    pub device_kind: DeviceKind,
    #[serde(default)]
    pub label: Option<String>,
    pub download_bps: Option<f64>,
    pub upload_bps: Option<f64>,
    pub down_loaded_latency_ms: Option<f64>,
    pub up_loaded_latency_ms: Option<f64>,
    /// Dropped out before the window ended.
    pub incomplete: bool,
    #[serde(default)]
    pub series: Vec<RateSample>,
    /// Highest 1 s rate within the download / upload window.
    #[serde(default)]
    pub download_peak_bps: Option<f64>,
    #[serde(default)]
    pub upload_peak_bps: Option<f64>,
}

/// A member's result within a group test.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupMember {
    pub session: Uuid,
    pub device_name: String,
    pub device_kind: DeviceKind,
    #[serde(default)]
    pub label: Option<String>,
    pub download_bps: Option<f64>,
    pub upload_bps: Option<f64>,
    /// Fraction (`0.0..=1.0`) of the aggregate; `None` if incomplete.
    pub download_share: Option<f64>,
    pub upload_share: Option<f64>,
    pub down_loaded_latency_ms: Option<f64>,
    pub up_loaded_latency_ms: Option<f64>,
    pub incomplete: bool,
    #[serde(default)]
    pub series: Vec<RateSample>,
    /// Highest 1 s rate within the download / upload window (`download_bps` is the
    /// average over the window, after its first second).
    #[serde(default)]
    pub download_peak_bps: Option<f64>,
    #[serde(default)]
    pub upload_peak_bps: Option<f64>,
}

/// Result of a synchronized group test (SPEC §5.11), computed by the server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupResult {
    pub group_id: Uuid,
    /// RFC 3339 UTC timestamp.
    pub created_at: String,
    /// Length of the common measurement window.
    pub window_ms: u64,
    pub members: Vec<GroupMember>,
    /// Sum over members that completed the window.
    pub aggregate_download_bps: Option<f64>,
    pub aggregate_upload_bps: Option<f64>,
    /// Jain's index over complete members.
    pub fairness_download: Option<f64>,
    pub fairness_upload: Option<f64>,
    /// Server interface speed, if known.
    #[serde(default)]
    pub server_link_bps: Option<f64>,
    /// Aggregate ≥ 90% of the server link: the server may be the bottleneck.
    #[serde(default)]
    pub bottleneck_warning: bool,
}

impl GroupResult {
    /// Computes aggregates, shares, fairness and the bottleneck flag.
    /// Incomplete members are listed but excluded from all of these.
    pub fn compute(
        group_id: Uuid,
        created_at: String,
        window_ms: u64,
        members: Vec<GroupMemberInput>,
        server_link_bps: Option<f64>,
    ) -> Self {
        fn complete_values(
            members: &[GroupMemberInput],
            f: impl Fn(&GroupMemberInput) -> Option<f64>,
        ) -> Vec<f64> {
            members
                .iter()
                .filter(|m| !m.incomplete)
                .filter_map(f)
                .filter(|x| x.is_finite())
                .collect()
        }
        let downs = complete_values(&members, |m| m.download_bps);
        let ups = complete_values(&members, |m| m.upload_bps);
        let total = |v: &[f64]| (!v.is_empty()).then(|| v.iter().sum::<f64>());
        let agg_down = total(&downs);
        let agg_up = total(&ups);
        let share = |incomplete: bool, value: Option<f64>, agg: Option<f64>| {
            if incomplete {
                return None;
            }
            match (value, agg) {
                (Some(v), Some(a)) if a > 0.0 && v.is_finite() => Some(v / a),
                _ => None,
            }
        };
        let bottleneck_warning = server_link_bps.is_some_and(|link| {
            link > 0.0
                && agg_down
                    .into_iter()
                    .chain(agg_up)
                    .any(|a| a >= BOTTLENECK_RATIO * link)
        });

        let members = members
            .into_iter()
            .map(|m| GroupMember {
                download_share: share(m.incomplete, m.download_bps, agg_down),
                upload_share: share(m.incomplete, m.upload_bps, agg_up),
                session: m.session,
                device_name: m.device_name,
                device_kind: m.device_kind,
                label: m.label,
                download_bps: m.download_bps,
                upload_bps: m.upload_bps,
                down_loaded_latency_ms: m.down_loaded_latency_ms,
                up_loaded_latency_ms: m.up_loaded_latency_ms,
                incomplete: m.incomplete,
                series: m.series,
                download_peak_bps: m.download_peak_bps,
                upload_peak_bps: m.upload_peak_bps,
            })
            .collect();

        Self {
            group_id,
            created_at,
            window_ms,
            members,
            aggregate_download_bps: agg_down,
            aggregate_upload_bps: agg_up,
            fairness_download: stats::jain_fairness(&downs),
            fairness_upload: stats::jain_fairness(&ups),
            server_link_bps,
            bottleneck_warning,
        }
    }

    /// True if any member dropped out.
    pub fn has_incomplete(&self) -> bool {
        self.members.iter().any(|m| m.incomplete)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::profile::Profile;
    use crate::model::scores::Classification;

    #[test]
    fn bandwidth_falls_back_to_largest_size_when_all_requests_are_too_short() {
        let config = Config::default();
        let mut m = Measurements::default();
        // Loopback-fast: every request finishes well under 10 ms.
        m.push_bandwidth(
            Direction::Upload,
            BandwidthPoint::new(100_000, 0.5, None),
            false,
        );
        m.push_bandwidth(
            Direction::Upload,
            BandwidthPoint::new(1_000_000, 2.0, None),
            false,
        );
        m.push_bandwidth(
            Direction::Upload,
            BandwidthPoint::new(1_000_000, 4.0, None),
            false,
        );
        let valid = m.valid_bandwidth(Direction::Upload, &config);
        assert_eq!(valid, vec![4.0e9, 2.0e9]);

        // As soon as one request is long enough, only qualifying samples count.
        m.push_bandwidth(
            Direction::Upload,
            BandwidthPoint::new(10_000_000, 20.0, None),
            false,
        );
        assert_eq!(m.valid_bandwidth(Direction::Upload, &config), vec![4.0e9]);
        assert!(m.valid_bandwidth(Direction::Download, &config).is_empty());
    }

    fn close(a: Option<f64>, b: f64) -> bool {
        a.is_some_and(|a| (a - b).abs() < 1e-6)
    }

    #[test]
    fn bandwidth_point_and_loss_counts() {
        let p = BandwidthPoint::new(1_000_000, 8.0, Some(7.5));
        assert_eq!(p.bps, 1e9);
        assert_eq!(BandwidthPoint::new(10, 0.0, None).bps, 0.0);
        let c = PacketLossCounts::new(1000, 990);
        assert_eq!(c.lost, 10);
        assert_eq!(c.ratio(), Some(0.01));
        assert_eq!(PacketLossCounts::new(0, 0).ratio(), None);
        assert_eq!(PacketLossCounts::new(5, 9).lost, 0);
    }

    #[test]
    fn bandwidth_filter_and_p90() {
        let cfg = Config::default();
        let mut m = Measurements::new();
        // Estimate: too short, but bypassed.
        m.push_bandwidth(
            Direction::Download,
            BandwidthPoint::new(100_000, 2.0, None),
            true,
        );
        // Too short, discarded.
        m.push_bandwidth(
            Direction::Download,
            BandwidthPoint::new(100_000, 5.0, None),
            false,
        );
        // Ten valid samples of 100..1000 Mbps (10 MB each).
        for i in 1..=10 {
            let bps = f64::from(i) * 1e8;
            let ms = 8e7 / bps * 1e3;
            m.push_bandwidth(
                Direction::Download,
                BandwidthPoint::new(10_000_000, ms, None),
                false,
            );
        }
        let valid = m.valid_bandwidth(Direction::Download, &cfg);
        assert_eq!(valid.len(), 11);
        assert_eq!(valid[0], 4e8);
        let s = summarize(&m, &cfg);
        // Sorted: 1,2,3,4(est),4,5,..,10 (×1e8); p90 index 9 → 9e8.
        assert!(close(s.download_bps, 9e8));
        assert_eq!(s.upload_bps, None);
    }

    #[test]
    fn latency_and_jitter() {
        let cfg = Config::default();
        let mut m = Measurements::new();
        for v in [3.0, 5.0, 4.0, 8.0] {
            m.push_latency(v);
        }
        let s = summarize(&m, &cfg);
        assert!(close(s.latency_ms, 4.5));
        assert!(close(s.jitter_ms, (2.0 + 1.0 + 4.0) / 3.0));
    }

    #[test]
    fn loaded_latency_filtering_keeps_latest_n() {
        let cfg = Config {
            loaded_latency_max_points: 3,
            ..Config::default()
        };
        let mut m = Measurements::new();
        // Too early in the transfer: ignored.
        m.push_loaded_latency(Direction::Download, 100.0, 249.0);
        for (i, rtt) in [10.0, 20.0, 30.0, 40.0, 50.0].into_iter().enumerate() {
            m.push_loaded_latency(Direction::Download, rtt, 250.0 + i as f64 * 400.0);
        }
        assert_eq!(
            m.loaded_latency(Direction::Download, &cfg),
            vec![30.0, 40.0, 50.0]
        );
        m.push_loaded_latency(Direction::Upload, 7.0, 300.0);
        let s = summarize(&m, &cfg);
        assert!(close(s.down_loaded_latency_ms, 40.0));
        assert!(close(s.down_loaded_jitter_ms, 10.0));
        assert!(close(s.up_loaded_latency_ms, 7.0));
        assert_eq!(s.up_loaded_jitter_ms, None);
        assert_eq!(m.points(&cfg).down_loaded_latency, vec![30.0, 40.0, 50.0]);
    }

    #[test]
    fn summary_scores() {
        let mut m = Measurements::new();
        for v in [2.0, 2.0, 2.0] {
            m.push_latency(v);
        }
        m.push_bandwidth(
            Direction::Download,
            BandwidthPoint::new(100_000_000, 1000.0, None),
            false,
        );
        m.push_bandwidth(
            Direction::Upload,
            BandwidthPoint::new(100_000_000, 1000.0, None),
            false,
        );
        m.push_loaded_latency(Direction::Download, 6.0, 500.0);
        m.push_loaded_latency(Direction::Upload, 4.0, 500.0);
        let s = summarize(&m, &Config::default());
        assert_eq!(s.packet_loss_ratio, None);
        assert!(close(s.loaded_latency_increase_ms(), 4.0));
        let sc = s.scores();
        // streaming: 20 (lat) + 0 (loss n/a) + 30 (800 Mbps) + 20 (lli 4) = 70
        assert_eq!(sc.streaming.unwrap().points, 70);
        assert_eq!(sc.streaming.unwrap().class, Classification::Great);
        assert_eq!(sc.rtc.unwrap().points, 20 + 10 + 20);
    }

    #[test]
    fn empty_measurements_summarize_to_nothing() {
        let s = summarize(&Measurements::new(), &Config::default());
        assert_eq!(s, Summary::default());
        assert_eq!(s.scores(), Scores::default());
    }

    #[test]
    fn phase_records_from_ramp() {
        let mut r = RampState::new(&Profile::quick(), &Config::default());
        let (i, _) = r.next_step(0.0).unwrap();
        r.complete_step(i, PhaseStatus::Done);
        let recs = r.phase_records();
        assert_eq!(recs.len(), 7);
        assert_eq!(
            recs[0],
            PhaseRecord {
                kind: Phase::Latency,
                bytes: None,
                count: 1,
                status: PhaseStatus::Done,
                duration_ms: None,
            }
        );
        // The second step (20 idle probes) hasn't run yet.
        assert_eq!(recs[1].kind, Phase::Latency);
        assert_eq!(recs[1].count, 20);
        assert_eq!(recs[1].status, PhaseStatus::Skipped);
        assert_eq!(recs[2].bytes, Some(100_000));
    }

    fn sample_run() -> RunResult {
        let cfg = Config::default();
        let mut m = Measurements::new();
        m.push_latency(1.5);
        m.push_latency(2.5);
        m.push_bandwidth(
            Direction::Download,
            BandwidthPoint::new(1_000_000, 20.0, Some(19.0)),
            false,
        );
        m.set_packet_loss(Some(PacketLossCounts::new(1000, 1000)));
        let summary = summarize(&m, &cfg);
        RunResult {
            id: Uuid::from_u128(42),
            created_at: "2026-10-10T08:00:00Z".into(),
            label: Some("Bedroom".into()),
            notes: None,
            transport: Transport::Websocket,
            profile: "quick".into(),
            streams: 1,
            client: ClientInfo {
                ip: Some("192.168.1.20".into()),
                user_agent: Some("UA".into()),
                device_id: Some("d1".into()),
                device_name: Some("iPhone".into()),
            },
            tls: false,
            group_id: None,
            concurrency: Concurrency {
                peers_max: 2,
                overlap_ms: 1200.0,
                overlap_ratio: 0.1,
            },
            scores: summary.scores(),
            summary,
            points: m.points(&cfg),
            phases: vec![PhaseRecord::from_step(
                &Step::download(1_000_000, 8),
                PhaseStatus::Done,
            )],
            duration_ms: 5123,
        }
    }

    #[test]
    fn run_result_round_trip() {
        let r = sample_run();
        assert!(r.concurrency.overlapped());
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains(r#""transport":"websocket""#));
        assert!(json.contains(r#""status":"done""#));
        assert!(json.contains(r#""packet_loss_ratio":0.0"#));
        assert_eq!(serde_json::from_str::<RunResult>(&json).unwrap(), r);
        assert_eq!(serde_json::to_string(&Transport::Http).unwrap(), "\"http\"");
    }

    #[test]
    fn run_result_minimal_json() {
        let r: RunResult = serde_json::from_str(
            r#"{"id":"00000000-0000-0000-0000-000000000001","created_at":"2026-10-10T00:00:00Z",
                "transport":"http","profile":"standard","streams":1,"summary":{},"duration_ms":1}"#,
        )
        .unwrap();
        assert_eq!(r.transport, Transport::Http);
        assert_eq!(r.summary, Summary::default());
        assert!(r.phases.is_empty());
        assert!(!r.concurrency.overlapped());
    }

    #[test]
    fn old_summary_json_has_no_timed_rates() {
        let s: Summary = serde_json::from_str(r#"{"download_bps":1e8,"upload_bps":2e7}"#).unwrap();
        assert_eq!(s.download_bps, Some(1e8));
        assert_eq!(s.download_peak_bps, None);
        assert_eq!(s.upload_avg_bps, None);
        let s2: Summary = serde_json::from_str(
            &serde_json::to_string(&Summary {
                download_peak_bps: Some(9e8),
                upload_avg_bps: Some(1e8),
                ..s
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(s2.download_peak_bps, Some(9e8));
        assert_eq!(s2.upload_avg_bps, Some(1e8));
        // Old points without a series.
        let p: Points = serde_json::from_str(r#"{"latency":[1.0]}"#).unwrap();
        assert!(p.download_series.is_empty());
        assert!(!serde_json::to_string(&p).unwrap().contains("series"));
    }

    /// Readings every 100 ms at `mbps(t)` Mbit/s.
    fn readings(from_ms: f64, to_ms: f64, mbps: impl Fn(f64) -> f64) -> Vec<ProgressSample> {
        let mut out = Vec::new();
        let mut bytes = 0.0;
        let mut t = from_ms;
        while t <= to_ms + 1e-9 {
            out.push(ProgressSample {
                t_ms: t,
                bytes: bytes as u64,
            });
            bytes += mbps(t) * 1e6 / 8.0 * 0.1;
            t += 100.0;
        }
        out
    }

    #[test]
    fn peak_and_average_of_a_timed_window() {
        // 2 s ramp at 100 Mbps, then 900 Mbps, with a 1.5 s burst at 1000 Mbps.
        let s = readings(1000.0, 11_000.0, |t| {
            if t < 2000.0 {
                100.0
            } else if (5000.0..6500.0).contains(&t) {
                1000.0
            } else {
                900.0
            }
        });
        let peak = peak_bps(&s, 1000.0, 11_000.0).unwrap();
        assert!((peak - 1e9).abs() < 1e6, "{peak}");
        // The first second (100 Mbps) is skipped: 8 s at 900 + 1.5 s at +100, over 9 s.
        let avg = window_avg_bps(&s, 1000.0, 11_000.0).unwrap();
        let expected = (9.0 * 900e6 + 1.5 * 100e6) / 9.0;
        assert!((avg - expected).abs() < 2e6, "{avg} vs {expected}");
        // Too short.
        assert_eq!(peak_bps(&s[..5], 0.0, 1e9), None);
        assert_eq!(window_avg_bps(&s[..3], 1000.0, 1300.0), None);
        assert_eq!(window_avg_bps(&[], 0.0, 1.0), None);
    }

    #[test]
    fn measurements_fill_timed_rates_only_for_timed_windows() {
        let mut m = Measurements::new();
        for s in readings(0.0, 5000.0, |_| 400.0) {
            m.push_progress(Direction::Download, s.t_ms, s.bytes);
        }
        let cfg = Config::default();
        let sized = summarize(&m, &cfg);
        assert_eq!(sized.download_peak_bps, None);
        assert_eq!(sized.download_avg_bps, None);
        m.push_timed_window(Direction::Download, 0.0, 5000.0);
        let timed = summarize(&m, &cfg);
        assert!((timed.download_peak_bps.unwrap() - 4e8).abs() < 1e6);
        assert!((timed.download_avg_bps.unwrap() - 4e8).abs() < 1e6);
        assert_eq!(timed.upload_peak_bps, None);
        let p = m.points(&cfg);
        assert_eq!(p.download_series.len(), 16);
        assert!(p.download_series.iter().all(|r| (r.bps - 4e8).abs() < 1e6));
        assert!(p.upload_series.is_empty());
    }

    #[test]
    fn rate_series_is_capped_and_splits_at_gaps() {
        // 10 minutes of readings every 100 ms.
        let s = readings(0.0, 600_000.0, |_| 100.0);
        let r = rate_series(&s, MAX_SERIES_POINTS);
        assert!(r.len() <= MAX_SERIES_POINTS && r.len() >= MAX_SERIES_POINTS - 2);
        assert!(r.iter().all(|p| (p.bps - 1e8).abs() < 1e6));
        // A gap between two steps produces no point spanning it.
        let mut a = readings(0.0, 1000.0, |_| 100.0);
        let last = a.last().unwrap().bytes;
        a.extend(
            readings(0.0, 1000.0, |_| 100.0)
                .into_iter()
                .map(|p| ProgressSample {
                    t_ms: p.t_ms + 5000.0,
                    bytes: p.bytes + last,
                }),
        );
        let r = rate_series(&a, 100);
        assert_eq!(r.len(), 6);
        assert!(r.iter().all(|p| (p.bps - 1e8).abs() < 1e6));
        assert!(rate_series(&[], 10).is_empty());
    }

    fn member(n: u128, down: f64, up: f64, incomplete: bool) -> GroupMemberInput {
        GroupMemberInput {
            session: Uuid::from_u128(n),
            device_name: format!("Device {n}"),
            device_kind: DeviceKind::Phone,
            label: None,
            download_bps: Some(down),
            upload_bps: Some(up),
            down_loaded_latency_ms: Some(10.0),
            up_loaded_latency_ms: None,
            incomplete,
            series: vec![RateSample {
                t_ms: 250,
                down_bps: down,
                up_bps: 0.0,
            }],
            download_peak_bps: None,
            upload_peak_bps: None,
        }
    }

    #[test]
    fn group_result_aggregates_shares_fairness() {
        let g = GroupResult::compute(
            Uuid::from_u128(1),
            "2026-10-10T00:00:00Z".into(),
            10_000,
            vec![
                member(1, 300e6, 100e6, false),
                member(2, 100e6, 100e6, false),
                member(3, 500e6, 500e6, true),
            ],
            Some(1e9),
        );
        assert!(close(g.aggregate_download_bps, 400e6));
        assert!(close(g.aggregate_upload_bps, 200e6));
        assert!(close(g.members[0].download_share, 0.75));
        assert!(close(g.members[1].download_share, 0.25));
        assert!(close(g.members[1].upload_share, 0.5));
        assert_eq!(g.members[2].download_share, None);
        assert!(g.has_incomplete());
        // (4)^2 / (2 * (9 + 1)) = 0.8
        assert!(close(g.fairness_download, 0.8));
        assert!(close(g.fairness_upload, 1.0));
        assert!(!g.bottleneck_warning);
        let json = serde_json::to_string(&g).unwrap();
        assert_eq!(serde_json::from_str::<GroupResult>(&json).unwrap(), g);
    }

    #[test]
    fn group_bottleneck_warning() {
        let mk = |link| {
            GroupResult::compute(
                Uuid::nil(),
                String::new(),
                10_000,
                vec![member(1, 450e6, 10e6, false), member(2, 450e6, 10e6, false)],
                link,
            )
        };
        assert!(mk(Some(1e9)).bottleneck_warning); // 900 Mbps = 90%
        assert!(!mk(Some(1.01e9)).bottleneck_warning);
        assert!(!mk(None).bottleneck_warning);
        assert!(!mk(Some(0.0)).bottleneck_warning);
    }

    #[test]
    fn group_all_incomplete() {
        let g = GroupResult::compute(
            Uuid::nil(),
            String::new(),
            1,
            vec![member(1, 1.0, 1.0, true)],
            None,
        );
        assert_eq!(g.aggregate_download_bps, None);
        assert_eq!(g.fairness_download, None);
        assert_eq!(g.members[0].download_share, None);
    }
}
