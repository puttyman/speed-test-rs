//! Executes a [`Profile`] (SPEC §5.3): walks it with [`RampState`] (ramp
//! halting, time cap), feeds [`Measurements`] and builds the [`RunResult`].
//!
//! **Timed steps** (the group `concurrent` profile, SPEC §5.11) follow a
//! schedule: consecutive timed steps end at `start + Σ durations`, measured
//! from the first one's start (the group start instant when the run was
//! started with [`start_at`](super::start_at)), so every device loads the
//! network at the same moments even if a probe or request runs a little long.
//!
//! - `TimedLatency`: idle probes, 50 ms apart, until the step's end.
//! - `TimedDownload` / `TimedUpload`: **back-to-back requests**, not one huge
//!   request aborted at the deadline. Each request is sized from the rate of
//!   the previous one so that it ends about when the step ends (and doubles
//!   while requests are shorter than 250 ms); no request starts within 30 ms
//!   of the end. This keeps the data sockets' request/ack protocol intact (no
//!   aborts, no drained leftovers) while overrunning the schedule by at most a
//!   fraction of one request. Every request is recorded as a bandwidth sample
//!   and the live meter runs throughout. Loaded-latency probes run for the
//!   whole step, their elapsed time counted from the step's start (not from
//!   each short request), so `loaded_request_min_ms` only drops the first
//!   250 ms of the step.
//! - `Pause`: waits until the step's scheduled end.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::pin;
use std::rc::Rc;

use futures::future::{Either, join, pending, select};
use uuid::Uuid;

use super::http_transport::HttpTransport;
use super::js::{AbortFlag, EngineError, Result, location_origin, now, sleep, window};
use super::loaded_latency::{live_loop, probe_loop};
use super::util::next_timed_size;
use super::ws_transport::{WsTransport, open};
use super::{
    EngineEvent, EngineOptions, PacketLossContext, PacketLossParams, Sample, UploadBuffer,
};
use crate::model::profile::{Config, Direction, PhaseStatus, RampState, Step};
use crate::model::protocol::{ClientMsg, Limits, Phase, Role};
use crate::model::result::{
    BandwidthPoint, ClientInfo, Measurements, RunResult, Transport, summarize,
};

/// Gap between probes of a timed latency step.
const TIMED_LATENCY_GAP_MS: u32 = 50;
/// No new request of a timed transfer starts this close to the step's end.
const TIMED_MIN_REMAINING_MS: f64 = 30.0;
/// First request size of a timed transfer.
const TIMED_FIRST_BYTES: u64 = 1_000_000;

/// The measurement link of a run.
pub enum Link {
    Ws(WsTransport),
    Http(HttpTransport),
}

impl Link {
    pub async fn ping(&self) -> Result<f64> {
        match self {
            Link::Ws(t) => t.ping().await,
            Link::Http(t) => t.ping().await,
        }
    }

    pub async fn transfer(&self, dir: Direction, bytes: u64, streams: u8) -> Result<Sample> {
        match (self, dir) {
            (Link::Ws(t), Direction::Download) => t.download(bytes, streams).await,
            (Link::Ws(t), Direction::Upload) => t.upload(bytes, streams).await,
            (Link::Http(t), Direction::Download) => t.download(bytes, streams).await,
            (Link::Http(t), Direction::Upload) => t.upload(bytes, streams).await,
        }
    }

    pub fn progress(&self, dir: Direction) -> u64 {
        let down = dir == Direction::Download;
        match self {
            Link::Ws(t) => t.progress(down),
            Link::Http(t) => t.progress(down),
        }
    }

    fn between_steps(&self) {
        if let Link::Http(t) = self {
            t.between_steps();
        }
    }
}

/// Runs `opts` to completion or abort, then emits exactly one of
/// `Finished`, `Error` or `Aborted`. Sockets are closed before that event.
///
/// With `start_at` (a `performance.now()` time), the sockets are opened right
/// away and the first step waits until then (group countdown).
pub async fn run(
    opts: EngineOptions,
    emit: Rc<dyn Fn(EngineEvent)>,
    abort: AbortFlag,
    start_at: Option<f64>,
) {
    let outcome = {
        let work = pin!(run_inner(&opts, &*emit, start_at));
        match select(work, pin!(abort.wait())).await {
            Either::Left((r, _)) => Some(r),
            Either::Right(_) => None,
        }
        // `work` (and every socket it owns) is dropped here.
    };
    match outcome {
        None | Some(Err(EngineError::Aborted)) => emit(EngineEvent::Aborted),
        Some(Ok(result)) => emit(EngineEvent::Finished(Box::new(result))),
        Some(Err(e)) => emit(EngineEvent::Error(e.to_string())),
    }
}

/// Shared state of a run.
struct Run<'a> {
    opts: &'a EngineOptions,
    emit: &'a dyn Fn(EngineEvent),
    link: Link,
    limits: Limits,
    t0: f64,
    measurements: RefCell<Measurements>,
    request_start: Cell<f64>,
    origin: String,
    /// Scheduled end of the previous timed step (`None` after a sized step).
    timed_clock: Cell<Option<f64>>,
}

async fn run_inner(
    opts: &EngineOptions,
    emit: &dyn Fn(EngineEvent),
    start_at: Option<f64>,
) -> Result<RunResult> {
    let mut t0 = now();
    let config = &opts.config;
    let origin = if opts.origin.is_empty() {
        location_origin().ok_or_else(|| EngineError::Connect("no page origin".into()))?
    } else {
        opts.origin.trim_end_matches('/').to_string()
    };

    // The control socket announces the device and carries status updates
    // and WebRTC signalling.
    let control = Rc::new(
        open(
            &origin,
            "/ws/control",
            opts.session,
            Role::Control,
            opts.auth.clone(),
            Some(opts.device.clone()),
        )
        .await?,
    );
    let limits = control.welcome.limits;
    let max_streams = limits.max_streams.max(1);
    let streams = opts
        .profile
        .steps
        .iter()
        .map(|s| s.streams(config))
        .max()
        .unwrap_or(1)
        .min(max_streams);
    let upload = UploadBuffer::new()?;
    let link = match opts.transport {
        Transport::Websocket => Link::Ws(
            WsTransport::connect(&origin, opts.session, opts.auth.clone(), streams, upload).await?,
        ),
        Transport::Http => Link::Http(HttpTransport::new(
            &origin,
            opts.session,
            opts.auth.clone(),
            upload,
        )?),
    };
    emit(EngineEvent::Connected {
        server_name: control.welcome.server_name.clone(),
        server_version: control.welcome.server_version.clone(),
    });
    // Group countdown: connected early, start on the shared instant. If the
    // connection took longer than the countdown, the schedule still counts
    // from the shared instant, so this device catches up.
    let mut timed_clock = None;
    if let Some(at) = start_at {
        let wait = at - now();
        if wait > 0.0 {
            sleep(wait.ceil() as u32).await;
        }
        t0 = now();
        timed_clock = Some(at);
    }

    let run = Run {
        opts,
        emit,
        link,
        limits,
        t0,
        measurements: RefCell::new(Measurements::new()),
        request_start: Cell::new(f64::NAN),
        origin,
        timed_clock: Cell::new(timed_clock),
    };
    let mut ramp = RampState::new(&opts.profile, config);
    while let Some((index, step)) = ramp.next_step(now() - t0) {
        let step = step.clone();
        emit(EngineEvent::PhaseStarted {
            index,
            step: step.clone(),
        });
        // Presence update; failure to send is not fatal.
        let _ = control.send(&ClientMsg::Status {
            phase: Some(step.phase()),
            label: opts.label.clone(),
        });
        let status = match run.step(&step, &mut ramp, &control).await {
            Ok(s) => s,
            Err(e) => {
                ramp.complete_step(index, PhaseStatus::Failed);
                emit(EngineEvent::PhaseFinished {
                    index,
                    status: PhaseStatus::Failed,
                });
                return Err(e);
            }
        };
        ramp.complete_step(index, status);
        run.link.between_steps();
        emit(EngineEvent::PhaseFinished { index, status });
    }
    // Time cap or end of profile: anything left is skipped.
    ramp.skip_remaining();
    let _ = control.send(&ClientMsg::Status {
        phase: Some(Phase::Idle),
        label: opts.label.clone(),
    });

    let m = run.measurements.into_inner();
    let summary = summarize(&m, config);
    let ua = window().navigator().user_agent().ok();
    Ok(RunResult {
        id: Uuid::now_v7(),
        created_at: String::from(js_sys::Date::new_0().to_iso_string()),
        label: None,
        notes: None,
        transport: opts.transport,
        profile: opts.profile.name.clone(),
        streams,
        client: ClientInfo {
            ip: None,
            user_agent: ua,
            device_id: Some(opts.device.device_id.clone()),
            device_name: Some(opts.device.device_name.clone()),
        },
        tls: run.origin.starts_with("https:"),
        group_id: None,
        concurrency: Default::default(),
        summary,
        scores: summary.scores(),
        points: m.points(config),
        phases: ramp.phase_records(),
        duration_ms: (now() - t0).max(0.0) as u64,
    })
}

impl Run<'_> {
    fn config(&self) -> &Config {
        &self.opts.config
    }

    fn elapsed(&self) -> f64 {
        now() - self.t0
    }

    async fn step(
        &self,
        step: &Step,
        ramp: &mut RampState,
        control: &Rc<super::socket::Socket>,
    ) -> Result<PhaseStatus> {
        match *step {
            Step::Latency { num_packets } => {
                self.timed_clock.set(None);
                let mut done = 0;
                for _ in 0..num_packets {
                    if ramp.time_up(self.elapsed()) {
                        break;
                    }
                    self.idle_probe().await?;
                    done += 1;
                }
                Ok(status_of(done))
            }
            Step::Download { .. } | Step::Upload { .. } => {
                self.timed_clock.set(None);
                self.transfer_step(step, ramp).await
            }
            Step::PacketLoss {
                num_packets,
                batch_size,
                batch_wait_ms,
                responses_wait_ms,
                connection_timeout_ms,
            } => {
                self.timed_clock.set(None);
                let Some(probe) = &self.opts.packet_loss else {
                    return Ok(PhaseStatus::Skipped);
                };
                let ctx = PacketLossContext {
                    control: control.clone(),
                    origin: self.origin.clone(),
                    session: self.opts.session,
                };
                let params = PacketLossParams {
                    num_packets,
                    batch_size,
                    batch_wait_ms,
                    responses_wait_ms,
                    connection_timeout_ms,
                };
                match probe.run(ctx, params).await {
                    Ok(counts) => {
                        self.measurements.borrow_mut().set_packet_loss(Some(counts));
                        (self.emit)(EngineEvent::PacketLoss(counts));
                        Ok(PhaseStatus::Done)
                    }
                    // Unavailable, not 0% (SPEC §5.7).
                    Err(_) => Ok(PhaseStatus::Failed),
                }
            }
            Step::TimedLatency { duration_ms } => {
                let end = self.timed_window(duration_ms).await;
                let mut done = 0;
                while now() < end && !ramp.time_up(self.elapsed()) {
                    self.idle_probe().await?;
                    done += 1;
                    let left = end - now();
                    if left > 0.0 {
                        sleep((left.ceil() as u32).min(TIMED_LATENCY_GAP_MS)).await;
                    }
                }
                Ok(status_of(done))
            }
            Step::TimedDownload { duration_ms } | Step::TimedUpload { duration_ms } => {
                let end = self.timed_window(duration_ms).await;
                self.timed_transfer(step, end, ramp).await
            }
            Step::Pause { duration_ms } => {
                let end = self.timed_window(duration_ms).await;
                let left = end - now();
                if left > 0.0 {
                    sleep(left.ceil() as u32).await;
                }
                Ok(PhaseStatus::Done)
            }
        }
    }

    async fn idle_probe(&self) -> Result<()> {
        let ms = self.link.ping().await?;
        self.measurements.borrow_mut().push_latency(ms);
        (self.emit)(EngineEvent::Latency { ms });
        Ok(())
    }

    /// Effective streams of a step, within the server limit.
    fn streams(&self, step: &Step) -> u8 {
        step.streams(self.config())
            .min(self.limits.max_streams.max(1))
    }

    /// The end of a timed step of `duration_ms` on the schedule (see the
    /// module docs). Waits for the step's scheduled start if it lies ahead.
    async fn timed_window(&self, duration_ms: u32) -> f64 {
        let begin = self.timed_clock.get().unwrap_or_else(now);
        let ahead = begin - now();
        if ahead > 0.0 {
            sleep(ahead.ceil() as u32).await;
        }
        let end = begin + f64::from(duration_ms);
        self.timed_clock.set(Some(end));
        end
    }

    /// Runs one request and records it. The loaded-latency clock starts with
    /// the request.
    async fn request(
        &self,
        dir: Direction,
        bytes: u64,
        streams: u8,
        bypass: bool,
    ) -> Result<Sample> {
        self.request_start.set(now());
        let r = self.request_in_step(dir, bytes, streams, bypass).await;
        self.request_start.set(f64::NAN);
        r
    }

    /// Runs one request and records it, leaving the loaded-latency clock alone.
    async fn request_in_step(
        &self,
        dir: Direction,
        bytes: u64,
        streams: u8,
        bypass: bool,
    ) -> Result<Sample> {
        let sample = self.link.transfer(dir, bytes, streams).await?;
        let point = BandwidthPoint::new(bytes, sample.duration_ms, sample.server_ms);
        self.measurements
            .borrow_mut()
            .push_bandwidth(dir, point, bypass);
        (self.emit)(EngineEvent::Bandwidth {
            direction: dir,
            point,
        });
        Ok(sample)
    }

    /// Runs `main` with the live meter and loaded-latency probes alongside.
    async fn with_side_tasks<T>(&self, dir: Direction, main: impl Future<Output = T>) -> T {
        let side = async {
            join(
                live_loop(&self.link, dir, self.emit),
                probe_loop(
                    &self.link,
                    dir,
                    self.config().loaded_latency_throttle_ms.max(1),
                    &self.request_start,
                    &self.measurements,
                    self.emit,
                ),
            )
            .await;
            pending::<()>().await;
        };
        match select(pin!(main), pin!(side)).await {
            Either::Left((r, _)) => r,
            Either::Right(_) => unreachable!("side tasks never finish"),
        }
    }

    async fn transfer_step(&self, step: &Step, ramp: &mut RampState) -> Result<PhaseStatus> {
        let (Some(dir), Some(bytes)) = (step.direction(), step.bytes()) else {
            return Ok(PhaseStatus::Skipped);
        };
        let streams = self.streams(step);
        if bytes.div_ceil(u64::from(streams)) > self.limits.max_bytes_per_request {
            return Ok(PhaseStatus::Skipped);
        }
        let bypass = step.bypass_min_duration();
        let count = step.count();
        let main = async {
            let mut done = 0;
            for _ in 0..count {
                if ramp.time_up_for(step, self.elapsed()) {
                    break;
                }
                let s = self.request(dir, bytes, streams, bypass).await?;
                ramp.record_request(dir, bytes, s.duration_ms);
                done += 1;
            }
            Ok(status_of(done))
        };
        self.with_side_tasks(dir, main).await
    }

    /// Back-to-back requests until `end`, each sized to finish about when the
    /// step does (see the module docs).
    async fn timed_transfer(
        &self,
        step: &Step,
        end: f64,
        ramp: &mut RampState,
    ) -> Result<PhaseStatus> {
        let Some(dir) = step.direction() else {
            return Ok(PhaseStatus::Skipped);
        };
        let streams = self.streams(step);
        let cap = (self.limits.max_bytes_per_request * u64::from(streams)).min(1_000_000_000);
        let main = async {
            // Loaded latency counts from the start of the step.
            self.request_start.set(now());
            let mut size = TIMED_FIRST_BYTES.min(cap);
            let mut done = 0;
            let result = loop {
                let left = end - now();
                if left < TIMED_MIN_REMAINING_MS || ramp.time_up(self.elapsed()) {
                    break Ok(status_of(done));
                }
                let s = match self.request_in_step(dir, size, streams, false).await {
                    Ok(s) => s,
                    Err(e) => break Err(e),
                };
                done += 1;
                size = next_timed_size(size, s.duration_ms, end - now(), cap);
            };
            self.request_start.set(f64::NAN);
            result
        };
        self.with_side_tasks(dir, main).await
    }
}

fn status_of(done: u32) -> PhaseStatus {
    if done > 0 {
        PhaseStatus::Done
    } else {
        PhaseStatus::Skipped
    }
}
