//! Group tests (SPEC §5.11 "Group test"): synchronized tests of several devices, measured by
//! the server from its own byte counters.
//!
//! Lifecycle and messages (all on control sockets):
//!
//! 1. `GroupCreate` → a new group in `Forming` with the sender as host (it leaves any other
//!    group first). The host gets `GroupUpdate`; every other control socket gets
//!    `GroupInvite{group, host: <host device name>}`.
//! 2. `GroupJoin{group}` → the sender joins (leaving any other group); every member gets
//!    `GroupUpdate{group, host, members, state}`. Joining a group that isn't forming is a
//!    `Conflict`, an unknown group `NotFound`.
//! 3. `GroupLeave` (or the last control socket of a member closing) → the member leaves;
//!    remaining members and the leaver get `GroupUpdate`. When the host leaves a forming or
//!    starting group, the group is `Cancelled`. Once the group is `Running`, a member who
//!    leaves (host included) is only marked incomplete, so the others' measurement survives.
//! 4. `GroupStart` (host only, `Conflict` otherwise; at least [`MIN_MEMBERS`] members) →
//!    every member gets `GroupStart{group, starts_in_ms: 3000, plan: Profile::concurrent()}`
//!    and `GroupUpdate{state: Starting}`; at the start instant the state becomes `Running`
//!    (another `GroupUpdate`).
//! 5. During the plan the server samples every member session's byte counters every
//!    [`SAMPLE_INTERVAL`](super::sessions::SAMPLE_INTERVAL). Windows are aligned from the
//!    start instant with the plan's step durations (3 s latency, 10 s download, 2 s pause,
//!    10 s upload). The first [`RAMP_SKIP_MS`] of each transfer window is ignored (TCP
//!    ramp-up and the clients' request-size growth), so a member's throughput is
//!    `Δbytes × 8 / Δt` over the remaining 9 s, with counters linearly interpolated at the
//!    window edges.
//! 6. After the plan, the server waits up to [`RESULT_GRACE`] for the members' runs to be
//!    posted (`POST /api/runs?session=`), which carry each device's loaded latency (the
//!    server can't time the clients' probes). It then computes [`GroupResult::compute`]
//!    (aggregate, shares, Jain's fairness, server bottleneck), sends
//!    `GroupResult{group, result}` and `GroupUpdate{state: Finished}` to the members, and
//!    stores the result when history is on. Members whose sockets all closed (or who left)
//!    during the test are `incomplete`.
//!
//! Timeouts: forming groups are cancelled after [`FORMING_TTL`]; a group still not finished
//! [`RESULT_GRACE`] + [`STUCK_GRACE`] after its plan ended is cancelled; finished and
//! cancelled groups are forgotten after [`KEEP_DONE`].
//!
//! With presence off (`--no-presence`), every group message is answered with
//! `Error{NotFound, "presence disabled"}`.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::ops::Range;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};
use uuid::Uuid;

use super::config::Config;
use super::presence::{UNKNOWN_DEVICE, peer_of};
use super::sessions::{SAMPLE_INTERVAL, Session, SessionRegistry};
use super::storage::Storage;
use crate::model::profile::{Profile, Step};
use crate::model::protocol::{ErrorCode, GroupState, Peer, ServerMsg};
use crate::model::result::{GroupMemberInput, GroupResult, RateSample, RunResult};

/// Countdown between `GroupStart` and the start of the plan.
pub const COUNTDOWN_MS: u32 = 3000;
/// A group needs at least this many members to start.
pub const MIN_MEMBERS: usize = 2;
/// Forming groups expire after this long.
pub const FORMING_TTL: Duration = Duration::from_secs(5 * 60);
/// How long to wait after the plan for the members' posted runs (loaded latency).
pub const RESULT_GRACE: Duration = Duration::from_secs(5);
/// Extra safety margin before a group that never finished is cancelled.
pub const STUCK_GRACE: Duration = Duration::from_secs(5);
/// Finished and cancelled groups are kept this long (for late `POST /api/runs`).
pub const KEEP_DONE: Duration = Duration::from_secs(10 * 60);
/// The start of each transfer window that is ignored (ramp-up).
pub const RAMP_SKIP_MS: u64 = 1000;
/// A posted run is attributed to a finished group only this long after it ended.
const REPORT_WINDOW: Duration = Duration::from_secs(2 * 60);

/// A refused group operation (sent back as `Error`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupError {
    pub code: ErrorCode,
    pub message: &'static str,
}

impl GroupError {
    /// The `Error` frame for this refusal.
    pub fn to_msg(&self) -> ServerMsg {
        ServerMsg::error(self.code, self.message)
    }
}

fn err(code: ErrorCode, message: &'static str) -> GroupError {
    GroupError { code, message }
}

/// Loaded latency reported by a member's posted run.
#[derive(Debug, Clone, Copy, Default)]
struct Report {
    down_loaded_latency_ms: Option<f64>,
    up_loaded_latency_ms: Option<f64>,
}

struct Group {
    id: Uuid,
    host: Uuid,
    /// Join order; the host first.
    members: Vec<Uuid>,
    /// Members that left or disconnected after `GroupStart`.
    left: HashSet<Uuid>,
    state: GroupState,
    created: Instant,
    /// When the plan starts (set by `GroupStart`).
    start_at: Option<Instant>,
    /// When the plan ends.
    plan_end: Option<Instant>,
    /// When the group reached `Finished` or `Cancelled`.
    done_at: Option<Instant>,
    plan: Profile,
    reports: HashMap<Uuid, Report>,
    task: Option<JoinHandle<()>>,
}

impl Group {
    fn active(&self) -> bool {
        matches!(
            self.state,
            GroupState::Forming | GroupState::Starting | GroupState::Running
        )
    }

    fn current_members(&self) -> impl Iterator<Item = Uuid> + '_ {
        self.members
            .iter()
            .copied()
            .filter(|m| !self.left.contains(m))
    }
}

struct Inner {
    registry: SessionRegistry,
    enabled: bool,
    server_link_mbps: Option<u32>,
    groups: Mutex<HashMap<Uuid, Group>>,
}

/// The group test manager. Cheap to clone.
#[derive(Clone)]
pub struct Groups {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Groups {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Groups")
            .field("enabled", &self.inner.enabled)
            .finish_non_exhaustive()
    }
}

/// Context of a `GroupStart`, from the host's connection.
#[derive(Clone, Debug, Default)]
pub struct StartContext {
    /// The run history, to store the result (`None` = history off).
    pub storage: Option<Storage>,
    /// The host's IP, to find the server interface that serves the clients.
    pub client_ip: Option<IpAddr>,
}

impl Groups {
    /// A manager for `registry`. Groups are disabled when `config.presence` is false.
    pub fn new(registry: SessionRegistry, config: &Config) -> Self {
        Self {
            inner: Arc::new(Inner {
                registry,
                enabled: config.presence,
                server_link_mbps: config.server_link_mbps,
                groups: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn enabled(&self) -> bool {
        self.inner.enabled
    }

    /// Spawns the task that expires and forgets groups (every second). Ends when the
    /// manager is dropped. Returns `None` outside a Tokio runtime.
    pub fn start_reaper(&self) -> Option<JoinHandle<()>> {
        let handle = tokio::runtime::Handle::try_current().ok()?;
        let weak: Weak<Inner> = Arc::downgrade(&self.inner);
        Some(handle.spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                let Some(inner) = weak.upgrade() else { break };
                Groups { inner }.reap(Instant::now());
            }
        }))
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<Uuid, Group>> {
        self.inner.groups.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn check_enabled(&self) -> Result<(), GroupError> {
        if self.inner.enabled {
            Ok(())
        } else {
            Err(err(ErrorCode::NotFound, "presence disabled"))
        }
    }

    fn device_name(&self, session: Uuid) -> String {
        self.inner
            .registry
            .snapshot_of(session)
            .and_then(|s| s.device.map(|d| d.device_name))
            .unwrap_or_else(|| UNKNOWN_DEVICE.to_owned())
    }

    /// The `GroupUpdate` for each current member (personalized `is_you`).
    fn updates(&self, g: &Group, extra: &[Uuid]) -> Vec<(Uuid, ServerMsg)> {
        let peers: Vec<Peer> = g
            .current_members()
            .filter_map(|m| self.inner.registry.snapshot_of(m))
            .map(|s| peer_of(&s))
            .collect();
        let mut recipients: Vec<Uuid> = g.current_members().collect();
        for e in extra {
            if !recipients.contains(e) {
                recipients.push(*e);
            }
        }
        recipients
            .into_iter()
            .map(|r| {
                let members = peers
                    .iter()
                    .map(|p| Peer {
                        is_you: p.id == r,
                        ..p.clone()
                    })
                    .collect();
                (
                    r,
                    ServerMsg::GroupUpdate {
                        group: g.id,
                        host: g.host,
                        members,
                        state: g.state,
                    },
                )
            })
            .collect()
    }

    fn deliver(&self, msgs: Vec<(Uuid, ServerMsg)>) {
        for (to, msg) in msgs {
            self.inner.registry.send_to(to, &msg);
        }
    }

    /// Removes `session` from every active group it is in (before it creates or joins
    /// another one). Returns the messages to deliver.
    fn leave_all(&self, map: &mut HashMap<Uuid, Group>, session: Uuid) -> Vec<(Uuid, ServerMsg)> {
        let ids: Vec<Uuid> = map
            .values()
            .filter(|g| g.active() && g.current_members().any(|m| m == session))
            .map(|g| g.id)
            .collect();
        let mut out = Vec::new();
        for id in ids {
            if let Some(g) = map.get_mut(&id) {
                out.extend(self.leave_group(g, session));
            }
        }
        out
    }

    fn leave_group(&self, g: &mut Group, session: Uuid) -> Vec<(Uuid, ServerMsg)> {
        match g.state {
            GroupState::Forming | GroupState::Starting if session == g.host => {
                let recipients: Vec<Uuid> = g.current_members().collect();
                self.cancel(g);
                self.updates(g, &recipients)
            }
            GroupState::Forming => {
                g.members.retain(|m| *m != session);
                self.updates(g, &[session])
            }
            GroupState::Starting | GroupState::Running => {
                g.left.insert(session);
                self.updates(g, &[session])
            }
            GroupState::Finished | GroupState::Cancelled => Vec::new(),
        }
    }

    fn cancel(&self, g: &mut Group) {
        g.state = GroupState::Cancelled;
        g.done_at = Some(Instant::now());
        if let Some(task) = g.task.take() {
            task.abort();
        }
        tracing::info!(group = %g.id, "group test cancelled");
    }

    /// `GroupCreate` from `host`.
    pub fn create(&self, host: Uuid) -> Result<Uuid, GroupError> {
        self.check_enabled()?;
        let id = Uuid::now_v7();
        let mut map = self.lock();
        let mut out = self.leave_all(&mut map, host);
        let g = Group {
            id,
            host,
            members: vec![host],
            left: HashSet::new(),
            state: GroupState::Forming,
            created: Instant::now(),
            start_at: None,
            plan_end: None,
            done_at: None,
            plan: Profile::concurrent(),
            reports: HashMap::new(),
            task: None,
        };
        out.extend(self.updates(&g, &[]));
        map.insert(id, g);
        drop(map);
        self.deliver(out);
        let invite = ServerMsg::GroupInvite {
            group: id,
            host: self.device_name(host),
        };
        for (session, tx) in self.inner.registry.control_senders() {
            if session != host {
                let _ = tx.try_send(invite.clone());
            }
        }
        tracing::info!(group = %id, %host, "group test created");
        Ok(id)
    }

    /// `GroupJoin{group}` from `session`.
    pub fn join(&self, session: Uuid, group: Uuid) -> Result<(), GroupError> {
        self.check_enabled()?;
        let mut map = self.lock();
        match map.get(&group) {
            None => return Err(err(ErrorCode::NotFound, "no such group")),
            Some(g) if g.state != GroupState::Forming => {
                return Err(err(
                    ErrorCode::Conflict,
                    "this group test has already started",
                ));
            }
            Some(g) if g.members.contains(&session) => {
                let out = self.updates(g, &[]);
                drop(map);
                self.deliver(out);
                return Ok(());
            }
            Some(_) => {}
        }
        let mut out = self.leave_all(&mut map, session);
        let Some(g) = map.get_mut(&group) else {
            return Err(err(ErrorCode::NotFound, "no such group"));
        };
        if g.state != GroupState::Forming {
            // The joiner was the host of this very group and leaving cancelled it.
            drop(map);
            self.deliver(out);
            return Err(err(ErrorCode::Conflict, "this group test was cancelled"));
        }
        g.members.push(session);
        out.extend(self.updates(g, &[]));
        drop(map);
        self.deliver(out);
        Ok(())
    }

    /// `GroupLeave` from `session` (no-op when it isn't in a group).
    pub fn leave(&self, session: Uuid) -> Result<(), GroupError> {
        self.check_enabled()?;
        let mut map = self.lock();
        let out = self.leave_all(&mut map, session);
        drop(map);
        self.deliver(out);
        Ok(())
    }

    /// The last control socket of `session` closed: it leaves its group.
    pub fn disconnected(&self, session: Uuid) {
        if self.inner.enabled {
            let _ = self.leave(session);
        }
    }

    /// `GroupStart` from `session`.
    pub fn start(&self, session: Uuid, ctx: StartContext) -> Result<(), GroupError> {
        self.check_enabled()?;
        let mut map = self.lock();
        let Some(g) = map
            .values_mut()
            .find(|g| g.active() && g.current_members().any(|m| m == session))
        else {
            return Err(err(ErrorCode::NotFound, "not in a group test"));
        };
        if g.host != session {
            return Err(err(ErrorCode::Conflict, "only the host can start the test"));
        }
        if g.state != GroupState::Forming {
            return Err(err(ErrorCode::Conflict, "the test has already started"));
        }
        if g.members.len() < MIN_MEMBERS {
            return Err(err(
                ErrorCode::Conflict,
                "at least two devices are needed to test together",
            ));
        }
        let now = Instant::now();
        let start_at = now + Duration::from_millis(u64::from(COUNTDOWN_MS));
        let plan_ms = plan_duration_ms(&g.plan);
        g.state = GroupState::Starting;
        g.start_at = Some(start_at);
        g.plan_end = Some(start_at + Duration::from_millis(plan_ms));
        let start = ServerMsg::GroupStart {
            group: g.id,
            starts_in_ms: COUNTDOWN_MS,
            plan: g.plan.clone(),
        };
        let mut out: Vec<(Uuid, ServerMsg)> =
            g.members.iter().map(|m| (*m, start.clone())).collect();
        out.extend(self.updates(g, &[]));
        let link_bps = server_link_bps(self.inner.server_link_mbps, ctx.client_ip);
        let members: Vec<(Uuid, Option<Arc<Session>>)> = g
            .members
            .iter()
            .map(|m| (*m, self.inner.registry.get(*m)))
            .collect();
        let task = tokio::spawn(run_group(
            self.clone(),
            g.id,
            start_at,
            g.plan.clone(),
            members,
            ctx.storage,
            link_bps,
        ));
        g.task = Some(task);
        tracing::info!(group = %g.id, members = g.members.len(), "group test starting");
        drop(map);
        self.deliver(out);
        Ok(())
    }

    /// A member posted its run (`POST /api/runs?session=`). Records its loaded latency for
    /// the group result and returns the group ID when the run belongs to a group test: the
    /// session is a member of a running group, or of one that finished less than two
    /// minutes ago, and the run used the group's plan.
    pub fn report_run(&self, session: Uuid, run: &RunResult) -> Option<Uuid> {
        let now = Instant::now();
        let mut map = self.lock();
        let g = map
            .values_mut()
            .filter(|g| {
                g.members.contains(&session)
                    && g.plan.name == run.profile
                    && match g.state {
                        GroupState::Running => true,
                        GroupState::Finished => g
                            .done_at
                            .is_some_and(|d| now.saturating_duration_since(d) < REPORT_WINDOW),
                        _ => false,
                    }
            })
            .max_by_key(|g| g.created)?;
        g.reports.entry(session).or_insert(Report {
            down_loaded_latency_ms: run.summary.down_loaded_latency_ms,
            up_loaded_latency_ms: run.summary.up_loaded_latency_ms,
        });
        Some(g.id)
    }

    /// The state of `group`, if it is known.
    pub fn state_of(&self, group: Uuid) -> Option<GroupState> {
        self.lock().get(&group).map(|g| g.state)
    }

    /// Expires forming groups, cancels stuck ones and forgets old ones.
    fn reap(&self, now: Instant) {
        let mut map = self.lock();
        let mut out = Vec::new();
        for g in map.values_mut() {
            let expired = match g.state {
                GroupState::Forming => now.saturating_duration_since(g.created) >= FORMING_TTL,
                GroupState::Starting | GroupState::Running => g
                    .plan_end
                    .is_some_and(|end| now >= end + RESULT_GRACE + STUCK_GRACE),
                _ => false,
            };
            if expired {
                let recipients: Vec<Uuid> = g.current_members().collect();
                self.cancel(g);
                out.extend(self.updates(g, &recipients));
            }
        }
        map.retain(|_, g| {
            g.done_at
                .is_none_or(|d| now.saturating_duration_since(d) < KEEP_DONE)
        });
        drop(map);
        self.deliver(out);
    }

    /// Moves `group` to `state` (if still active) and sends `GroupUpdate`.
    fn transition(&self, group: Uuid, state: GroupState) -> bool {
        let mut map = self.lock();
        let Some(g) = map.get_mut(&group) else {
            return false;
        };
        if !g.active() {
            return false;
        }
        g.state = state;
        if !g.active() {
            g.done_at = Some(Instant::now());
            g.task = None;
        }
        let out = self.updates(g, &[]);
        drop(map);
        self.deliver(out);
        true
    }

    fn left_members(&self, group: Uuid) -> HashSet<Uuid> {
        self.lock()
            .get(&group)
            .map(|g| g.left.clone())
            .unwrap_or_default()
    }

    fn reports(&self, group: Uuid) -> HashMap<Uuid, Report> {
        self.lock()
            .get(&group)
            .map(|g| g.reports.clone())
            .unwrap_or_default()
    }
}

/// Total duration of the timed steps of `plan`.
pub fn plan_duration_ms(plan: &Profile) -> u64 {
    plan.steps
        .iter()
        .filter_map(Step::duration_ms)
        .map(u64::from)
        .sum()
}

/// The download and upload windows of `plan` (ms from the start), from the step durations.
/// Only the first timed download and upload count. Steps without a fixed duration (sized
/// transfers, packet loss) end the schedule: nothing after them can be aligned.
pub fn transfer_windows(plan: &Profile) -> (Option<Range<u64>>, Option<Range<u64>>) {
    let mut t = 0u64;
    let (mut down, mut up) = (None, None);
    for step in &plan.steps {
        let Some(d) = step.duration_ms().map(u64::from) else {
            break;
        };
        match step {
            Step::TimedDownload { .. } if down.is_none() => down = Some(t..t + d),
            Step::TimedUpload { .. } if up.is_none() => up = Some(t..t + d),
            _ => {}
        }
        t += d;
    }
    (down, up)
}

/// Cumulative counters of one member: `(ms since start, bytes sent, bytes received)`.
type Samples = Vec<(f64, u64, u64)>;

/// Linear interpolation of a cumulative counter at `t`.
fn counter_at(samples: &Samples, t: f64, pick: impl Fn(&(f64, u64, u64)) -> u64) -> Option<f64> {
    let first = samples.first()?;
    if t <= first.0 {
        return Some(pick(first) as f64);
    }
    for w in samples.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        if t <= b.0 {
            let span = b.0 - a.0;
            let f = if span > 0.0 { (t - a.0) / span } else { 1.0 };
            let (va, vb) = (pick(a) as f64, pick(b) as f64);
            return Some(va + (vb - va) * f);
        }
    }
    samples.last().map(|s| pick(s) as f64)
}

/// Throughput over `window` (ms), skipping the ramp at its start.
fn window_bps(
    samples: &Samples,
    window: &Range<u64>,
    pick: impl Fn(&(f64, u64, u64)) -> u64 + Copy,
) -> Option<f64> {
    let len = window.end.saturating_sub(window.start);
    if len == 0 {
        return None;
    }
    let skip = RAMP_SKIP_MS.min(len / 2);
    let a = (window.start + skip) as f64;
    let b = window.end as f64;
    let last = samples.last()?.0;
    if last < b - 1.0 {
        return None;
    }
    let bytes = counter_at(samples, b, pick)? - counter_at(samples, a, pick)?;
    Some((bytes.max(0.0) * 8.0) / ((b - a) / 1000.0))
}

/// Per-interval rates for the result's time series.
fn series(samples: &Samples) -> Vec<RateSample> {
    samples
        .windows(2)
        .filter_map(|w| {
            let (a, b) = (&w[0], &w[1]);
            let secs = (b.0 - a.0) / 1000.0;
            (secs > 0.0).then(|| RateSample {
                t_ms: b.0.round().max(0.0) as u32,
                down_bps: b.1.saturating_sub(a.1) as f64 * 8.0 / secs,
                up_bps: b.2.saturating_sub(a.2) as f64 * 8.0 / secs,
            })
        })
        .collect()
}

/// Samples the members during the plan, then computes and delivers the result.
async fn run_group(
    groups: Groups,
    id: Uuid,
    start_at: Instant,
    plan: Profile,
    members: Vec<(Uuid, Option<Arc<Session>>)>,
    storage: Option<Storage>,
    link_bps: Option<f64>,
) {
    tokio::time::sleep_until(start_at).await;
    if !groups.transition(id, GroupState::Running) {
        return;
    }
    let total_ms = plan_duration_ms(&plan);
    let end = start_at + Duration::from_millis(total_ms);
    let mut samples: Vec<Samples> = vec![Vec::new(); members.len()];
    let mut dropped = vec![false; members.len()];
    let mut next = start_at;
    loop {
        tokio::time::sleep_until(next).await;
        let now = Instant::now();
        let t_ms = now
            .min(end)
            .saturating_duration_since(start_at)
            .as_secs_f64()
            * 1000.0;
        let left = groups.left_members(id);
        for (i, (session, s)) in members.iter().enumerate() {
            match s {
                Some(s) => {
                    samples[i].push((t_ms, s.bytes_sent(), s.bytes_received()));
                    if s.attached_sockets() == 0 {
                        dropped[i] = true;
                    }
                }
                None => dropped[i] = true,
            }
            if left.contains(session) {
                dropped[i] = true;
            }
        }
        if now >= end {
            break;
        }
        // The last sample lands on the end of the plan.
        next = (next + SAMPLE_INTERVAL).min(end);
    }

    // Give the members a moment to post their runs (loaded latency).
    let grace_end = Instant::now() + RESULT_GRACE;
    loop {
        let reports = groups.reports(id);
        let all = members
            .iter()
            .zip(&dropped)
            .all(|((m, _), d)| *d || reports.contains_key(m));
        if all || Instant::now() >= grace_end {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let reports = groups.reports(id);
    let left = groups.left_members(id);

    let (down_w, up_w) = transfer_windows(&plan);
    let inputs = members
        .iter()
        .enumerate()
        .map(|(i, (session, _))| {
            let snap = groups.inner.registry.snapshot_of(*session);
            let device = snap.as_ref().and_then(|s| s.device.clone());
            let report = reports.get(session).copied().unwrap_or_default();
            let s = &samples[i];
            GroupMemberInput {
                session: *session,
                device_name: device
                    .as_ref()
                    .map_or_else(|| UNKNOWN_DEVICE.to_owned(), |d| d.device_name.clone()),
                device_kind: device.map(|d| d.device_kind).unwrap_or_default(),
                label: snap.and_then(|s| s.label),
                download_bps: down_w.as_ref().and_then(|w| window_bps(s, w, |x| x.1)),
                upload_bps: up_w.as_ref().and_then(|w| window_bps(s, w, |x| x.2)),
                down_loaded_latency_ms: report.down_loaded_latency_ms,
                up_loaded_latency_ms: report.up_loaded_latency_ms,
                incomplete: dropped[i] || left.contains(session),
                series: series(s),
            }
        })
        .collect();
    let result = GroupResult::compute(id, rfc3339_now(), total_ms, inputs, link_bps);
    if !groups.transition(id, GroupState::Finished) {
        return;
    }
    tracing::info!(
        group = %id,
        aggregate_download_mbps = result.aggregate_download_bps.map(|b| b / 1e6),
        aggregate_upload_mbps = result.aggregate_upload_bps.map(|b| b / 1e6),
        "group test finished"
    );
    let msg = ServerMsg::GroupResult { group: id, result };
    for (m, _) in &members {
        groups.inner.registry.send_to(*m, &msg);
    }
    if let (Some(storage), ServerMsg::GroupResult { result, .. }) = (storage, msg)
        && let Err(e) = storage.insert_group(&result).await
    {
        tracing::warn!(group = %id, "could not store the group result: {e}");
    }
}

/// The current UTC time as RFC 3339 with milliseconds, e.g. `2026-10-10T08:00:00.123Z`.
pub fn rfc3339_now() -> String {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format_rfc3339(d.as_secs(), d.subsec_millis())
}

fn format_rfc3339(secs: u64, millis: u32) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

// ---------------------------------------------------------------------------------------
// Server link speed (bottleneck warning)

/// The server's link speed in bps: `--server-link-mbps` if set; otherwise, on Linux, the
/// speed of the interface the server uses to reach `client_ip` (from
/// `/sys/class/net/<iface>/speed`), falling back to the fastest interface that is up and
/// not loopback. `None` when unknown.
pub fn server_link_bps(configured_mbps: Option<u32>, client_ip: Option<IpAddr>) -> Option<f64> {
    if let Some(mbps) = configured_mbps.filter(|m| *m > 0) {
        return Some(f64::from(mbps) * 1e6);
    }
    detect_link_mbps(client_ip).map(|mbps| mbps as f64 * 1e6)
}

#[cfg(target_os = "linux")]
fn detect_link_mbps(client_ip: Option<IpAddr>) -> Option<u64> {
    let ifaces = if_addrs::get_if_addrs().ok()?;
    if let Some(local) = client_ip.and_then(local_ip_towards)
        && let Some(iface) = ifaces.iter().find(|i| i.ip() == local)
        && let Some(speed) = iface_speed_mbps(&iface.name)
    {
        return Some(speed);
    }
    ifaces
        .iter()
        .filter(|i| !i.is_loopback() && iface_is_up(&i.name))
        .filter_map(|i| iface_speed_mbps(&i.name))
        .max()
}

#[cfg(not(target_os = "linux"))]
fn detect_link_mbps(_client_ip: Option<IpAddr>) -> Option<u64> {
    None
}

/// The local address the kernel would use to reach `ip` (no packet is sent).
#[cfg(target_os = "linux")]
fn local_ip_towards(ip: IpAddr) -> Option<IpAddr> {
    use std::net::{Ipv4Addr, Ipv6Addr, UdpSocket};
    let ip = ip.to_canonical();
    let bind: IpAddr = if ip.is_ipv4() {
        Ipv4Addr::UNSPECIFIED.into()
    } else {
        Ipv6Addr::UNSPECIFIED.into()
    };
    let socket = UdpSocket::bind((bind, 0)).ok()?;
    socket.connect((ip, 9)).ok()?;
    Some(socket.local_addr().ok()?.ip())
}

#[cfg(target_os = "linux")]
fn iface_speed_mbps(name: &str) -> Option<u64> {
    if name.contains('/') || name.contains("..") {
        return None;
    }
    let s = std::fs::read_to_string(format!("/sys/class/net/{name}/speed")).ok()?;
    s.trim()
        .parse::<i64>()
        .ok()
        .filter(|v| *v > 0)
        .map(|v| v as u64)
}

#[cfg(target_os = "linux")]
fn iface_is_up(name: &str) -> bool {
    std::fs::read_to_string(format!("/sys/class/net/{name}/operstate"))
        .is_ok_and(|s| matches!(s.trim(), "up" | "unknown"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_of_the_concurrent_plan() {
        let (down, up) = transfer_windows(&Profile::concurrent());
        assert_eq!(down, Some(3000..13_000));
        assert_eq!(up, Some(15_000..25_000));
        assert_eq!(plan_duration_ms(&Profile::concurrent()), 25_000);
        let (d, u) = transfer_windows(&Profile::standard());
        assert!(d.is_none() && u.is_none());
    }

    #[test]
    fn window_rate_interpolates_and_skips_the_ramp() {
        // 1 MB/s down, starting at t = 0, sampled every 250 ms for 4 s.
        let s: Samples = (0..=16)
            .map(|i| {
                let t = f64::from(i) * 250.0;
                (t, (t * 1000.0) as u64, 0)
            })
            .collect();
        let bps = window_bps(&s, &(500..3700), |x| x.1).unwrap();
        assert!((bps - 8e6).abs() < 1.0, "{bps}");
        // Not enough samples to cover the window.
        assert!(window_bps(&s, &(0..5000), |x| x.1).is_none());
        let ser = series(&s);
        assert_eq!(ser.len(), 16);
        assert!((ser[0].down_bps - 8e6).abs() < 1.0);
        assert_eq!(ser[0].t_ms, 250);
    }

    #[test]
    fn rfc3339_formatting() {
        assert_eq!(format_rfc3339(0, 0), "1970-01-01T00:00:00.000Z");
        // 2026-10-10T08:00:00Z
        assert_eq!(format_rfc3339(1_791_619_200, 5), "2026-10-10T08:00:00.005Z");
        assert_eq!(format_rfc3339(951_782_400, 999), "2000-02-29T00:00:00.999Z");
    }

    #[test]
    fn configured_link_speed_wins() {
        assert_eq!(server_link_bps(Some(2500), None), Some(2.5e9));
    }
}
