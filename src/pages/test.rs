//! The test page (`/`, SPEC §7.4): hero with the packet-flow canvas and phase
//! timeline, label field and Start button, live throughput chart, metric
//! tiles, network quality and server cards, and the detail charts once the
//! run finishes. Finished runs are saved automatically (§7.7).

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_query_map;
use uuid::Uuid;

use crate::components::MetricKind;
use crate::components::charts::{LiveArea, LivePoint, PayloadTick, Series};
use crate::components::devices_panel::DevicesPanel;
use crate::components::group_view::{
    Countdown, GroupLanes, GroupResultView, plan_progress, use_group_total,
};
use crate::components::hero::Hero;
use crate::components::icons;
use crate::components::label_picker::{LabelPicker, clean_label};
use crate::components::metric_tile::MetricTile;
use crate::components::motion::{FlowMode, OtherFlow};
use crate::components::phase_timeline::{PHASES, timeline};
use crate::components::presence::{GroupStage, PresenceCtx, use_presence};
use crate::components::quality_card::QualityCard;
use crate::components::result_view::{
    INFO_DOWNLOAD, INFO_JITTER, INFO_LATENCY, INFO_LOSS, INFO_UPLOAD, ResultDetails, SharedBadge,
    shared_text,
};
use crate::components::server_card::ServerCard;
use crate::model::device::default_label;
use crate::model::profile::{PhaseStatus, Profile, Step};
use crate::model::protocol::{PeerState, Phase};
use crate::model::result::{Concurrency, PacketLossCounts, RunResult};
use crate::model::scores::Scores;
use crate::model::units::{format_ms, format_rate};
use crate::pages::api::{self, ServerInfo};
use crate::ui_state::{keys, storage_get, storage_set, use_ui_state};

// ---------------------------------------------------------------------------------------------
// Packet loss probe hook
// ---------------------------------------------------------------------------------------------

/// The WebRTC packet loss probe for new runs; `None` marks packet loss steps
/// skipped ("Unavailable").
#[cfg(feature = "hydrate")]
fn packet_loss_probe() -> Option<std::rc::Rc<dyn crate::engine::PacketLossProbe>> {
    crate::engine::default_packet_loss_probe()
}

// ---------------------------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------------------------

/// One-line description of a profile step, e.g. `Download · 10 MB × 6`.
pub fn step_label(step: &Step) -> String {
    use crate::components::charts::scale::format_bytes;
    let plural = |n: u32, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let secs = |ms: u32| format!("{} s", f64::from(ms) / 1000.0);
    match *step {
        Step::Latency { num_packets } => {
            format!("Latency · {}", plural(num_packets, "probe", "probes"))
        }
        Step::Download { bytes, count, .. } => {
            format!("Download · {} × {count}", format_bytes(bytes))
        }
        Step::Upload { bytes, count, .. } => format!("Upload · {} × {count}", format_bytes(bytes)),
        Step::PacketLoss { num_packets, .. } => {
            format!("Packet loss · {}", plural(num_packets, "packet", "packets"))
        }
        Step::TimedLatency { duration_ms } => format!("Latency · {}", secs(duration_ms)),
        Step::TimedDownload { duration_ms } => format!("Download · {}", secs(duration_ms)),
        Step::TimedUpload { duration_ms } => format!("Upload · {}", secs(duration_ms)),
        Step::Pause { duration_ms } => format!("Pause · {}", secs(duration_ms)),
    }
}

/// Units of progress a step reports (probes or requests).
fn step_units(step: &Step) -> u32 {
    match *step {
        Step::Latency { num_packets } => num_packets,
        Step::Download { count, .. } | Step::Upload { count, .. } => count,
        _ => 0,
    }
}

/// Why packet loss is unavailable for a run.
pub fn loss_reason(profile: &str, steps: &[Step]) -> String {
    if !steps.iter().any(|s| s.phase() == Phase::PacketLoss) {
        let name = crate::ui_state::PROFILES
            .iter()
            .find(|(k, _)| *k == profile)
            .map_or(profile, |(_, v)| v);
        format!("The {name} profile doesn't measure packet loss.")
    } else {
        "Packet loss needs a WebRTC connection, which wasn't available for this run.".into()
    }
}

fn phase_name(phase: Phase) -> &'static str {
    PHASES
        .iter()
        .find(|(p, _)| *p == phase)
        .map_or("Test", |(_, n)| n)
}

// ---------------------------------------------------------------------------------------------
// Controller
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Idle,
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SaveState {
    Idle,
    Saving,
    Saved(Uuid),
    Failed(String),
}

/// Signals the page reads, fed by engine events (never the other way round).
#[derive(Clone, Copy)]
struct TestController {
    status: RwSignal<RunStatus>,
    /// Error or stop message.
    message: RwSignal<String>,
    steps: RwSignal<Vec<Step>>,
    statuses: RwSignal<Vec<Option<PhaseStatus>>>,
    current: RwSignal<Option<usize>>,
    /// Probes/requests finished in the current step.
    step_done: RwSignal<u32>,
    phase: RwSignal<Phase>,
    live_bps: RwSignal<Option<f64>>,
    download: RwSignal<Option<f64>>,
    upload: RwSignal<Option<f64>>,
    latency: RwSignal<Option<f64>>,
    jitter: RwSignal<Option<f64>>,
    packet_loss: RwSignal<Option<f64>>,
    loss_counts: RwSignal<Option<PacketLossCounts>>,
    down_samples: RwSignal<Vec<f64>>,
    up_samples: RwSignal<Vec<f64>>,
    latency_samples: RwSignal<Vec<f64>>,
    jitter_samples: RwSignal<Vec<f64>>,
    down_loaded: RwSignal<Vec<f64>>,
    up_loaded: RwSignal<Vec<f64>>,
    live_points: RwSignal<Vec<LivePoint>>,
    payload_ticks: RwSignal<Vec<PayloadTick>>,
    pulses: RwSignal<u32>,
    scores: RwSignal<Option<Scores>>,
    result: RwSignal<Option<RunResult>>,
    duration_ms: RwSignal<Option<u64>>,
    save: RwSignal<SaveState>,
    /// The label the last run was saved with.
    saved_label: RwSignal<String>,
    /// Polite screen reader announcements (phase completions).
    announce: RwSignal<String>,
    info: RwSignal<Option<ServerInfo>>,
    label: RwSignal<String>,
    profile_name: RwSignal<String>,
    /// "Shared network" explanation once the saved run says it overlapped.
    shared: RwSignal<Option<String>>,
    /// Most other devices seen testing during this run (client view).
    peers_seen: RwSignal<u32>,
    /// The "Wait / Test anyway" prompt is open.
    prompt: RwSignal<bool>,
    /// Waiting for the other devices to finish before starting.
    waiting: RwSignal<bool>,
    ui: crate::ui_state::UiState,
    presence: PresenceCtx,
    #[cfg(feature = "hydrate")]
    handle: StoredValue<Option<crate::engine::EngineHandle>, LocalStorage>,
}

impl TestController {
    fn new(ui: crate::ui_state::UiState, presence: PresenceCtx) -> Self {
        Self {
            ui,
            presence,
            shared: RwSignal::new(None),
            peers_seen: RwSignal::new(0),
            prompt: RwSignal::new(false),
            waiting: RwSignal::new(false),
            status: RwSignal::new(RunStatus::Idle),
            message: RwSignal::new(String::new()),
            steps: RwSignal::new(Profile::standard().steps),
            statuses: RwSignal::new(Vec::new()),
            current: RwSignal::new(None),
            step_done: RwSignal::new(0),
            phase: RwSignal::new(Phase::Idle),
            live_bps: RwSignal::new(None),
            download: RwSignal::new(None),
            upload: RwSignal::new(None),
            latency: RwSignal::new(None),
            jitter: RwSignal::new(None),
            packet_loss: RwSignal::new(None),
            loss_counts: RwSignal::new(None),
            down_samples: RwSignal::new(Vec::new()),
            up_samples: RwSignal::new(Vec::new()),
            latency_samples: RwSignal::new(Vec::new()),
            jitter_samples: RwSignal::new(Vec::new()),
            down_loaded: RwSignal::new(Vec::new()),
            up_loaded: RwSignal::new(Vec::new()),
            live_points: RwSignal::new(Vec::new()),
            payload_ticks: RwSignal::new(Vec::new()),
            pulses: RwSignal::new(0),
            scores: RwSignal::new(None),
            result: RwSignal::new(None),
            duration_ms: RwSignal::new(None),
            save: RwSignal::new(SaveState::Idle),
            saved_label: RwSignal::new(String::new()),
            announce: RwSignal::new(String::new()),
            info: RwSignal::new(None),
            label: presence.label,
            profile_name: RwSignal::new("standard".into()),
            #[cfg(feature = "hydrate")]
            handle: StoredValue::new_local(None),
        }
    }

    fn history_on(&self) -> bool {
        self.info.get().is_none_or(|i| i.history)
    }

    #[cfg_attr(not(feature = "hydrate"), allow(dead_code))]
    fn reset(&self, steps: Vec<Step>) {
        let n = steps.len();
        self.steps.set(steps);
        self.statuses.set(vec![None; n]);
        self.current.set(None);
        self.step_done.set(0);
        self.phase.set(Phase::Idle);
        for s in [
            self.live_bps,
            self.download,
            self.upload,
            self.latency,
            self.jitter,
            self.packet_loss,
        ] {
            s.set(None);
        }
        for v in [
            self.down_samples,
            self.up_samples,
            self.latency_samples,
            self.jitter_samples,
            self.down_loaded,
            self.up_loaded,
        ] {
            v.set(Vec::new());
        }
        self.loss_counts.set(None);
        self.live_points.set(Vec::new());
        self.payload_ticks.set(Vec::new());
        self.scores.set(None);
        self.result.set(None);
        self.duration_ms.set(None);
        self.save.set(SaveState::Idle);
        self.message.set(String::new());
        self.shared.set(None);
        self.peers_seen.set(0);
    }

    /// Other devices are testing (visible or hidden).
    fn others_busy(&self) -> bool {
        !self.presence.others_testing().is_empty() || self.presence.hidden_active.get() > 0
    }

    /// Start pressed: asks first when other devices are testing (SPEC §5.11).
    fn request_start(self) {
        if self.status.get_untracked() != RunStatus::Running && untrack(|| self.others_busy()) {
            self.prompt.set(true);
            return;
        }
        self.toggle();
    }

    /// Starts a test, or stops the running one.
    fn toggle(self) {
        #[cfg(feature = "hydrate")]
        {
            if self.status.get_untracked() == RunStatus::Running {
                self.handle.with_value(|h| {
                    if let Some(h) = h {
                        h.abort();
                    }
                });
            } else {
                self.prompt.set(false);
                self.waiting.set(false);
                self.start();
            }
        }
    }

    #[cfg(feature = "hydrate")]
    fn start(self) {
        use std::cell::RefCell;
        use std::rc::Rc;

        use crate::engine::{self, EngineEvent, EngineOptions};
        use crate::model::profile::Direction;
        use crate::model::result::Transport;
        use crate::model::stats;

        let ui = self.ui;
        let transport = match ui.transport.get_untracked().as_str() {
            "http" => Transport::Http,
            _ => Transport::Websocket,
        };
        let profile_name = ui.profile.get_untracked();
        let profile = Profile::by_name(&profile_name).unwrap_or_default();
        // Reuse the page's presence session, so the server attributes this
        // run's traffic to this device (live speed, overlap flags).
        let mut opts = match self.presence.handle() {
            Some(h) => h.engine_options(transport, profile),
            None => {
                let mut device = engine::detect_device();
                let name = ui.device_name.get_untracked();
                if !name.trim().is_empty() {
                    device.device_name = name.trim().to_string();
                }
                EngineOptions::new(transport, profile, device)
            }
        };
        let max = self
            .info
            .get_untracked()
            .map_or(8, |i| i.limits.max_streams.max(1));
        opts.config.streams = ui.streams.get_untracked().clamp(1, max);
        opts.packet_loss = packet_loss_probe();
        let session = opts.session;
        let percentile = opts.config.bandwidth_percentile;

        // Remember the label for the next run on this device.
        let label = clean_label(&self.label.get_untracked());
        if let Some(l) = &label {
            storage_set(keys::LAST_LABEL, l);
        }
        opts.label = label.clone();

        self.profile_name.set(profile_name);
        self.reset(opts.profile.steps.clone());
        self.status.set(RunStatus::Running);
        self.announce.set("Test started".into());

        #[derive(Default)]
        struct Acc {
            down: Vec<f64>,
            up: Vec<f64>,
        }
        let acc = Rc::new(RefCell::new(Acc::default()));
        let t0 = web_sys::window()
            .and_then(|w| w.performance())
            .map_or(0.0, |p| p.now());
        let elapsed = move || {
            web_sys::window()
                .and_then(|w| w.performance())
                .map_or(0.0, |p| p.now())
                - t0
        };
        let this = self;
        let on_event = move |ev: EngineEvent| match ev {
            EngineEvent::Connected { .. } => {}
            EngineEvent::PhaseStarted { index, step } => {
                this.current.set(Some(index));
                this.step_done.set(0);
                this.phase.set(step.phase());
                if let (Some(direction), Some(bytes)) = (step.direction(), step.bytes()) {
                    this.payload_ticks.update(|t| {
                        t.push(PayloadTick {
                            t_ms: elapsed(),
                            bytes,
                            direction,
                        })
                    });
                } else {
                    this.live_bps.set(None);
                }
            }
            EngineEvent::Latency { ms } => {
                this.step_done.update(|n| *n += 1);
                this.pulses.update(|n| *n = n.wrapping_add(1));
                this.latency_samples.update(|v| v.push(ms));
                let (median, jitter) = this
                    .latency_samples
                    .with_untracked(|v| (stats::median(v), stats::jitter(v)));
                this.latency.set(median);
                this.jitter.set(jitter);
                if let Some(j) = jitter {
                    this.jitter_samples.update(|v| v.push(j));
                }
            }
            EngineEvent::Bandwidth { direction, point } => {
                this.step_done.update(|n| *n += 1);
                let mut a = acc.borrow_mut();
                let (v, sig, samples) = match direction {
                    Direction::Download => (&mut a.down, this.download, this.down_samples),
                    Direction::Upload => (&mut a.up, this.upload, this.up_samples),
                };
                v.push(point.bps);
                sig.set(stats::percentile(v, percentile));
                samples.update(|s| s.push(point.bps));
            }
            EngineEvent::Live { direction, bps } => {
                this.live_bps.set(Some(bps));
                this.live_points.update(|p| {
                    p.push(LivePoint {
                        t_ms: elapsed(),
                        bps,
                        direction,
                    })
                });
            }
            EngineEvent::LoadedLatency { direction, ms } => match direction {
                Direction::Download => this.down_loaded.update(|v| v.push(ms)),
                Direction::Upload => this.up_loaded.update(|v| v.push(ms)),
            },
            EngineEvent::PacketLoss(counts) => {
                this.loss_counts.set(Some(counts));
                this.packet_loss.set(counts.ratio());
            }
            EngineEvent::PhaseFinished { index, status } => {
                this.statuses.update(|s| {
                    if let Some(slot) = s.get_mut(index) {
                        *slot = Some(status);
                    }
                });
                // Announce when every step of this phase is finished.
                let phase = this.steps.with_untracked(|s| s.get(index).map(Step::phase));
                if let Some(phase) = phase {
                    let complete = this.steps.with_untracked(|steps| {
                        this.statuses.with_untracked(|st| {
                            steps
                                .iter()
                                .zip(st)
                                .filter(|(s, _)| s.phase() == phase)
                                .all(|(_, st)| st.is_some())
                        })
                    });
                    if complete {
                        this.announce.set(this.phase_summary(phase));
                    }
                }
            }
            EngineEvent::Finished(result) => this.finish(*result, session),
            EngineEvent::Error(msg) => {
                this.live_bps.set(None);
                this.current.set(None);
                this.phase.set(Phase::Idle);
                this.message.set(msg.clone());
                this.status.set(RunStatus::Failed);
                this.announce.set(format!("Test failed: {msg}"));
            }
            EngineEvent::Aborted => {
                this.live_bps.set(None);
                this.current.set(None);
                this.phase.set(Phase::Idle);
                this.message.set("Stopped".into());
                this.status.set(RunStatus::Failed);
                this.announce.set("Test stopped".into());
            }
        };
        let handle = engine::start(opts, on_event);
        self.handle.set_value(Some(handle));
    }

    #[cfg_attr(not(feature = "hydrate"), allow(dead_code))]
    fn phase_summary(&self, phase: Phase) -> String {
        let unit = self.ui.units.get_untracked();
        let rate = |v: Option<f64>| {
            let (s, u) = format_rate(v.unwrap_or(f64::NAN), unit);
            format!("{s} {u}")
        };
        match phase {
            Phase::Latency => {
                let (s, u) = format_ms(self.latency.get_untracked().unwrap_or(f64::NAN));
                format!("Latency done: {s} {u}")
            }
            Phase::Download => format!("Download done: {}", rate(self.download.get_untracked())),
            Phase::Upload => format!("Upload done: {}", rate(self.upload.get_untracked())),
            Phase::PacketLoss => "Packet loss done".into(),
            _ => String::new(),
        }
    }

    /// Shows the final result and saves it (SPEC §7.7).
    #[cfg_attr(not(feature = "hydrate"), allow(dead_code))]
    fn finish(self, mut result: RunResult, session: Uuid) {
        let s = result.summary;
        self.download.set(s.download_bps);
        self.upload.set(s.upload_bps);
        self.latency.set(s.latency_ms);
        self.jitter.set(s.jitter_ms);
        self.packet_loss.set(s.packet_loss_ratio);
        self.loss_counts.set(result.points.packet_loss);
        self.scores.set(Some(result.scores));
        self.live_bps.set(None);
        self.current.set(None);
        self.phase.set(Phase::Idle);
        self.duration_ms.set(Some(result.duration_ms));
        // Steps the ramp passed over get no events: take the final statuses.
        if result.phases.len() == self.steps.with_untracked(Vec::len) {
            self.statuses
                .set(result.phases.iter().map(|p| Some(p.status)).collect());
        }
        result.label = clean_label(&self.label.get_untracked());
        self.saved_label
            .set(result.label.clone().unwrap_or_default());
        self.result.set(Some(result.clone()));
        self.status.set(RunStatus::Done);
        let unit = self.ui.units.get_untracked();
        let (d, du) = format_rate(s.download_bps.unwrap_or(f64::NAN), unit);
        let (u, uu) = format_rate(s.upload_bps.unwrap_or(f64::NAN), unit);
        self.announce.set(format!(
            "Test finished. Download {d} {du}, upload {u} {uu}."
        ));

        // "Shared network": the server's overlap record of the saved run
        // (`GET /api/runs/{id}` after the POST, which only returns the id).
        // Without history, fall back to the other devices this page saw
        // testing during the run.
        let seen = self.peers_seen.get_untracked();
        let client_view = Concurrency {
            peers_max: seen,
            overlap_ms: 0.0,
            overlap_ratio: if seen > 0 { 1.0 } else { 0.0 },
        };
        let history = self.info.get_untracked().is_none_or(|i| i.history);
        if history {
            self.save.set(SaveState::Saving);
        }
        let (save, shared) = (self.save, self.shared);
        spawn_local(async move {
            // Always posted with the session: the server fills in the
            // overlap (and accepts, without storing, when history is off).
            let posted = api::save_run(&result, session).await;
            let state = match &posted {
                Ok(saved) if saved.stored => SaveState::Saved(saved.id),
                Ok(_) => SaveState::Idle,
                Err(e) => SaveState::Failed(e.clone()),
            };
            if history {
                save.set(state);
            }
            let concurrency = match posted {
                Ok(saved) if saved.stored => api::get_run(saved.id)
                    .await
                    .map(|r| r.concurrency)
                    .unwrap_or(client_view),
                _ => client_view,
            };
            shared.set(shared_text(&concurrency));
        });
    }
}

// ---------------------------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------------------------

#[component]
pub fn TestPage() -> impl IntoView {
    let ui = use_ui_state();
    let presence = use_presence();
    let test = TestController::new(ui, presence);
    let status = test.status;
    let running = Signal::derive(move || status.get() == RunStatus::Running);
    let done = Signal::derive(move || status.get() == RunStatus::Done);
    let stage = presence.stage;
    let group_mode = Memo::new(move |_| stage.with(GroupStage::active));
    let group_done = Memo::new(move |_| stage.with(|s| *s == GroupStage::Done));

    // Server info (name, history on/off, limits).
    Effect::new(move |_| {
        spawn_local(async move {
            if let Ok(info) = api::server_info().await {
                test.info.set(Some(info));
            }
        });
    });

    // Share run state and limits with the settings drawer.
    Effect::new(move |prev: Option<bool>| {
        let r = running.get();
        // A group run manages the flag itself.
        if prev.is_some() || r {
            ui.running.set(r || group_mode.get_untracked());
        }
        r
    });
    Effect::new(move |_| {
        if let Some(i) = test.info.get() {
            ui.max_streams.set(i.limits.max_streams.max(1));
        }
    });

    // Prefill the label once the device name is known (client only).
    Effect::new(move |_| {
        if !ui.loaded.get() || !test.label.get_untracked().is_empty() {
            return;
        }
        let name = ui.device_name.get();
        let label = storage_get(keys::LAST_LABEL)
            .and_then(|l| clean_label(&l))
            .unwrap_or_else(|| {
                let (day, month) = today();
                default_label(&name, day, month)
            });
        test.label.set(label);
    });
    // Other devices see this device's label.
    Effect::new(move |prev: Option<String>| {
        let l = test.label.get();
        let connected = presence.connected.get();
        if connected && prev.as_deref() != Some(l.as_str()) && !running.get_untracked() {
            presence.announce_label(clean_label(&l));
        }
        if connected { l } else { String::new() }
    });

    // Remember how many other devices tested during this run.
    Effect::new(move |_| {
        if running.get() {
            let n = presence.others_testing().len() as u32 + presence.hidden_active.get();
            if n > test.peers_seen.get_untracked() {
                test.peers_seen.set(n);
            }
        }
    });
    // "Wait for them": start once nobody else is testing.
    Effect::new(move |_| {
        if test.waiting.get() && !test.others_busy() {
            test.waiting.set(false);
            test.toggle();
        }
    });

    // Steps shown by the idle timeline follow the selected profile.
    Effect::new(move |_| {
        let p = ui.profile.get();
        if test.status.get_untracked() == RunStatus::Idle {
            test.steps
                .set(Profile::by_name(&p).unwrap_or_default().steps);
        }
    });

    // `?autostart=1` starts a run once settings are loaded (SPEC §7.8).
    let query = use_query_map();
    Effect::new(move |started: Option<bool>| {
        if started == Some(true) {
            return true;
        }
        let wanted = query.with(|q| q.get("autostart").as_deref() == Some("1"));
        if wanted && ui.loaded.get() && !test.label.get().is_empty() {
            test.toggle();
            return true;
        }
        false
    });

    let timeline_steps = Signal::derive(move || {
        if group_mode.get() {
            let steps = presence.group_plan.get();
            let elapsed = presence.clock.get() - presence.group_starts_at.get();
            let (mut st, cur, p) = plan_progress(&steps, elapsed);
            if group_done.get() {
                st = vec![Some(PhaseStatus::Done); steps.len()];
            }
            return timeline(&steps, &st, cur, p);
        }
        let current = test.current.get();
        let progress = test.steps.with(|steps| {
            current
                .and_then(|i| steps.get(i))
                .map_or(0.0, |s| match step_units(s) {
                    0 => 0.5,
                    n => f64::from(test.step_done.get()) / f64::from(n),
                })
        });
        test.steps.with(|steps| {
            test.statuses
                .with(|st| timeline(steps, st, current, progress))
        })
    });

    // Hero content by state (solo run or group test).
    let group_total = use_group_total();
    let group_phase = presence.group_phase;
    let hero_kind = Signal::derive(move || match (status.get(), test.phase.get()) {
        _ if group_mode.get() => MetricKind::Rate,
        (RunStatus::Running, Phase::Latency) => MetricKind::Ms,
        (RunStatus::Running, Phase::PacketLoss) => MetricKind::Percent,
        _ => MetricKind::Rate,
    });
    let hero_value = Signal::derive(move || {
        if group_mode.get() {
            return match stage.get() {
                GroupStage::Done => presence
                    .group_result
                    .with(|r| r.as_ref().and_then(|r| r.aggregate_download_bps)),
                _ => group_total.get(),
            };
        }
        match (status.get(), test.phase.get()) {
            (RunStatus::Running, Phase::Latency) => test.latency.get(),
            (RunStatus::Running, Phase::Download) => test.live_bps.get().or(test.download.get()),
            (RunStatus::Running, Phase::Upload) => test.live_bps.get().or(test.upload.get()),
            (RunStatus::Running, Phase::PacketLoss) => test.packet_loss.get(),
            (RunStatus::Running, _) => None,
            _ => test.download.get(),
        }
    });
    let hero_title = Signal::derive(move || {
        if group_mode.get() {
            return match stage.get() {
                GroupStage::Countdown { .. } => "Testing together".to_string(),
                GroupStage::Done => "Total download".to_string(),
                GroupStage::Waiting => "Collecting results".to_string(),
                GroupStage::Failed(_) => "This device dropped out".to_string(),
                _ => match group_phase.get() {
                    Phase::Download => "Total download".to_string(),
                    Phase::Upload => "Total upload".to_string(),
                    Phase::Latency => "Latency, together".to_string(),
                    Phase::Pause => "Switching to upload".to_string(),
                    _ => "Testing together".to_string(),
                },
            };
        }
        match (status.get(), test.phase.get()) {
            (RunStatus::Running, Phase::Idle | Phase::Pause) => "Connecting",
            (RunStatus::Running, p) => phase_name(p),
            (RunStatus::Done, _) => "Download",
            (RunStatus::Failed, _) => "Test stopped",
            (RunStatus::Idle, _) => "Your network speed",
        }
        .to_string()
    });
    let hero_caption = Signal::derive(move || {
        if group_mode.get() {
            let n = presence.group_members.with(Vec::len);
            return match stage.get() {
                GroupStage::Countdown { .. } => format!("{n} devices start at the same moment."),
                GroupStage::Waiting => "Waiting for every device's result…".to_string(),
                GroupStage::Failed(e) => e,
                GroupStage::Done => {
                    let unit = ui.units.get();
                    let up = presence
                        .group_result
                        .with(|r| r.as_ref().and_then(|r| r.aggregate_upload_bps));
                    let (u, uu) = format_rate(up.unwrap_or(f64::NAN), unit);
                    format!("{n} devices · Total upload {u} {uu}")
                }
                _ => format!("{n} devices, measured by the server"),
            };
        }
        match status.get() {
            RunStatus::Idle => "Measures download, upload and latency to this server.".to_string(),
            RunStatus::Running => test
                .current
                .get()
                .and_then(|i| test.steps.with(|s| s.get(i).map(step_label)))
                .unwrap_or_else(|| "Opening connections…".into()),
            RunStatus::Done => {
                let unit = ui.units.get();
                let (u, uu) = format_rate(test.upload.get().unwrap_or(f64::NAN), unit);
                let (l, lu) = format_ms(test.latency.get().unwrap_or(f64::NAN));
                let secs = test.duration_ms.get().unwrap_or(0) as f64 / 1000.0;
                format!("Upload {u} {uu} · Latency {l} {lu} · {secs:.1} s")
            }
            RunStatus::Failed => test.message.get(),
        }
    });
    let flow_mode = Signal::derive(move || {
        let phase = if group_mode.get() {
            group_phase.get()
        } else if status.get() == RunStatus::Running {
            test.phase.get()
        } else {
            Phase::Idle
        };
        match phase {
            Phase::Latency => FlowMode::Latency,
            Phase::Download => FlowMode::Download,
            Phase::Upload => FlowMode::Upload,
            _ => FlowMode::Idle,
        }
    });
    let flow_bps = Signal::derive(move || {
        if group_mode.get() {
            group_total.get()
        } else {
            test.live_bps.get()
        }
    });
    // Other devices' transfers, faint in their colors.
    let others_flow = Signal::derive(move || {
        presence.peers.with(|peers| {
            peers
                .iter()
                .filter(|p| !p.is_you)
                .filter_map(|p| match p.state {
                    PeerState::Testing {
                        phase,
                        down_bps,
                        up_bps,
                        ..
                    } => {
                        let (mode, bps) = match phase {
                            Phase::Download => (FlowMode::Download, down_bps),
                            Phase::Upload => (FlowMode::Upload, up_bps),
                            _ => return None,
                        };
                        let slot = match presence.color(p.id) {
                            Series::Device(n) => n,
                            _ => 1,
                        };
                        Some(OtherFlow { slot, mode, bps })
                    }
                    PeerState::Idle => None,
                })
                .collect()
        })
    });
    let vt_name = Signal::derive(move || match test.save.get() {
        SaveState::Saved(id) if !group_mode.get() => format!("run-{id}"),
        _ => "hero-number".to_string(),
    });

    let button_label = move || match status.get() {
        RunStatus::Idle => "Start",
        RunStatus::Running => "Stop",
        RunStatus::Done | RunStatus::Failed => "Test again",
    };
    let in_group = Memo::new(move |_| presence.group.with(Option::is_some));

    let phase_is = move |p: Phase| Signal::derive(move || running.get() && test.phase.get() == p);
    let loaded_detail = move |v: RwSignal<Vec<f64>>| {
        Signal::derive(move || {
            v.with(|v| crate::model::stats::median(v)).map(|m| {
                let (s, u) = format_ms(m);
                format!("Under load {s} {u}")
            })
        })
    };
    let loss_detail = Signal::derive(move || match (test.loss_counts.get(), status.get()) {
        (Some(c), _) => Some(format!("{} of {} lost", c.lost, c.sent)),
        (None, RunStatus::Done) => Some("Unavailable".into()),
        _ => None,
    });
    let latency_detail = Signal::derive(move || {
        let n = test.latency_samples.with(Vec::len);
        (n > 0).then(|| format!("Median of {n} probes"))
    });
    let loss_reason = Signal::derive(move || {
        test.steps
            .with(|s| loss_reason(&test.profile_name.get(), s))
    });

    view! {
        <div
            class="test-page"
            class:is-revealed=move || done.get() && !group_mode.get()
            class:is-group=move || group_mode.get()
        >
            <Hero
                title=hero_title
                value=hero_value
                kind=hero_kind
                caption=hero_caption
                timeline=timeline_steps
                flow_mode=flow_mode
                flow_bps=flow_bps
                pulses=test.pulses
                idle=Signal::derive(move || {
                    if group_mode.get() {
                        matches!(stage.get(), GroupStage::Countdown { .. })
                    } else {
                        status.get() == RunStatus::Idle
                    }
                })
                others=others_flow
                vt_name=vt_name
            >
                <Countdown/>
                {move || {
                    if group_mode.get() {
                        view! { <GroupActions/> }.into_any()
                    } else {
                        view! {
                            <div class="hero-actions">
                                <Show when=move || test.history_on()>
                                    <LabelPicker
                                        label=test.label
                                        device_id=ui.device_id
                                        disabled=running
                                    />
                                </Show>
                                <button
                                    type="button"
                                    class="start-button"
                                    class:is-idle=move || status.get() == RunStatus::Idle && !test.waiting.get()
                                    class:is-running=running
                                    disabled=move || in_group.get() && !running.get()
                                    on:click=move |_| test.request_start()
                                >
                                    {button_label}
                                </button>
                                <BusyPrompt test=test/>
                                <SaveNotice test=test/>
                                {move || {
                                    test.shared
                                        .get()
                                        .filter(|_| done.get())
                                        .map(|d| view! { <SharedBadge detail=d long=true/> })
                                }}
                            </div>
                        }
                            .into_any()
                    }
                }}
            </Hero>

            <p class="visually-hidden" aria-live="polite" role="status">
                {move || test.announce.get()}
            </p>

            <Show when=move || !group_mode.get()>
                <DevicesPanel busy=running/>
            </Show>

            <Show
                when=move || !group_mode.get()
                fallback=move || {
                    view! {
                        <Show
                            when=move || group_done.get()
                            fallback=|| view! { <GroupLanes/> }
                        >
                            {move || {
                                presence
                                    .group_result
                                    .get()
                                    .map(|r| {
                                        view! { <GroupResultView result=r me=presence.session.get_untracked().unwrap_or_default()/> }
                                    })
                            }}
                        </Show>
                    }
                }
            >
                <section class="card live-card" aria-label="Live throughput">
                    <LiveArea
                        points=test.live_points
                        ticks=test.payload_ticks
                        empty_text=Signal::derive(move || {
                            if running.get() {
                                "Waiting for the first transfer…".to_string()
                            } else {
                                "Throughput appears here while the test runs.".to_string()
                            }
                        })
                    />
                </section>

                <section class="metric-grid" aria-label="Results">
                    <MetricTile
                        label="Download"
                        info=INFO_DOWNLOAD
                        kind=MetricKind::Rate
                        value=test.download
                        series=Series::Download
                        samples=test.down_samples
                        detail=loaded_detail(test.down_loaded)
                        active=phase_is(Phase::Download)
                        index=0
                    />
                    <MetricTile
                        label="Upload"
                        info=INFO_UPLOAD
                        kind=MetricKind::Rate
                        value=test.upload
                        series=Series::Upload
                        samples=test.up_samples
                        detail=loaded_detail(test.up_loaded)
                        active=phase_is(Phase::Upload)
                        index=1
                    />
                    <MetricTile
                        label="Latency"
                        info=INFO_LATENCY
                        kind=MetricKind::Ms
                        value=test.latency
                        series=Series::Latency
                        samples=test.latency_samples
                        detail=latency_detail
                        active=phase_is(Phase::Latency)
                        index=2
                    />
                    <MetricTile
                        label="Jitter"
                        info=INFO_JITTER
                        kind=MetricKind::Ms
                        value=test.jitter
                        series=Series::Latency
                        samples=test.jitter_samples
                        active=phase_is(Phase::Latency)
                        index=3
                    />
                    <MetricTile
                        label="Packet loss"
                        info=INFO_LOSS
                        kind=MetricKind::Percent
                        value=test.packet_loss
                        detail=loss_detail
                        active=phase_is(Phase::PacketLoss)
                        index=4
                    />
                </section>

                <div class="card-row">
                    <QualityCard scores=test.scores/>
                    <ServerCard
                        info=test.info
                        transport=ui.transport
                        profile=ui.profile
                        streams=ui.streams
                    />
                </div>

                <Show when=move || test.result.with(Option::is_some)>
                    <ResultDetails result=test.result loss_reason=loss_reason/>
                </Show>
            </Show>
        </div>
    }
}

/// Group test controls in the hero: Leave while running, Done afterwards.
#[component]
fn GroupActions() -> impl IntoView {
    let presence = use_presence();
    let stage = presence.stage;
    view! {
        <div class="hero-actions">
            {move || match stage.get() {
                GroupStage::Done | GroupStage::Failed(_) => {
                    view! {
                        <button
                            type="button"
                            class="start-button"
                            on:click=move |_| presence.leave_group()
                        >
                            "Done"
                        </button>
                    }
                        .into_any()
                }
                _ => {
                    view! {
                        <button
                            type="button"
                            class="start-button is-running"
                            on:click=move |_| presence.leave_group()
                        >
                            "Leave"
                        </button>
                    }
                        .into_any()
                }
            }}
        </div>
    }
}

/// "Other devices are testing": Wait for them / Test anyway (SPEC §7.11).
#[component]
fn BusyPrompt(test: TestController) -> impl IntoView {
    let presence = test.presence;
    let names = move || {
        let others = presence.others_testing();
        let hidden = presence.hidden_active.get();
        let mut parts: Vec<String> = others.iter().map(|p| p.device_name.clone()).collect();
        if hidden > 0 {
            parts.push(if hidden == 1 {
                "a hidden device".to_string()
            } else {
                format!("{hidden} hidden devices")
            });
        }
        match parts.len() {
            0 => "Another device".to_string(),
            1 => parts.remove(0),
            _ => {
                let last = parts.pop().unwrap_or_default();
                format!("{} and {last}", parts.join(", "))
            }
        }
    };
    let busy = Signal::derive(move || {
        presence.others_testing().len() + presence.hidden_active.get() as usize
    });
    view! {
        {move || {
            if test.waiting.get() {
                view! {
                    <div class="busy-prompt is-waiting" role="status">
                        <span class="spinner" aria-hidden="true"></span>
                        <p>{format!("Waiting for {} to finish…", names())}</p>
                        <button
                            type="button"
                            class="link-button"
                            on:click=move |_| test.waiting.set(false)
                        >
                            "Cancel"
                        </button>
                    </div>
                }
                    .into_any()
            } else if test.prompt.get() {
                let verb = if busy.get() == 1 { "is" } else { "are" };
                view! {
                    <div class="busy-prompt" role="alertdialog" aria-labelledby="busy-text">
                        <p id="busy-text">
                            <strong>{format!("{} {verb} testing right now.", names())}</strong>
                            " Testing at the same time shares the network, so both results come out lower."
                        </p>
                        <div class="busy-actions">
                            <button
                                type="button"
                                class="pill-button pill-sm"
                                on:click=move |_| {
                                    test.prompt.set(false);
                                    test.waiting.set(true);
                                }
                            >
                                "Wait for them"
                            </button>
                            <button
                                type="button"
                                class="pill-button pill-sm pill-ghost"
                                on:click=move |_| test.toggle()
                            >
                                "Test anyway"
                            </button>
                        </div>
                    </div>
                }
                    .into_any()
            } else {
                ().into_any()
            }
        }}
    }
}

/// "Saved · Edit label" with inline rename (`PATCH /api/runs/{id}`).
#[component]
fn SaveNotice(test: TestController) -> impl IntoView {
    let editing = RwSignal::new(false);
    let draft = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let rename_input = NodeRef::<leptos::html::Input>::new();
    Effect::new(move |_| {
        if editing.get()
            && let Some(el) = rename_input.get()
        {
            let _ = el.focus();
            el.select();
        }
    });

    let rename = move || {
        let SaveState::Saved(id) = test.save.get_untracked() else {
            return;
        };
        let value = clean_label(&draft.get_untracked());
        editing.set(false);
        if value.as_deref() == Some(test.saved_label.get_untracked().as_str()) {
            return;
        }
        spawn_local(async move {
            match api::rename_run(id, value.as_deref()).await {
                Ok(run) => {
                    let l = run.label.unwrap_or_default();
                    if !l.is_empty() {
                        storage_set(keys::LAST_LABEL, &l);
                        test.label.set(l.clone());
                    }
                    test.saved_label.set(l);
                    error.set(None);
                }
                Err(e) => error.set(Some(e)),
            }
        });
    };

    view! {
        <div class="save-notice" aria-live="polite">
            {move || match test.save.get() {
                SaveState::Idle => ().into_any(),
                SaveState::Saving => view! { <span class="muted">"Saving…"</span> }.into_any(),
                SaveState::Failed(e) => {
                    view! {
                        <span class="save-error" title=e>
                            "Couldn't save this run"
                        </span>
                    }
                        .into_any()
                }
                SaveState::Saved(_) if editing.get() => {
                    view! {
                        <form
                            class="rename-form"
                            on:submit=move |ev| {
                                ev.prevent_default();
                                rename();
                            }
                        >
                            <label class="visually-hidden" for="rename-input">
                                "Label"
                            </label>
                            <input
                                id="rename-input"
                                class="text-input text-input-sm"
                                type="text"
                                maxlength="100"
                                node_ref=rename_input
                                prop:value=move || draft.get()
                                on:input=move |ev| draft.set(event_target_value(&ev))
                                on:keydown=move |ev| {
                                    if ev.key() == "Escape" {
                                        editing.set(false);
                                    }
                                }
                            />
                            <button type="submit" class="pill-button pill-sm">
                                "Save"
                            </button>
                        </form>
                    }
                        .into_any()
                }
                SaveState::Saved(_) => {
                    view! {
                        <span class="saved">
                            <span class="saved-check">{icons::check()}</span>
                            "Saved"
                            {move || {
                                let l = test.saved_label.get();
                                (!l.is_empty()).then(|| format!(" as “{l}”"))
                            }}
                        </span>
                        <span class="dot-sep" aria-hidden="true">
                            "·"
                        </span>
                        <button
                            type="button"
                            class="link-button"
                            on:click=move |_| {
                                draft.set(test.saved_label.get_untracked());
                                editing.set(true);
                            }
                        >
                            "Edit label"
                        </button>
                        {move || error.get().map(|_| view! { <span class="save-error">"Rename failed"</span> })}
                    }
                        .into_any()
                }
            }}
        </div>
    }
}

/// Today's (day, month) in the browser's time zone; (0, 0) on the server.
fn today() -> (u32, u32) {
    #[cfg(feature = "hydrate")]
    {
        let d = js_sys::Date::new_0();
        (d.get_date(), d.get_month() + 1)
    }
    #[cfg(not(feature = "hydrate"))]
    {
        (0, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_labels() {
        assert_eq!(
            step_label(&Step::download(10_000_000, 6)),
            "Download · 10 MB × 6"
        );
        assert_eq!(step_label(&Step::upload(100_000, 8)), "Upload · 100 kB × 8");
        assert_eq!(
            step_label(&Step::Latency { num_packets: 1 }),
            "Latency · 1 probe"
        );
        assert_eq!(
            step_label(&Step::default_packet_loss()),
            "Packet loss · 1000 packets"
        );
        assert_eq!(
            step_label(&Step::Pause { duration_ms: 2500 }),
            "Pause · 2.5 s"
        );
    }

    #[test]
    fn loss_reasons() {
        assert_eq!(
            loss_reason("quick", &Profile::quick().steps),
            "The Quick profile doesn't measure packet loss."
        );
        assert!(loss_reason("standard", &Profile::standard().steps).contains("WebRTC"));
    }

    #[test]
    fn units_per_step() {
        assert_eq!(step_units(&Step::Latency { num_packets: 20 }), 20);
        assert_eq!(step_units(&Step::download(1, 6)), 6);
        assert_eq!(step_units(&Step::default_packet_loss()), 0);
    }
}
