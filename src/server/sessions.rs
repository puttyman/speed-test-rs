//! Session registry (SPEC §5.11, §6.1, §11).
//!
//! A *session* is one browser test session, identified by the UUID the client sends in
//! `Hello`. It groups the sockets the browser opens (`/ws/control`, one or more `/ws/data`,
//! `/ws/probe`) and the HTTP-mode requests that name it (`?session=`). For each session the
//! registry keeps:
//!
//! - the attached sockets by role, and a sender for every control socket so the server can
//!   push messages at any time (presence, group tests);
//! - the [`DeviceInfo`] from the control socket's `Hello`, plus the presence status
//!   (phase, label, hidden);
//! - atomic byte counters (bytes sent to and received from the client) and the live rates
//!   computed from them every [`SAMPLE_INTERVAL`] by a small sampler task;
//! - the number of in-flight transfers and a log of the time spans during which the session
//!   moved data, used to compute [`Concurrency`] (overlap with other sessions) for a run.
//!
//! When the last socket closes and no transfer is running, the session becomes a
//! *tombstone*: it no longer counts as connected, but its activity log stays queryable for
//! [`TOMBSTONE_TTL`], so a run saved right after the test can still get its overlap.
//!
//! `max_sessions` counts the sessions that have a data socket attached or a transfer in
//! flight. Presence-only sessions (just a control socket, and probe sockets) don't count.
//!
//! Locking: a `std::sync::Mutex` guards the session map and another one each session's
//! mutable state. Neither is ever held across an `.await`, and the map lock is always taken
//! before a session lock.

use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};
use uuid::Uuid;

use crate::model::protocol::{DeviceInfo, ErrorCode, Phase, Role, ServerMsg};
use crate::model::result::Concurrency;

/// How often the rate sampler runs (and tombstones are reaped).
pub const SAMPLE_INTERVAL: Duration = Duration::from_millis(250);
/// How long a closed session's activity log stays queryable.
pub const TOMBSTONE_TTL: Duration = Duration::from_secs(5 * 60);
/// Activity spans separated by less than this are merged into one (back-to-back requests).
const ACTIVITY_MERGE_GAP: Duration = Duration::from_millis(50);
/// Upper bound on the activity spans kept per session (oldest are dropped).
const ACTIVITY_LOG_MAX: usize = 4096;
/// Capacity of each control socket's outbound queue.
pub const CONTROL_QUEUE: usize = 64;

/// Sends a message to one control socket (its writer task serializes it).
pub type ControlSender = mpsc::Sender<ServerMsg>;

/// Why a socket or transfer was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub code: ErrorCode,
    pub message: String,
}

impl Rejection {
    fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// The `Error` frame for this rejection.
    pub fn to_msg(&self) -> ServerMsg {
        ServerMsg::error(self.code, self.message.clone())
    }
}

/// Live rates of one session, in bits per second, from the server's byte counters.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rates {
    /// Server → client.
    pub down_bps: f64,
    /// Client → server.
    pub up_bps: f64,
}

/// A point-in-time copy of one session, from [`SessionRegistry::snapshot`].
#[derive(Debug, Clone)]
pub struct SessionSnapshot {
    pub id: Uuid,
    /// From the control socket's `Hello`.
    pub device: Option<DeviceInfo>,
    /// Last `Status` phase (`None` = idle).
    pub phase: Option<Phase>,
    pub label: Option<String>,
    pub hidden: bool,
    pub control_sockets: usize,
    pub data_sockets: usize,
    pub probe_sockets: usize,
    /// Transfers in flight (WebSocket and HTTP).
    pub active_transfers: u32,
    /// Total payload bytes sent to the client.
    pub bytes_sent: u64,
    /// Total payload bytes received from the client.
    pub bytes_received: u64,
    /// Rates over the last sampler tick.
    pub rates: Rates,
    /// Time since the last message, frame or transfer.
    pub idle_for: Duration,
    /// True for tombstones (all sockets closed, kept for overlap queries).
    pub closed: bool,
}

impl SessionSnapshot {
    /// Counts toward `max_sessions`.
    pub fn counts(&self) -> bool {
        !self.closed && (self.data_sockets > 0 || self.active_transfers > 0)
    }
}

/// A span of time during which a session had at least one transfer in flight.
#[derive(Debug, Clone, Copy)]
struct Span {
    start: Instant,
    /// `None` while still running.
    end: Option<Instant>,
}

struct SessionState {
    device: Option<DeviceInfo>,
    phase: Option<Phase>,
    label: Option<String>,
    hidden: bool,
    /// Attached sockets by connection ID.
    sockets: HashMap<u64, Role>,
    /// Outbound queues of the attached control sockets.
    control: HashMap<u64, ControlSender>,
    active_transfers: u32,
    last_activity: Instant,
    activity: VecDeque<Span>,
    rates: Rates,
    /// Counter values at the previous sampler tick.
    last_sample: (Instant, u64, u64),
    /// Set when the session became a tombstone.
    closed_at: Option<Instant>,
}

impl SessionState {
    fn new(now: Instant) -> Self {
        Self {
            device: None,
            phase: None,
            label: None,
            hidden: false,
            sockets: HashMap::new(),
            control: HashMap::new(),
            active_transfers: 0,
            last_activity: now,
            activity: VecDeque::new(),
            rates: Rates::default(),
            last_sample: (now, 0, 0),
            closed_at: None,
        }
    }

    fn sockets_with(&self, role: Role) -> usize {
        self.sockets.values().filter(|r| **r == role).count()
    }

    fn counts(&self) -> bool {
        self.active_transfers > 0 || self.sockets.values().any(|r| *r == Role::Data)
    }

    fn start_activity(&mut self, now: Instant) {
        match self.activity.back_mut() {
            Some(span) if span.end.is_some_and(|end| now - end < ACTIVITY_MERGE_GAP) => {
                span.end = None;
            }
            _ => {
                if self.activity.len() >= ACTIVITY_LOG_MAX {
                    self.activity.pop_front();
                }
                self.activity.push_back(Span {
                    start: now,
                    end: None,
                });
            }
        }
    }

    fn end_activity(&mut self, now: Instant) {
        if let Some(span) = self.activity.back_mut()
            && span.end.is_none()
        {
            span.end = Some(now);
        }
    }

    /// Marks the session closed if nothing is attached or running any more.
    fn maybe_close(&mut self, now: Instant) -> bool {
        if self.sockets.is_empty() && self.active_transfers == 0 && self.closed_at.is_none() {
            self.closed_at = Some(now);
            true
        } else {
            false
        }
    }
}

/// One test session. Shared by its sockets and transfers through an `Arc`.
pub struct Session {
    id: Uuid,
    bytes_sent: AtomicU64,
    bytes_received: AtomicU64,
    /// Bumped by [`Session::abort`]; data sockets watch it to stop their transfer.
    abort: watch::Sender<u64>,
    state: Mutex<SessionState>,
}

impl Session {
    fn new(id: Uuid, now: Instant) -> Self {
        Self {
            id,
            bytes_sent: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),
            abort: watch::channel(0).0,
            state: Mutex::new(SessionState::new(now)),
        }
    }

    pub fn id(&self) -> Uuid {
        self.id
    }

    /// Counts `n` payload bytes sent to the client.
    pub fn add_sent(&self, n: u64) {
        self.bytes_sent.fetch_add(n, Ordering::Relaxed);
    }

    /// Counts `n` payload bytes received from the client.
    pub fn add_received(&self, n: u64) {
        self.bytes_received.fetch_add(n, Ordering::Relaxed);
    }

    pub fn bytes_sent(&self) -> u64 {
        self.bytes_sent.load(Ordering::Relaxed)
    }

    pub fn bytes_received(&self) -> u64 {
        self.bytes_received.load(Ordering::Relaxed)
    }

    /// Asks every data socket of this session to stop its current transfer (`Abort` on
    /// the control socket).
    pub fn abort(&self) {
        self.abort.send_modify(|v| *v = v.wrapping_add(1));
    }

    /// Changes whenever [`abort`](Self::abort) is called.
    pub fn abort_signal(&self) -> watch::Receiver<u64> {
        self.abort.subscribe()
    }

    /// Records activity (a message or frame) now.
    pub fn touch(&self) {
        lock(&self.state).last_activity = Instant::now();
    }

    fn snapshot(&self, now: Instant) -> SessionSnapshot {
        let st = lock(&self.state);
        SessionSnapshot {
            id: self.id,
            device: st.device.clone(),
            phase: st.phase,
            label: st.label.clone(),
            hidden: st.hidden,
            control_sockets: st.sockets_with(Role::Control),
            data_sockets: st.sockets_with(Role::Data),
            probe_sockets: st.sockets_with(Role::Probe),
            active_transfers: st.active_transfers,
            bytes_sent: self.bytes_sent(),
            bytes_received: self.bytes_received(),
            rates: st.rates,
            idle_for: now.saturating_duration_since(st.last_activity),
            closed: st.closed_at.is_some(),
        }
    }
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session").field("id", &self.id).finish()
    }
}

struct Inner {
    sessions: Mutex<HashMap<Uuid, Arc<Session>>>,
    max_sessions: usize,
    max_streams: usize,
    tombstone_ttl: Duration,
    next_conn: AtomicU64,
    /// Bumped whenever presence-relevant state changes (sockets, status, device).
    changes: watch::Sender<u64>,
}

impl Inner {
    fn changed(&self) {
        self.changes.send_modify(|v| *v = v.wrapping_add(1));
    }
}

/// The registry of all sessions. Cheap to clone.
#[derive(Clone)]
pub struct SessionRegistry {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for SessionRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionRegistry")
            .field("max_sessions", &self.inner.max_sessions)
            .finish_non_exhaustive()
    }
}

impl SessionRegistry {
    /// A registry allowing `max_sessions` counted sessions and `max_streams` data sockets
    /// per session. Call [`start_sampler`](Self::start_sampler) to get live rates.
    pub fn new(max_sessions: usize, max_streams: usize) -> Self {
        Self::with_tombstone_ttl(max_sessions, max_streams, TOMBSTONE_TTL)
    }

    /// Like [`new`](Self::new) with a custom tombstone lifetime.
    pub fn with_tombstone_ttl(
        max_sessions: usize,
        max_streams: usize,
        tombstone_ttl: Duration,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                sessions: Mutex::new(HashMap::new()),
                max_sessions,
                max_streams,
                tombstone_ttl,
                next_conn: AtomicU64::new(1),
                changes: watch::channel(0).0,
            }),
        }
    }

    /// Spawns the rate sampler on the current Tokio runtime: every [`SAMPLE_INTERVAL`] it
    /// updates each session's [`Rates`] and drops expired tombstones. The task ends when the
    /// registry is dropped. Returns `None` (and does nothing) outside a runtime.
    pub fn start_sampler(&self) -> Option<JoinHandle<()>> {
        let handle = tokio::runtime::Handle::try_current().ok()?;
        let weak: Weak<Inner> = Arc::downgrade(&self.inner);
        Some(handle.spawn(async move {
            let mut tick = tokio::time::interval(SAMPLE_INTERVAL);
            tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                let Some(inner) = weak.upgrade() else { break };
                let registry = SessionRegistry { inner };
                registry.sample_now();
                registry.reap();
            }
        }))
    }

    /// Recomputes every session's rates from its counters (normally done by the sampler).
    pub fn sample_now(&self) {
        let now = Instant::now();
        for session in self.sessions() {
            let sent = session.bytes_sent();
            let received = session.bytes_received();
            let mut st = lock(&session.state);
            let (at, prev_sent, prev_received) = st.last_sample;
            let secs = now.saturating_duration_since(at).as_secs_f64();
            if secs <= 0.0 {
                continue;
            }
            st.rates = Rates {
                down_bps: sent.saturating_sub(prev_sent) as f64 * 8.0 / secs,
                up_bps: received.saturating_sub(prev_received) as f64 * 8.0 / secs,
            };
            st.last_sample = (now, sent, received);
        }
    }

    /// Drops tombstones older than the tombstone lifetime.
    pub fn reap(&self) {
        let now = Instant::now();
        let ttl = self.inner.tombstone_ttl;
        lock(&self.inner.sessions).retain(|_, s| {
            lock(&s.state)
                .closed_at
                .is_none_or(|closed| now.saturating_duration_since(closed) < ttl)
        });
    }

    fn sessions(&self) -> Vec<Arc<Session>> {
        lock(&self.inner.sessions).values().cloned().collect()
    }

    /// A receiver that changes whenever sockets attach or detach or a status changes.
    /// The presence broadcaster can use it to push `Peers` frames only when needed.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.inner.changes.subscribe()
    }

    /// The session, if it is known (live or tombstone).
    pub fn get(&self, session: Uuid) -> Option<Arc<Session>> {
        lock(&self.inner.sessions).get(&session).cloned()
    }

    /// Attaches a socket of `role` to `session`, creating the session if needed.
    ///
    /// `device` is stored when given (control socket); `control` is the outbound queue of a
    /// control socket. A data socket is refused with `Busy` when `max_sessions` sessions
    /// already count and this one doesn't, and with `LimitExceeded` beyond `max_streams`
    /// data sockets. The returned guard detaches the socket on drop.
    pub fn attach(
        &self,
        session: Uuid,
        role: Role,
        device: Option<DeviceInfo>,
        control: Option<ControlSender>,
    ) -> Result<SocketGuard, Rejection> {
        let now = Instant::now();
        let conn = self.inner.next_conn.fetch_add(1, Ordering::Relaxed);
        let mut map = lock(&self.inner.sessions);
        if role == Role::Data {
            let already_counts = map.get(&session).is_some_and(|s| {
                let st = lock(&s.state);
                st.closed_at.is_none() && st.counts()
            });
            if !already_counts {
                let counted = count_sessions(&map);
                if counted >= self.inner.max_sessions {
                    return Err(Rejection::new(
                        ErrorCode::Busy,
                        format!(
                            "Server busy: {counted} tests are already running (max {})",
                            self.inner.max_sessions
                        ),
                    ));
                }
            }
        }
        let entry = map
            .entry(session)
            .or_insert_with(|| Arc::new(Session::new(session, now)))
            .clone();
        // Still under the map lock, so a concurrent `attach` sees this socket when counting.
        {
            let mut st = lock(&entry.state);
            if role == Role::Data && st.sockets_with(Role::Data) >= self.inner.max_streams {
                return Err(Rejection::new(
                    ErrorCode::LimitExceeded,
                    format!(
                        "at most {} data sockets per session",
                        self.inner.max_streams
                    ),
                ));
            }
            st.sockets.insert(conn, role);
            if let Some(tx) = control {
                st.control.insert(conn, tx);
            }
            if device.is_some() {
                st.device = device;
            }
            st.closed_at = None;
            st.last_activity = now;
        }
        drop(map);
        self.inner.changed();
        tracing::debug!(%session, ?role, conn, "socket attached");
        Ok(SocketGuard {
            inner: self.inner.clone(),
            session: entry,
            conn,
            role,
        })
    }

    /// Starts a transfer on `session` that isn't tied to a data socket (HTTP mode),
    /// creating the session if needed. Refused with `Busy` like [`attach`](Self::attach).
    pub fn begin_transfer(&self, session: Uuid) -> Result<TransferGuard, Rejection> {
        let now = Instant::now();
        let mut map = lock(&self.inner.sessions);
        let already_counts = map.get(&session).is_some_and(|s| {
            let st = lock(&s.state);
            st.closed_at.is_none() && st.counts()
        });
        if !already_counts {
            let counted = count_sessions(&map);
            if counted >= self.inner.max_sessions {
                return Err(Rejection::new(
                    ErrorCode::Busy,
                    format!("Server busy: {counted} tests are already running"),
                ));
            }
        }
        let entry = map
            .entry(session)
            .or_insert_with(|| Arc::new(Session::new(session, now)))
            .clone();
        // Started under the map lock, so a concurrent check sees it.
        Ok(TransferGuard::start(self.inner.clone(), entry, now))
    }

    /// All sessions (live and tombstones) with their current rates.
    pub fn snapshot(&self) -> Vec<SessionSnapshot> {
        let now = Instant::now();
        self.sessions().iter().map(|s| s.snapshot(now)).collect()
    }

    /// Number of live sessions that count toward `max_sessions`.
    pub fn counted_sessions(&self) -> usize {
        count_sessions(&lock(&self.inner.sessions))
    }

    /// Number of sessions with at least one transfer in flight right now.
    pub fn active_transfer_count(&self) -> usize {
        self.sessions()
            .iter()
            .filter(|s| lock(&s.state).active_transfers > 0)
            .count()
    }

    /// Overlap of other sessions' data transfers with `session` during `window`
    /// (SPEC §5.11 "overlap flagging").
    ///
    /// With `window = None` the window is the session's own activity: from the start of
    /// its first transfer to the end of its last one (or now, if one is running). Returns
    /// `None` for an unknown session (never seen, or a tombstone already reaped).
    pub fn concurrency_for(
        &self,
        session: Uuid,
        window: Option<Range<Instant>>,
    ) -> Option<Concurrency> {
        let now = Instant::now();
        let map = lock(&self.inner.sessions);
        let me = map.get(&session)?;
        let window = match window {
            Some(w) => w,
            None => {
                let st = lock(&me.state);
                match (st.activity.front(), st.activity.back()) {
                    (Some(first), Some(last)) => first.start..last.end.unwrap_or(now),
                    _ => return Some(Concurrency::default()),
                }
            }
        };
        if window.end <= window.start {
            return Some(Concurrency::default());
        }
        let mut events = Vec::new();
        for (id, other) in map.iter() {
            if *id == session {
                continue;
            }
            let st = lock(&other.state);
            for span in &st.activity {
                let start = span.start.max(window.start);
                let end = span.end.unwrap_or(now).min(window.end);
                if start < end {
                    events.push((start, 1));
                    events.push((end, -1));
                }
            }
        }
        drop(map);
        Some(sweep_overlap(events, window))
    }

    /// Stores a control socket's `Status`.
    pub fn set_status(&self, session: Uuid, phase: Option<Phase>, label: Option<String>) {
        if let Some(s) = self.get(session) {
            let mut st = lock(&s.state);
            st.phase = phase.filter(|p| *p != Phase::Idle);
            st.label = label;
            drop(st);
            self.inner.changed();
        }
    }

    /// Stores the "Hide me" flag.
    pub fn set_hidden(&self, session: Uuid, hidden: bool) {
        if let Some(s) = self.get(session) {
            lock(&s.state).hidden = hidden;
            self.inner.changed();
        }
    }

    /// The outbound queues of every attached control socket, with their session.
    pub fn control_senders(&self) -> Vec<(Uuid, ControlSender)> {
        let mut out = Vec::new();
        for s in self.sessions() {
            let st = lock(&s.state);
            out.extend(st.control.values().map(|tx| (s.id, tx.clone())));
        }
        out
    }

    /// Queues `msg` on every control socket of `session` without waiting. Returns how many
    /// sockets accepted it (a full or closed queue drops the message).
    pub fn send_to(&self, session: Uuid, msg: &ServerMsg) -> usize {
        let Some(s) = self.get(session) else { return 0 };
        let senders: Vec<ControlSender> = lock(&s.state).control.values().cloned().collect();
        senders
            .iter()
            .filter(|tx| tx.try_send(msg.clone()).is_ok())
            .count()
    }
}

fn count_sessions(map: &HashMap<Uuid, Arc<Session>>) -> usize {
    map.values()
        .filter(|s| {
            let st = lock(&s.state);
            st.closed_at.is_none() && st.counts()
        })
        .count()
}

/// Sweeps `(time, +1/-1)` events of other sessions' activity clipped to `window`. Each
/// session's spans are disjoint, so the running count is the number of other sessions
/// moving data.
fn sweep_overlap(mut events: Vec<(Instant, i32)>, window: Range<Instant>) -> Concurrency {
    // Ends before starts at the same instant, so touching spans don't count as overlap.
    events.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut active = 0i32;
    let mut peers_max = 0i32;
    let mut overlap = Duration::ZERO;
    let mut since = window.start;
    for (t, delta) in events {
        if active > 0 {
            overlap += t.saturating_duration_since(since);
        }
        active += delta;
        peers_max = peers_max.max(active);
        since = t;
    }
    let total_ms = (window.end - window.start).as_secs_f64() * 1000.0;
    let overlap_ms = overlap.as_secs_f64() * 1000.0;
    Concurrency {
        peers_max: peers_max.max(0) as u32,
        overlap_ms,
        overlap_ratio: if total_ms > 0.0 {
            (overlap_ms / total_ms).clamp(0.0, 1.0)
        } else {
            0.0
        },
    }
}

/// Keeps a socket attached to its session; detaches it on drop.
pub struct SocketGuard {
    inner: Arc<Inner>,
    session: Arc<Session>,
    conn: u64,
    role: Role,
}

impl std::fmt::Debug for SocketGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SocketGuard")
            .field("session", &self.session.id)
            .field("conn", &self.conn)
            .field("role", &self.role)
            .finish()
    }
}

impl SocketGuard {
    pub fn session(&self) -> &Arc<Session> {
        &self.session
    }

    pub fn role(&self) -> Role {
        self.role
    }

    /// Starts a transfer on this socket's session. Never refused: a data socket already
    /// counts toward `max_sessions`.
    pub fn begin_transfer(&self) -> TransferGuard {
        TransferGuard::start(self.inner.clone(), self.session.clone(), Instant::now())
    }
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let now = Instant::now();
        {
            let mut st = lock(&self.session.state);
            st.sockets.remove(&self.conn);
            st.control.remove(&self.conn);
            st.maybe_close(now);
        }
        self.inner.changed();
        tracing::debug!(session = %self.session.id, role = ?self.role, conn = self.conn, "socket detached");
    }
}

/// An in-flight transfer: marks the session as moving data until dropped.
pub struct TransferGuard {
    inner: Arc<Inner>,
    session: Arc<Session>,
}

impl std::fmt::Debug for TransferGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransferGuard")
            .field("session", &self.session.id)
            .finish()
    }
}

impl TransferGuard {
    fn start(inner: Arc<Inner>, session: Arc<Session>, now: Instant) -> Self {
        {
            let mut st = lock(&session.state);
            if st.active_transfers == 0 {
                st.start_activity(now);
            }
            st.active_transfers += 1;
            st.closed_at = None;
            st.last_activity = now;
        }
        Self { inner, session }
    }

    pub fn session(&self) -> &Arc<Session> {
        &self.session
    }
}

impl Drop for TransferGuard {
    fn drop(&mut self) {
        let now = Instant::now();
        let closed = {
            let mut st = lock(&self.session.state);
            st.active_transfers = st.active_transfers.saturating_sub(1);
            if st.active_transfers == 0 {
                st.end_activity(now);
            }
            st.last_activity = now;
            st.maybe_close(now)
        };
        if closed {
            self.inner.changed();
        }
    }
}

/// Locks a mutex, recovering from poisoning (the guarded data stays consistent because
/// no code path panics while holding these locks).
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[test]
    fn sweep_counts_peers_and_overlap() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        // Window 0..1000. Peer A active 100..400, peer B 300..600.
        let events = vec![(ms(100), 1), (ms(400), -1), (ms(300), 1), (ms(600), -1)];
        let c = sweep_overlap(events, ms(0)..ms(1000));
        assert_eq!(c.peers_max, 2);
        assert!((c.overlap_ms - 500.0).abs() < 1e-6, "{c:?}");
        assert!((c.overlap_ratio - 0.5).abs() < 1e-9);

        let c = sweep_overlap(vec![], ms(0)..ms(1000));
        assert_eq!(c, Concurrency::default());

        // Touching spans of the same peer don't double-count.
        let c = sweep_overlap(
            vec![(ms(0), 1), (ms(100), -1), (ms(100), 1), (ms(200), -1)],
            ms(0)..ms(400),
        );
        assert_eq!(c.peers_max, 1);
        assert!((c.overlap_ms - 200.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn busy_counts_only_data_sessions() {
        let reg = SessionRegistry::new(1, 8);
        // Presence-only sockets never count.
        let _c1 = reg.attach(id(1), Role::Control, None, None).unwrap();
        let _c2 = reg.attach(id(2), Role::Control, None, None).unwrap();
        let _p2 = reg.attach(id(2), Role::Probe, None, None).unwrap();
        assert_eq!(reg.counted_sessions(), 0);

        let d1 = reg.attach(id(1), Role::Data, None, None).unwrap();
        // The same session may open more streams.
        let d1b = reg.attach(id(1), Role::Data, None, None).unwrap();
        assert_eq!(reg.counted_sessions(), 1);
        let err = reg.attach(id(2), Role::Data, None, None).unwrap_err();
        assert_eq!(err.code, ErrorCode::Busy);
        assert_eq!(
            reg.begin_transfer(id(3)).err().map(|e| e.code),
            Some(ErrorCode::Busy)
        );

        drop((d1, d1b));
        assert_eq!(reg.counted_sessions(), 0);
        assert!(reg.attach(id(2), Role::Data, None, None).is_ok());
    }

    #[tokio::test]
    async fn max_streams_per_session() {
        let reg = SessionRegistry::new(16, 2);
        let _a = reg.attach(id(1), Role::Data, None, None).unwrap();
        let _b = reg.attach(id(1), Role::Data, None, None).unwrap();
        let err = reg.attach(id(1), Role::Data, None, None).unwrap_err();
        assert_eq!(err.code, ErrorCode::LimitExceeded);
    }

    #[tokio::test]
    async fn tombstones_and_reaping() {
        let reg = SessionRegistry::with_tombstone_ttl(16, 8, Duration::from_millis(0));
        let guard = reg.attach(id(1), Role::Control, None, None).unwrap();
        assert!(!reg.snapshot()[0].closed);
        drop(guard);
        assert!(reg.snapshot()[0].closed);
        reg.reap();
        assert!(reg.snapshot().is_empty());

        // HTTP-only transfer: the session lives while the transfer runs.
        let t = reg.begin_transfer(id(2)).unwrap();
        assert_eq!(reg.active_transfer_count(), 1);
        assert!(reg.snapshot()[0].counts());
        drop(t);
        assert!(reg.snapshot()[0].closed);
    }

    #[tokio::test]
    async fn rates_and_overlap() {
        let reg = SessionRegistry::new(16, 8);
        let a = reg.attach(id(1), Role::Data, None, None).unwrap();
        let b = reg.attach(id(2), Role::Data, None, None).unwrap();
        reg.sample_now();

        let ta = a.begin_transfer();
        tokio::time::sleep(Duration::from_millis(60)).await;
        let tb = b.begin_transfer();
        b.session().add_received(1_000_000);
        tokio::time::sleep(Duration::from_millis(60)).await;
        drop(tb);
        tokio::time::sleep(Duration::from_millis(60)).await;
        a.session().add_sent(2_000_000);
        drop(ta);
        reg.sample_now();

        let snap = reg.snapshot();
        let sa = snap.iter().find(|s| s.id == id(1)).unwrap();
        let sb = snap.iter().find(|s| s.id == id(2)).unwrap();
        assert_eq!(sa.bytes_sent, 2_000_000);
        assert_eq!(sb.bytes_received, 1_000_000);
        assert!(sa.rates.down_bps > 0.0 && sa.rates.up_bps == 0.0);
        assert!(sb.rates.up_bps > 0.0);

        let c = reg.concurrency_for(id(1), None).unwrap();
        assert_eq!(c.peers_max, 1);
        assert!(c.overlap_ms >= 50.0 && c.overlap_ms < 150.0, "{c:?}");
        assert!(c.overlap_ratio > 0.2 && c.overlap_ratio < 0.7, "{c:?}");
        // B was entirely inside A's run.
        let c = reg.concurrency_for(id(2), None).unwrap();
        assert!((c.overlap_ratio - 1.0).abs() < 1e-9, "{c:?}");
        assert!(reg.concurrency_for(id(9), None).is_none());
    }

    #[tokio::test]
    async fn control_senders_and_send_to() {
        let reg = SessionRegistry::new(16, 8);
        let (tx, mut rx) = mpsc::channel(4);
        let mut changes = reg.subscribe();
        let g = reg.attach(id(1), Role::Control, None, Some(tx)).unwrap();
        assert!(changes.has_changed().unwrap());
        changes.mark_unchanged();
        assert_eq!(reg.control_senders().len(), 1);
        assert_eq!(reg.send_to(id(1), &ServerMsg::Pong { seq: 1 }), 1);
        assert_eq!(rx.recv().await, Some(ServerMsg::Pong { seq: 1 }));
        reg.set_status(id(1), Some(Phase::Download), Some("Desk".into()));
        assert!(changes.has_changed().unwrap());
        assert_eq!(reg.snapshot()[0].phase, Some(Phase::Download));
        drop(g);
        assert!(reg.control_senders().is_empty());
    }
}
