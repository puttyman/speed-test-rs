//! Executes a [`Profile`] (SPEC §5.3): walks it with [`RampState`] (ramp
//! halting, time cap), feeds [`Measurements`] and builds the [`RunResult`].

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::pin;
use std::rc::Rc;

use futures::future::{Either, join, pending, select};
use uuid::Uuid;

use super::http_transport::HttpTransport;
use super::js::{AbortFlag, EngineError, Result, location_origin, now, sleep, window};
use super::loaded_latency::{live_loop, probe_loop};
use super::ws_transport::{WsTransport, open};
use super::{
    EngineEvent, EngineOptions, PacketLossContext, PacketLossParams, Sample, UploadBuffer,
};
use crate::model::profile::{Config, Direction, PhaseStatus, RampState, Step};
use crate::model::protocol::{ClientMsg, Limits, Phase, Role};
use crate::model::result::{
    BandwidthPoint, ClientInfo, Measurements, RunResult, Transport, summarize,
};

/// Timed steps grow the request size while requests are shorter than this.
const TIMED_GROW_BELOW_MS: f64 = 250.0;
/// Gap between probes of a timed latency step.
const TIMED_LATENCY_GAP_MS: u32 = 50;

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
pub async fn run(opts: EngineOptions, emit: Rc<dyn Fn(EngineEvent)>, abort: AbortFlag) {
    let outcome = {
        let work = pin!(run_inner(&opts, &*emit));
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
}

async fn run_inner(opts: &EngineOptions, emit: &dyn Fn(EngineEvent)) -> Result<RunResult> {
    let t0 = now();
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

    let run = Run {
        opts,
        emit,
        link,
        limits,
        t0,
        measurements: RefCell::new(Measurements::new()),
        request_start: Cell::new(f64::NAN),
        origin,
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
            label: None,
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
        label: None,
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
            Step::Download { .. } | Step::Upload { .. } => self.transfer_step(step, ramp).await,
            Step::PacketLoss {
                num_packets,
                batch_size,
                batch_wait_ms,
                responses_wait_ms,
                connection_timeout_ms,
            } => {
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
                let start = now();
                let mut done = 0;
                while now() - start < f64::from(duration_ms) && !ramp.time_up(self.elapsed()) {
                    self.idle_probe().await?;
                    done += 1;
                    sleep(TIMED_LATENCY_GAP_MS).await;
                }
                Ok(status_of(done))
            }
            Step::TimedDownload { duration_ms } | Step::TimedUpload { duration_ms } => {
                self.timed_transfer(step, duration_ms, ramp).await
            }
            Step::Pause { duration_ms } => {
                sleep(duration_ms).await;
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

    /// Runs one request and records it.
    async fn request(
        &self,
        dir: Direction,
        bytes: u64,
        streams: u8,
        bypass: bool,
    ) -> Result<Sample> {
        self.request_start.set(now());
        let r = self.link.transfer(dir, bytes, streams).await;
        self.request_start.set(f64::NAN);
        let sample = r?;
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
                if ramp.time_up(self.elapsed()) {
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

    /// Back-to-back requests for `duration_ms`, growing the size while they
    /// are short.
    async fn timed_transfer(
        &self,
        step: &Step,
        duration_ms: u32,
        ramp: &mut RampState,
    ) -> Result<PhaseStatus> {
        let Some(dir) = step.direction() else {
            return Ok(PhaseStatus::Skipped);
        };
        let streams = self.streams(step);
        let cap = (self.limits.max_bytes_per_request * u64::from(streams)).min(1_000_000_000);
        let main = async {
            let start = now();
            let mut size = 1_000_000u64.min(cap);
            let mut done = 0;
            while now() - start < f64::from(duration_ms) && !ramp.time_up(self.elapsed()) {
                let s = self.request(dir, size, streams, false).await?;
                done += 1;
                if s.duration_ms < TIMED_GROW_BELOW_MS && size * 2 <= cap {
                    size *= 2;
                }
            }
            Ok(status_of(done))
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
