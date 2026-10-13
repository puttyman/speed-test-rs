//! Test profiles, engine configuration and the ramp planner (SPEC §5.3, §5.11).
//!
//! A [`Profile`] is an ordered list of [`Step`]s, defined as data so custom
//! sequences can be passed as JSON. [`RampState`] is a pure, deterministic
//! state machine that walks a profile, halts the size ramp per direction and
//! enforces the overall time cap.

use serde::{Deserialize, Serialize};

use super::protocol::Phase;

/// Maximum number of parallel data streams (SPEC §5.5).
pub const MAX_STREAMS: u8 = 8;

/// Transfer direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Download,
    Upload,
}

/// One step of a profile. Serialized tagged by `"type"`, e.g.
/// `{"type":"download","bytes":100000,"count":9}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Step {
    /// `num_packets` ping/pong probes.
    Latency { num_packets: u32 },
    /// `count` download requests of `bytes` each.
    Download {
        bytes: u64,
        count: u32,
        /// Count samples even if shorter than `bandwidth_min_request_ms`.
        #[serde(default, skip_serializing_if = "is_false")]
        bypass_min_duration: bool,
        /// Parallel streams for this step; `None` = [`Config::streams`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        streams: Option<u8>,
    },
    /// `count` upload requests of `bytes` each.
    Upload {
        bytes: u64,
        count: u32,
        #[serde(default, skip_serializing_if = "is_false")]
        bypass_min_duration: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        streams: Option<u8>,
    },
    /// WebRTC packet loss (SPEC §5.7).
    PacketLoss {
        num_packets: u32,
        batch_size: u32,
        batch_wait_ms: u32,
        responses_wait_ms: u32,
        connection_timeout_ms: u32,
    },
    /// Idle latency probes for a fixed time (group `concurrent` profile).
    TimedLatency { duration_ms: u32 },
    /// Continuous download for a fixed time, with loaded-latency probes.
    TimedDownload { duration_ms: u32 },
    /// Continuous upload for a fixed time, with loaded-latency probes.
    TimedUpload { duration_ms: u32 },
    /// Do nothing for a fixed time.
    Pause { duration_ms: u32 },
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Step {
    /// A download step without bypass and with the default stream count.
    pub const fn download(bytes: u64, count: u32) -> Self {
        Step::Download {
            bytes,
            count,
            bypass_min_duration: false,
            streams: None,
        }
    }

    /// An upload step without bypass and with the default stream count.
    pub const fn upload(bytes: u64, count: u32) -> Self {
        Step::Upload {
            bytes,
            count,
            bypass_min_duration: false,
            streams: None,
        }
    }

    /// Cloudflare's default packet loss step: 1000 packets, batches of 10,
    /// 10 ms apart, 3000 ms response wait, 5000 ms connection timeout.
    pub const fn default_packet_loss() -> Self {
        Step::PacketLoss {
            num_packets: 1000,
            batch_size: 10,
            batch_wait_ms: 10,
            responses_wait_ms: 3000,
            connection_timeout_ms: 5000,
        }
    }

    /// The phase this step measures.
    pub fn phase(&self) -> Phase {
        match self {
            Step::Latency { .. } | Step::TimedLatency { .. } => Phase::Latency,
            Step::Download { .. } | Step::TimedDownload { .. } => Phase::Download,
            Step::Upload { .. } | Step::TimedUpload { .. } => Phase::Upload,
            Step::PacketLoss { .. } => Phase::PacketLoss,
            Step::Pause { .. } => Phase::Pause,
        }
    }

    /// Transfer direction, for download and upload steps (sized or timed).
    pub fn direction(&self) -> Option<Direction> {
        match self {
            Step::Download { .. } | Step::TimedDownload { .. } => Some(Direction::Download),
            Step::Upload { .. } | Step::TimedUpload { .. } => Some(Direction::Upload),
            _ => None,
        }
    }

    /// Request size of a sized transfer step.
    pub fn bytes(&self) -> Option<u64> {
        match self {
            Step::Download { bytes, .. } | Step::Upload { bytes, .. } => Some(*bytes),
            _ => None,
        }
    }

    /// Number of requests, probes or packets (1 for timed steps).
    pub fn count(&self) -> u32 {
        match self {
            Step::Latency { num_packets } | Step::PacketLoss { num_packets, .. } => *num_packets,
            Step::Download { count, .. } | Step::Upload { count, .. } => *count,
            _ => 1,
        }
    }

    /// Duration of a time-based step.
    pub fn duration_ms(&self) -> Option<u32> {
        match self {
            Step::TimedLatency { duration_ms }
            | Step::TimedDownload { duration_ms }
            | Step::TimedUpload { duration_ms }
            | Step::Pause { duration_ms } => Some(*duration_ms),
            _ => None,
        }
    }

    /// Effective parallel streams of a transfer step, clamped to `1..=MAX_STREAMS`.
    pub fn streams(&self, config: &Config) -> u8 {
        let s = match self {
            Step::Download { streams, .. } | Step::Upload { streams, .. } => {
                streams.unwrap_or(config.streams)
            }
            _ => config.streams,
        };
        s.clamp(1, MAX_STREAMS)
    }

    /// Whether short samples of this step count anyway.
    pub fn bypass_min_duration(&self) -> bool {
        matches!(
            self,
            Step::Download {
                bypass_min_duration: true,
                ..
            } | Step::Upload {
                bypass_min_duration: true,
                ..
            }
        )
    }
}

/// A named, ordered sequence of steps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// `quick`, `standard`, `extended`, `timed`, `concurrent` or a custom name.
    pub name: String,
    pub steps: Vec<Step>,
}

const KB100: u64 = 100_000;
const MB1: u64 = 1_000_000;
const MB10: u64 = 10_000_000;
const MB25: u64 = 25_000_000;
const MB50: u64 = 50_000_000;
const MB100: u64 = 100_000_000;
const MB250: u64 = 250_000_000;
const GB1: u64 = 1_000_000_000;
const GB2: u64 = 2_000_000_000;

impl Profile {
    /// Names of the built-in profiles.
    pub const BUILTIN: [&'static str; 5] = ["quick", "standard", "extended", "timed", "concurrent"];

    /// Shortest and longest download (and upload) time of a user-chosen timed test.
    pub const TIMED_MIN_MS: u32 = 10_000;
    pub const TIMED_MAX_MS: u32 = 300_000;
    /// Download (and upload) time of `by_name("timed")`.
    pub const TIMED_DEFAULT_MS: u32 = 30_000;

    /// The default profile (SPEC §5.3, 20 steps incl. the LAN extension), in four blocks:
    /// idle latency, every download size (smallest first), every upload size, then packet
    /// loss. Sizes and counts match Cloudflare's sequence; only the order differs.
    pub fn standard() -> Self {
        let mut steps = Self::latency_steps();
        steps.extend(Self::download_steps());
        steps.extend(Self::upload_steps());
        steps.push(Step::default_packet_loss());
        Profile {
            name: "standard".into(),
            steps,
        }
    }

    /// Idle latency, downloads up to 1 MB, uploads up to 1 MB (about 5 s, no packet loss).
    pub fn quick() -> Self {
        let mut steps = Self::latency_steps();
        steps.extend(
            Self::download_steps()
                .into_iter()
                .filter(|s| s.bytes().is_some_and(|b| b <= MB1)),
        );
        steps.extend(
            Self::upload_steps()
                .into_iter()
                .filter(|s| s.bytes().is_some_and(|b| b <= MB1)),
        );
        Profile {
            name: "quick".into(),
            steps,
        }
    }

    /// `standard` with 4 parallel streams for the large transfers (50 MB and up).
    pub fn extended() -> Self {
        let mut steps = Self::standard().steps;
        for step in &mut steps {
            if let Step::Download { bytes, streams, .. } | Step::Upload { bytes, streams, .. } =
                step
                && *bytes >= MB50
            {
                *streams = Some(4);
            }
        }
        Profile {
            name: "extended".into(),
            steps,
        }
    }

    /// One unloaded probe, then 20 idle latency probes.
    fn latency_steps() -> Vec<Step> {
        vec![
            Step::Latency { num_packets: 1 },
            Step::Latency { num_packets: 20 },
        ]
    }

    /// Every download size, smallest first. The first 100 kB request warms the connection
    /// and counts even if very short.
    fn download_steps() -> Vec<Step> {
        vec![
            Step::Download {
                bytes: KB100,
                count: 1,
                bypass_min_duration: true,
                streams: None,
            },
            Step::download(KB100, 9),
            Step::download(MB1, 8),
            Step::download(MB10, 6),
            Step::download(MB25, 4),
            Step::download(MB100, 3),
            Step::download(MB250, 2),
            // Fast links only: ramp halting skips these unless the previous size finished in
            // under `bandwidth_finish_request_ms` (1 GB above ~2 Gbps, 2 GB above ~8 Gbps).
            Step::download(GB1, 2),
            Step::download(GB2, 2),
        ]
    }

    /// Every upload size, smallest first.
    fn upload_steps() -> Vec<Step> {
        vec![
            Step::upload(KB100, 8),
            Step::upload(MB1, 6),
            Step::upload(MB10, 4),
            Step::upload(MB25, 4),
            Step::upload(MB50, 3),
            Step::upload(MB250, 2),
            // Fast links only, as for downloads.
            Step::upload(GB1, 2),
            Step::upload(GB2, 2),
        ]
    }

    /// A fixed-duration test (the "Timed" test length, SPEC §5.3): idle latency as in
    /// `standard`, then `direction_ms` of continuous download, the same of continuous
    /// upload, then packet loss. `direction_ms` is clamped to
    /// [`TIMED_MIN_MS`](Self::TIMED_MIN_MS)`..=`[`TIMED_MAX_MS`](Self::TIMED_MAX_MS); use
    /// [`timed_unclamped`](Self::timed_unclamped) for shorter test-only runs.
    pub fn timed(direction_ms: u32) -> Self {
        Self::timed_unclamped(direction_ms.clamp(Self::TIMED_MIN_MS, Self::TIMED_MAX_MS))
    }

    /// [`timed`](Self::timed) without the clamp (at least 1 s per direction). Only for
    /// the test-only `?timed_ms=` override (SPEC §7.8).
    pub fn timed_unclamped(direction_ms: u32) -> Self {
        let direction_ms = direction_ms.clamp(1000, Self::TIMED_MAX_MS);
        let mut steps = Self::latency_steps();
        steps.push(Step::TimedDownload {
            duration_ms: direction_ms,
        });
        steps.push(Step::TimedUpload {
            duration_ms: direction_ms,
        });
        steps.push(Step::default_packet_loss());
        Profile {
            name: "timed".into(),
            steps,
        }
    }

    /// Download time of a timed profile (its first timed download), if it has one.
    pub fn timed_direction_ms(&self) -> Option<u32> {
        self.steps.iter().find_map(|s| match s {
            Step::TimedDownload { duration_ms } => Some(*duration_ms),
            _ => None,
        })
    }

    /// The time-based group profile (SPEC §5.11): 3 s idle latency, 10 s
    /// download, 2 s pause, 10 s upload. Packet loss is optional; append
    /// [`Step::default_packet_loss`] to include it.
    pub fn concurrent() -> Self {
        Self::concurrent_for(10_000)
    }

    /// The group profile with `direction_ms` of download and of upload. Plans shorter than
    /// 10 s per direction (CI, quick checks) use 1 s of latency and a 0.5 s pause.
    pub fn concurrent_for(direction_ms: u32) -> Self {
        let short = direction_ms < 10_000;
        Profile {
            name: "concurrent".into(),
            steps: vec![
                Step::TimedLatency {
                    duration_ms: if short { 1000 } else { 3000 },
                },
                Step::TimedDownload {
                    duration_ms: direction_ms,
                },
                Step::Pause {
                    duration_ms: if short { 500 } else { 2000 },
                },
                Step::TimedUpload {
                    duration_ms: direction_ms,
                },
            ],
        }
    }

    /// A built-in profile by name.
    pub fn by_name(name: &str) -> Option<Self> {
        match name {
            "quick" => Some(Self::quick()),
            "standard" => Some(Self::standard()),
            "extended" => Some(Self::extended()),
            "timed" => Some(Self::timed(Self::TIMED_DEFAULT_MS)),
            "concurrent" => Some(Self::concurrent()),
            _ => None,
        }
    }
}

impl Default for Profile {
    fn default() -> Self {
        Self::standard()
    }
}

/// Engine configuration (Cloudflare's defaults, SPEC §5.3–§5.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// A request this long halts larger sizes in its direction.
    pub bandwidth_finish_request_ms: f64,
    /// Shorter samples are discarded (unless the step bypasses this).
    pub bandwidth_min_request_ms: f64,
    /// Interval between loaded-latency probes.
    pub loaded_latency_throttle_ms: u32,
    /// A loaded probe counts once the transfer has run this long.
    pub loaded_request_min_ms: f64,
    /// Keep only the latest N loaded-latency points per direction.
    pub loaded_latency_max_points: usize,
    /// Percentile used to reduce latency samples (0.5 = median).
    pub latency_percentile: f64,
    /// Percentile used to reduce bandwidth samples.
    pub bandwidth_percentile: f64,
    /// Cap on measurement time for one test.
    pub max_test_ms: u64,
    /// Time budget for each direction's size-based transfers, so a slow download can't
    /// use up the time the upload needs.
    pub max_direction_ms: u64,
    /// Default parallel streams.
    pub streams: u8,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bandwidth_finish_request_ms: 1000.0,
            bandwidth_min_request_ms: 10.0,
            loaded_latency_throttle_ms: 400,
            loaded_request_min_ms: 250.0,
            loaded_latency_max_points: 20,
            latency_percentile: 0.5,
            bandwidth_percentile: 0.9,
            max_test_ms: 60_000,
            max_direction_ms: 25_000,
            streams: 1,
        }
    }
}

/// Final status of a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseStatus {
    Done,
    Skipped,
    Failed,
}

/// Steps the time cap never cuts: packet loss (bounded by its timeouts) and
/// steps with a fixed duration.
fn cap_exempt(step: &Step) -> bool {
    matches!(step, Step::PacketLoss { .. }) || step.duration_ms().is_some()
}

/// Deterministic planner that walks a profile (SPEC §5.3).
///
/// Usage: call [`next_step`](Self::next_step) with the elapsed measurement
/// time; run the returned step, feeding each finished transfer to
/// [`record_request`](Self::record_request) and checking
/// [`time_up`](Self::time_up) between requests; then call
/// [`complete_step`](Self::complete_step). Steps the planner passes over are
/// marked [`PhaseStatus::Skipped`].
///
/// **Ramp halting**: once any request in a direction takes at least
/// `bandwidth_finish_request_ms`, later steps in that direction with a larger
/// request size are skipped. The current step still runs to completion
/// (unless the time cap hits).
///
/// **Direction budget**: each direction's size-based transfers get
/// `max_direction_ms`, counted from its first transfer step. Once used up, the
/// rest of that direction is skipped (see [`time_up_for`](Self::time_up_for)).
///
/// **Time cap**: when `elapsed_ms >= max_test_ms`, every remaining step is
/// skipped, except packet loss, which is short and bounded by its own timeouts,
/// and timed steps (`TimedLatency`, `TimedDownload`, `TimedUpload`, `Pause`),
/// which are bounded by their own duration: a 5 min timed test runs in full.
#[derive(Debug, Clone, PartialEq)]
pub struct RampState {
    steps: Vec<Step>,
    status: Vec<Option<PhaseStatus>>,
    cursor: usize,
    halt_download: Option<u64>,
    halt_upload: Option<u64>,
    finish_request_ms: f64,
    max_test_ms: f64,
    max_direction_ms: f64,
    download_started: Option<f64>,
    upload_started: Option<f64>,
}

impl RampState {
    pub fn new(profile: &Profile, config: &Config) -> Self {
        Self {
            steps: profile.steps.clone(),
            status: vec![None; profile.steps.len()],
            cursor: 0,
            halt_download: None,
            halt_upload: None,
            finish_request_ms: config.bandwidth_finish_request_ms,
            max_test_ms: config.max_test_ms as f64,
            max_direction_ms: config.max_direction_ms as f64,
            download_started: None,
            upload_started: None,
        }
    }

    /// The profile's steps.
    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    /// Whether the time cap has been reached.
    pub fn time_up(&self, elapsed_ms: f64) -> bool {
        elapsed_ms >= self.max_test_ms
    }

    /// Whether `step` must stop now: the overall time cap, or (for a size-based
    /// transfer) its direction's budget. Always false for timed steps and packet
    /// loss, which are exempt from the cap.
    pub fn time_up_for(&self, step: &Step, elapsed_ms: f64) -> bool {
        !cap_exempt(step)
            && (self.time_up(elapsed_ms) || self.direction_budget_used(step, elapsed_ms))
    }

    fn direction_budget_used(&self, step: &Step, elapsed_ms: f64) -> bool {
        let started = match step {
            Step::Download { .. } => self.download_started,
            Step::Upload { .. } => self.upload_started,
            _ => return false,
        };
        started.is_some_and(|t| elapsed_ms - t >= self.max_direction_ms)
    }

    /// Whether `direction` has halted, and at which request size.
    pub fn halted_at(&self, direction: Direction) -> Option<u64> {
        match direction {
            Direction::Download => self.halt_download,
            Direction::Upload => self.halt_upload,
        }
    }

    fn should_skip(&self, step: &Step) -> bool {
        match (step.direction(), step.bytes()) {
            (Some(dir), Some(bytes)) => self.halted_at(dir).is_some_and(|h| bytes > h),
            _ => false,
        }
    }

    /// Advances to the next step to run, returning its index and definition.
    /// Returns `None` when the profile is finished or the time cap is reached
    /// (all remaining steps are then marked skipped).
    pub fn next_step(&mut self, elapsed_ms: f64) -> Option<(usize, &Step)> {
        let time_up = self.time_up(elapsed_ms);
        while self.cursor < self.steps.len() {
            let i = self.cursor;
            self.cursor += 1;
            if self.status[i].is_some() {
                continue;
            }
            let step = &self.steps[i];
            let exempt = cap_exempt(step);
            if (time_up && !exempt)
                || self.should_skip(step)
                || self.direction_budget_used(step, elapsed_ms)
            {
                self.status[i] = Some(PhaseStatus::Skipped);
                continue;
            }
            match step {
                Step::Download { .. } => {
                    self.download_started.get_or_insert(elapsed_ms);
                }
                Step::Upload { .. } => {
                    self.upload_started.get_or_insert(elapsed_ms);
                }
                _ => {}
            }
            return Some((i, &self.steps[i]));
        }
        None
    }

    /// Records one finished transfer of `bytes` that took `duration_ms`
    /// (client first-to-last byte for downloads, server time for uploads).
    pub fn record_request(&mut self, direction: Direction, bytes: u64, duration_ms: f64) {
        if duration_ms.is_nan() || duration_ms < self.finish_request_ms {
            return;
        }
        let slot = match direction {
            Direction::Download => &mut self.halt_download,
            Direction::Upload => &mut self.halt_upload,
        };
        *slot = Some(slot.map_or(bytes, |h| h.min(bytes)));
    }

    /// Sets the final status of step `index`.
    pub fn complete_step(&mut self, index: usize, status: PhaseStatus) {
        if let Some(s) = self.status.get_mut(index) {
            *s = Some(status);
        }
    }

    /// Marks every step without a status as skipped (time cap, abort).
    pub fn skip_remaining(&mut self) {
        for s in &mut self.status {
            s.get_or_insert(PhaseStatus::Skipped);
        }
        self.cursor = self.steps.len();
    }

    /// Status of step `index` (`None` = not finished yet).
    pub fn status(&self, index: usize) -> Option<PhaseStatus> {
        self.status.get(index).copied().flatten()
    }

    /// True once every step has a status.
    pub fn is_finished(&self) -> bool {
        self.status.iter().all(Option::is_some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_matches_spec_table() {
        let p = Profile::standard();
        assert_eq!(p.steps.len(), 20);
        let summary: Vec<(Phase, Option<u64>, u32)> = p
            .steps
            .iter()
            .map(|s| (s.phase(), s.bytes(), s.count()))
            .collect();
        use Phase::*;
        assert_eq!(
            summary,
            vec![
                (Latency, None, 1),
                (Latency, None, 20),
                (Download, Some(100_000), 1),
                (Download, Some(100_000), 9),
                (Download, Some(1_000_000), 8),
                (Download, Some(10_000_000), 6),
                (Download, Some(25_000_000), 4),
                (Download, Some(100_000_000), 3),
                (Download, Some(250_000_000), 2),
                (Download, Some(1_000_000_000), 2),
                (Download, Some(2_000_000_000), 2),
                (Upload, Some(100_000), 8),
                (Upload, Some(1_000_000), 6),
                (Upload, Some(10_000_000), 4),
                (Upload, Some(25_000_000), 4),
                (Upload, Some(50_000_000), 3),
                (Upload, Some(250_000_000), 2),
                (Upload, Some(1_000_000_000), 2),
                (Upload, Some(2_000_000_000), 2),
                (PacketLoss, None, 1000),
            ]
        );
        assert!(p.steps[2].bypass_min_duration());
        assert!(!p.steps[3].bypass_min_duration());
        assert_eq!(p.steps[19], Step::default_packet_loss());
        assert!(p.steps.iter().all(|s| s.streams(&Config::default()) == 1));
    }

    #[test]
    fn phases_come_in_blocks() {
        // Latency, then all downloads, then all uploads, then packet loss: a phase never
        // comes back once the next one has started.
        for p in [Profile::quick(), Profile::standard(), Profile::extended()] {
            let mut phases: Vec<Phase> = p.steps.iter().map(Step::phase).collect();
            phases.dedup();
            let expected: &[Phase] = if p.name == "quick" {
                &[Phase::Latency, Phase::Download, Phase::Upload]
            } else {
                &[
                    Phase::Latency,
                    Phase::Download,
                    Phase::Upload,
                    Phase::PacketLoss,
                ]
            };
            assert_eq!(phases, expected, "{}", p.name);
            // Sizes grow within each direction.
            for dir in [Direction::Download, Direction::Upload] {
                let sizes: Vec<u64> = p
                    .steps
                    .iter()
                    .filter(|s| s.direction() == Some(dir))
                    .filter_map(Step::bytes)
                    .collect();
                assert!(sizes.windows(2).all(|w| w[0] <= w[1]), "{} {dir:?}", p.name);
            }
        }
    }

    #[test]
    fn quick_and_extended() {
        let q = Profile::quick();
        assert_eq!(q.name, "quick");
        let summary: Vec<(Phase, Option<u64>, u32)> = q
            .steps
            .iter()
            .map(|s| (s.phase(), s.bytes(), s.count()))
            .collect();
        use Phase::*;
        assert_eq!(
            summary,
            vec![
                (Latency, None, 1),
                (Latency, None, 20),
                (Download, Some(100_000), 1),
                (Download, Some(100_000), 9),
                (Download, Some(1_000_000), 8),
                (Upload, Some(100_000), 8),
                (Upload, Some(1_000_000), 6),
            ]
        );

        let e = Profile::extended();
        let cfg = Config::default();
        for s in &e.steps {
            let expected = if s.bytes().is_some_and(|b| b >= 50_000_000) {
                4
            } else {
                1
            };
            assert_eq!(s.streams(&cfg), expected, "{s:?}");
        }
        assert_eq!(e.steps.iter().filter(|s| s.streams(&cfg) == 4).count(), 8);
    }

    #[test]
    fn concurrent_profile() {
        let c = Profile::concurrent();
        let d: Vec<_> = c
            .steps
            .iter()
            .map(|s| (s.phase(), s.duration_ms()))
            .collect();
        assert_eq!(
            d,
            vec![
                (Phase::Latency, Some(3000)),
                (Phase::Download, Some(10_000)),
                (Phase::Pause, Some(2000)),
                (Phase::Upload, Some(10_000)),
            ]
        );
        assert_eq!(c.steps[1].direction(), Some(Direction::Download));
    }

    #[test]
    fn short_concurrent_profile() {
        let c = Profile::concurrent_for(3000);
        let d: Vec<_> = c.steps.iter().map(Step::duration_ms).collect();
        assert_eq!(d, vec![Some(1000), Some(3000), Some(500), Some(3000)]);
        assert_eq!(Profile::concurrent_for(10_000), Profile::concurrent());
    }

    #[test]
    fn timed_profile_and_clamping() {
        let t = Profile::timed(30_000);
        assert_eq!(t.name, "timed");
        let shape: Vec<_> = t
            .steps
            .iter()
            .map(|s| (s.phase(), s.duration_ms()))
            .collect();
        assert_eq!(
            shape,
            vec![
                (Phase::Latency, None),
                (Phase::Latency, None),
                (Phase::Download, Some(30_000)),
                (Phase::Upload, Some(30_000)),
                (Phase::PacketLoss, None),
            ]
        );
        assert_eq!(t.steps[0], Step::Latency { num_packets: 1 });
        assert_eq!(t.steps[1], Step::Latency { num_packets: 20 });
        assert_eq!(t.timed_direction_ms(), Some(30_000));
        assert_eq!(Profile::timed(1).timed_direction_ms(), Some(10_000));
        assert_eq!(Profile::timed(u32::MAX).timed_direction_ms(), Some(300_000));
        assert_eq!(
            Profile::timed_unclamped(3000).timed_direction_ms(),
            Some(3000)
        );
        assert_eq!(
            Profile::timed_unclamped(10).timed_direction_ms(),
            Some(1000)
        );
        assert_eq!(
            Profile::timed_unclamped(900_000).timed_direction_ms(),
            Some(300_000)
        );
        assert_eq!(Profile::by_name("timed"), Some(Profile::timed(30_000)));
        assert_eq!(Profile::standard().timed_direction_ms(), None);
    }

    #[test]
    fn time_cap_never_cuts_timed_steps() {
        // A 5 min timed test: far beyond the 60 s cap.
        let p = Profile::timed(300_000);
        let cfg = Config::default();
        let mut r = RampState::new(&p, &cfg);
        let mut t = 0.0;
        let mut ran = Vec::new();
        while let Some((i, step)) = r.next_step(t) {
            let step = step.clone();
            // Mid-step checks never stop a timed step.
            if step.duration_ms().is_some() {
                assert!(!r.time_up_for(&step, t + 250_000.0));
            }
            t += f64::from(step.duration_ms().unwrap_or(100));
            ran.push(i);
            r.complete_step(i, PhaseStatus::Done);
        }
        assert_eq!(ran, vec![0, 1, 2, 3, 4]);
        assert!(t > 600_000.0);
        // A long group plan too.
        let mut r = RampState::new(&Profile::concurrent_for(300_000), &cfg);
        let mut n = 0;
        while let Some((i, _)) = r.next_step(f64::from(n) * 200_000.0) {
            r.complete_step(i, PhaseStatus::Done);
            n += 1;
        }
        assert_eq!(n, 4);
        // Sized steps still stop at the cap.
        assert!(r.time_up_for(&Step::download(1, 1), 60_000.0));
    }

    #[test]
    fn by_name_and_streams_clamp() {
        for n in Profile::BUILTIN {
            assert_eq!(Profile::by_name(n).unwrap().name, n);
        }
        assert!(Profile::by_name("nope").is_none());
        let cfg = Config {
            streams: 0,
            ..Config::default()
        };
        assert_eq!(Step::download(1, 1).streams(&cfg), 1);
        let s = Step::Upload {
            bytes: 1,
            count: 1,
            bypass_min_duration: false,
            streams: Some(99),
        };
        assert_eq!(s.streams(&cfg), MAX_STREAMS);
    }

    #[test]
    fn profile_json() {
        assert_eq!(
            serde_json::to_string(&Step::download(100_000, 9)).unwrap(),
            r#"{"type":"download","bytes":100000,"count":9}"#
        );
        assert_eq!(
            serde_json::to_string(&Step::Pause { duration_ms: 5 }).unwrap(),
            r#"{"type":"pause","duration_ms":5}"#
        );
        let custom: Profile = serde_json::from_str(
            r#"{"name":"custom","steps":[{"type":"latency","num_packets":5},{"type":"upload","bytes":1000,"count":2,"streams":2}]}"#,
        )
        .unwrap();
        assert_eq!(custom.steps.len(), 2);
        for p in [
            Profile::quick(),
            Profile::standard(),
            Profile::extended(),
            Profile::concurrent(),
        ] {
            let json = serde_json::to_string(&p).unwrap();
            assert_eq!(serde_json::from_str::<Profile>(&json).unwrap(), p);
        }
    }

    #[test]
    fn config_defaults_and_partial_json() {
        let c = Config::default();
        assert_eq!(c.bandwidth_finish_request_ms, 1000.0);
        assert_eq!(c.bandwidth_min_request_ms, 10.0);
        assert_eq!(c.loaded_latency_throttle_ms, 400);
        assert_eq!(c.loaded_request_min_ms, 250.0);
        assert_eq!(c.loaded_latency_max_points, 20);
        assert_eq!(c.latency_percentile, 0.5);
        assert_eq!(c.bandwidth_percentile, 0.9);
        assert_eq!(c.max_test_ms, 60_000);
        assert_eq!(c.streams, 1);
        let p: Config = serde_json::from_str(r#"{"streams":4}"#).unwrap();
        assert_eq!(
            p,
            Config {
                streams: 4,
                ..Config::default()
            }
        );
    }

    /// Runs the whole profile; `duration(step, bytes)` gives each request's time.
    fn simulate(
        profile: &Profile,
        config: &Config,
        duration: impl Fn(Direction, u64) -> f64,
    ) -> (RampState, Vec<usize>, f64) {
        let mut ramp = RampState::new(profile, config);
        let mut ran = Vec::new();
        let mut elapsed = 0.0;
        while let Some((i, step)) = ramp.next_step(elapsed) {
            let step = step.clone();
            ran.push(i);
            match (step.direction(), step.bytes()) {
                (Some(dir), Some(bytes)) => {
                    for _ in 0..step.count() {
                        let d = duration(dir, bytes);
                        elapsed += d;
                        ramp.record_request(dir, bytes, d);
                    }
                }
                _ => elapsed += 10.0,
            }
            ramp.complete_step(i, PhaseStatus::Done);
        }
        (ramp, ran, elapsed)
    }

    #[test]
    fn fast_link_runs_everything() {
        // 10 Gbps symmetric: 1 GB takes 800 ms, under the 1 s threshold, so 2 GB runs too.
        let p = Profile::standard();
        let (ramp, ran, _) = simulate(&p, &Config::default(), |_, b| b as f64 * 8.0 / 1e10 * 1e3);
        assert_eq!(ran, (0..20).collect::<Vec<_>>());
        assert!(ramp.is_finished());
        assert!((0..20).all(|i| ramp.status(i) == Some(PhaseStatus::Done)));
        // 2 GB takes 1.6 s, so it registers as the halt size, but nothing is larger.
        assert_eq!(ramp.halted_at(Direction::Download), Some(2_000_000_000));
    }

    #[test]
    fn large_sizes_only_on_fast_links() {
        let p = Profile::standard();
        let ran_sizes = |bps: f64| {
            let (_, ran, _) = simulate(&p, &Config::default(), |_, b| b as f64 * 8.0 / bps * 1e3);
            let mut sizes: Vec<u64> = ran.iter().filter_map(|&i| p.steps[i].bytes()).collect();
            sizes.sort_unstable();
            sizes.dedup();
            sizes.last().copied()
        };
        // 1 Gbps: 250 MB takes 2 s, so 1 GB never runs.
        assert_eq!(ran_sizes(1e9), Some(250_000_000));
        // 2.5 Gbps: 250 MB takes 0.8 s, so 1 GB runs (3.2 s) but 2 GB doesn't.
        assert_eq!(ran_sizes(2.5e9), Some(1_000_000_000));
        // 10 Gbps: 1 GB takes 0.8 s, so 2 GB runs.
        assert_eq!(ran_sizes(1e10), Some(2_000_000_000));
    }

    #[test]
    fn slow_download_halts_only_download() {
        // Download at 50 Mbps (10 MB = 1.6 s), upload at 10 Gbps.
        let p = Profile::standard();
        let (ramp, ran, _) = simulate(&p, &Config::default(), |dir, b| match dir {
            Direction::Download => b as f64 * 8.0 / 5e7 * 1e3,
            Direction::Upload => b as f64 * 8.0 / 1e10 * 1e3,
        });
        assert_eq!(ramp.halted_at(Direction::Download), Some(10_000_000));
        // Upload reaches 2 GB (1.6 s), the largest size.
        assert_eq!(ramp.halted_at(Direction::Upload), Some(2_000_000_000));
        // The 10 MB download (index 5) ran to completion; 25 MB+ downloads skipped.
        assert!(ran.contains(&5));
        for i in 6..=10 {
            assert_eq!(ramp.status(i), Some(PhaseStatus::Skipped), "step {}", i + 1);
            assert!(!ran.contains(&i));
        }
        // Every upload and the packet loss test still ran.
        for i in 11..=19 {
            assert_eq!(ramp.status(i), Some(PhaseStatus::Done), "step {}", i + 1);
        }
    }

    #[test]
    fn halting_threshold_is_inclusive() {
        let mut r = RampState::new(&Profile::standard(), &Config::default());
        r.record_request(Direction::Upload, 1_000_000, 999.9);
        assert_eq!(r.halted_at(Direction::Upload), None);
        r.record_request(Direction::Upload, 1_000_000, 1000.0);
        assert_eq!(r.halted_at(Direction::Upload), Some(1_000_000));
        // A later, larger request doesn't move the halt size up.
        r.record_request(Direction::Upload, 10_000_000, 5000.0);
        assert_eq!(r.halted_at(Direction::Upload), Some(1_000_000));
        r.record_request(Direction::Upload, 1, f64::NAN);
        assert_eq!(r.halted_at(Direction::Upload), Some(1_000_000));
    }

    #[test]
    fn time_cap_skips_remaining() {
        let cfg = Config {
            max_test_ms: 1000,
            bandwidth_finish_request_ms: f64::INFINITY,
            ..Config::default()
        };
        // Every request takes 100 ms.
        let (ramp, ran, elapsed) = simulate(&Profile::standard(), &cfg, |_, _| 100.0);
        assert!(elapsed >= 1000.0);
        assert!(ramp.is_finished());
        let last = *ran.last().unwrap();
        for i in 0..20 {
            let expected = if ran.contains(&i) {
                PhaseStatus::Done
            } else {
                PhaseStatus::Skipped
            };
            assert_eq!(ramp.status(i), Some(expected));
        }
        // Transfers stopped at the cap; packet loss (exempt) still ran last.
        assert_eq!(last, 19);
        assert!((3..19).any(|i| ramp.status(i) == Some(PhaseStatus::Skipped)));
        assert_eq!(ramp.clone().next_step(0.0), None);
    }

    #[test]
    fn time_cap_at_start_and_manual_statuses() {
        let mut r = RampState::new(&Profile::quick(), &Config::default());
        let (i, _) = r.next_step(0.0).unwrap();
        assert_eq!(i, 0);
        assert!(!r.time_up(59_999.0));
        r.complete_step(i, PhaseStatus::Failed);
        assert_eq!(r.next_step(60_000.0), None);
        assert_eq!(r.status(0), Some(PhaseStatus::Failed));
        assert!((1..7).all(|i| r.status(i) == Some(PhaseStatus::Skipped)));
        assert!(r.is_finished());
    }

    #[test]
    fn direction_budget_keeps_time_for_upload() {
        let cfg = Config {
            max_direction_ms: 1000,
            ..Config::default()
        };
        let p = Profile::standard();
        let mut r = RampState::new(&p, &cfg);
        // Latency x2.
        for _ in 0..2 {
            let (i, _) = r.next_step(0.0).unwrap();
            r.complete_step(i, PhaseStatus::Done);
        }
        // First download starts the download budget at t = 100 ms.
        let (i, step) = r.next_step(100.0).unwrap();
        let step = step.clone();
        assert_eq!(step.direction(), Some(Direction::Download));
        assert!(!r.time_up_for(&step, 1000.0));
        assert!(r.time_up_for(&step, 1100.0));
        r.complete_step(i, PhaseStatus::Done);
        // Budget used: the remaining downloads are skipped, uploads still run.
        let (i, step) = r.next_step(1200.0).unwrap();
        assert_eq!(step.direction(), Some(Direction::Upload));
        assert_eq!(step.bytes(), Some(100_000));
        for j in 3..=10 {
            assert_eq!(r.status(j), Some(PhaseStatus::Skipped), "step {j}");
        }
        r.complete_step(i, PhaseStatus::Done);
    }

    #[test]
    fn packet_loss_runs_after_the_time_cap() {
        let cfg = Config {
            max_test_ms: 1000,
            ..Config::default()
        };
        let p = Profile::standard();
        let mut r = RampState::new(&p, &cfg);
        let (i, _) = r.next_step(0.0).unwrap();
        r.complete_step(i, PhaseStatus::Done);
        // Past the cap: every transfer is skipped, packet loss still runs.
        let (i, step) = r.next_step(5000.0).unwrap();
        assert_eq!(step.phase(), Phase::PacketLoss);
        assert_eq!(i, 19);
        r.complete_step(i, PhaseStatus::Done);
        assert!(r.next_step(5000.0).is_none());
        assert!(r.is_finished());
        assert_eq!(r.status(1), Some(PhaseStatus::Skipped));
    }
}
