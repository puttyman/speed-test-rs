//! The page-wide presence context (SPEC §5.11, §7.11).
//!
//! `App` calls [`provide_presence`] once: after hydration it opens a single
//! presence connection ([`crate::engine::presence::connect_presence`]) for
//! the whole page load and keeps it across client-side navigation. The test
//! page, the Devices panel, the invite toast and the group test all read the
//! same [`PresenceCtx`], so there is exactly one session per page: solo runs
//! and group runs reuse it (`PresenceHandle::engine_options`) and are posted
//! with `?session=` (overlap flags, group results).
//!
//! The group test itself (countdown, the synchronized run, posting the
//! member's run, the server's result) is driven from here too, so it keeps
//! going whichever page is open.

use std::collections::HashMap;

use leptos::prelude::*;
use uuid::Uuid;

use crate::components::charts::Series;
use crate::model::protocol::{GroupState, Peer, PeerState, Phase};
use crate::model::result::GroupResult;

/// Samples kept per device for the Devices panel sparklines.
pub const SPARK_POINTS: usize = 40;

// ---------------------------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------------------------

/// The countdown digit shown `remaining_ms` before the start: 3, 2, 1, then
/// `None` once the test starts.
pub fn countdown_digit(remaining_ms: f64) -> Option<u32> {
    if !(remaining_ms.is_finite() && remaining_ms > 0.0) {
        return None;
    }
    Some((remaining_ms / 1000.0).ceil().max(1.0) as u32)
}

/// The rate that matters for a peer's state: its download rate while
/// downloading, upload while uploading, otherwise the larger of the two.
pub fn peer_rate(state: &PeerState) -> Option<f64> {
    match *state {
        PeerState::Idle => None,
        PeerState::Testing {
            phase,
            down_bps,
            up_bps,
            ..
        } => Some(match phase {
            Phase::Download => down_bps,
            Phase::Upload => up_bps,
            _ => down_bps.max(up_bps),
        }),
    }
}

/// The phase a peer is testing, if any.
pub fn peer_phase(state: &PeerState) -> Option<Phase> {
    match *state {
        PeerState::Idle => None,
        PeerState::Testing { phase, .. } => Some(phase),
    }
}

/// Other (not this) devices that are testing right now.
pub fn others_testing(peers: &[Peer]) -> Vec<Peer> {
    peers
        .iter()
        .filter(|p| !p.is_you && matches!(p.state, PeerState::Testing { .. }))
        .cloned()
        .collect()
}

/// Appends unseen sessions to the color order (first seen, first slot).
pub fn extend_order(order: &mut Vec<Uuid>, ids: impl IntoIterator<Item = Uuid>) {
    for id in ids {
        if !order.contains(&id) {
            order.push(id);
        }
    }
}

/// Moves `members` to the front of the color order, in member order, so
/// every member of a group test gets the same slots on every device.
pub fn members_first(order: &mut Vec<Uuid>, members: &[Uuid]) {
    order.retain(|id| !members.contains(id));
    let mut next: Vec<Uuid> = members.to_vec();
    next.append(order);
    *order = next;
}

/// The categorical color of `id` in `order` (slot 1 when unknown).
pub fn slot_of(order: &[Uuid], id: Uuid) -> Series {
    Series::slot(order.iter().position(|x| *x == id).unwrap_or(0))
}

/// "Even split" / "Uneven split" for a Jain's fairness index (SPEC §5.11).
pub fn fairness_label(index: f64) -> &'static str {
    if index >= 0.9 {
        "Even split"
    } else {
        "Uneven split"
    }
}

// ---------------------------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------------------------

/// The group this device is in (from `GroupUpdate`).
#[derive(Debug, Clone, PartialEq)]
pub struct GroupView {
    pub id: Uuid,
    pub host: Uuid,
    pub members: Vec<Peer>,
    pub state: GroupState,
}

/// Where this device is in a group test.
#[derive(Debug, Clone, PartialEq)]
pub enum GroupStage {
    /// No group test running here.
    Idle,
    /// `GroupStart` received: the plan starts at `starts_at` (`performance.now()` ms).
    Countdown { starts_at: f64 },
    /// Measuring.
    Running,
    /// This device finished; waiting for the server's result.
    Waiting,
    /// The server's result is in `PresenceCtx::group_result`.
    Done,
    /// This device's run failed (the others carry on; it is marked incomplete).
    Failed(String),
}

impl GroupStage {
    pub fn active(&self) -> bool {
        !matches!(self, GroupStage::Idle)
    }
}

/// A pending "test together" invite.
#[derive(Debug, Clone, PartialEq)]
pub struct Invite {
    pub group: Uuid,
    pub host: String,
}

/// Shared presence state. Cheap to copy.
#[derive(Clone, Copy)]
pub struct PresenceCtx {
    /// The server has presence on (`/api/info`), and the connection was started.
    pub enabled: RwSignal<bool>,
    pub connected: RwSignal<bool>,
    /// This page's session ID, once connected.
    pub session: RwSignal<Option<Uuid>>,
    /// Every visible device, this one included (`is_you`).
    pub peers: RwSignal<Vec<Peer>>,
    /// Hidden devices that are testing.
    pub hidden_active: RwSignal<u32>,
    /// Recent rates per device (oldest first), for sparklines.
    pub spark: RwSignal<HashMap<Uuid, Vec<f64>>>,
    /// Color order of sessions (see [`slot_of`]).
    pub order: RwSignal<Vec<Uuid>>,
    /// The label the next run is saved with (shared with the test page).
    pub label: RwSignal<String>,
    pub invite: RwSignal<Option<Invite>>,
    pub group: RwSignal<Option<GroupView>>,
    pub stage: RwSignal<GroupStage>,
    /// This device's current phase in a group test.
    pub group_phase: RwSignal<Phase>,
    /// Members when the group test started (lanes keep a member who left).
    pub group_members: RwSignal<Vec<Peer>>,
    pub group_result: RwSignal<Option<GroupResult>>,
    /// A short message for the toast (refused commands, cancelled groups).
    pub notice: RwSignal<Option<String>>,
    /// `performance.now()`, ticking while a group test runs.
    pub clock: RwSignal<f64>,
    /// The group plan's steps and its start (`performance.now()` ms).
    pub group_plan: RwSignal<Vec<crate::model::profile::Step>>,
    pub group_starts_at: RwSignal<f64>,
    #[cfg(feature = "hydrate")]
    handle: StoredValue<Option<crate::engine::presence::PresenceHandle>, LocalStorage>,
    #[cfg(feature = "hydrate")]
    run: StoredValue<Option<crate::engine::EngineHandle>, LocalStorage>,
}

impl PresenceCtx {
    fn new() -> Self {
        Self {
            enabled: RwSignal::new(false),
            connected: RwSignal::new(false),
            session: RwSignal::new(None),
            peers: RwSignal::new(Vec::new()),
            hidden_active: RwSignal::new(0),
            spark: RwSignal::new(HashMap::new()),
            order: RwSignal::new(Vec::new()),
            label: RwSignal::new(String::new()),
            invite: RwSignal::new(None),
            group: RwSignal::new(None),
            stage: RwSignal::new(GroupStage::Idle),
            group_phase: RwSignal::new(Phase::Idle),
            group_members: RwSignal::new(Vec::new()),
            group_result: RwSignal::new(None),
            notice: RwSignal::new(None),
            clock: RwSignal::new(0.0),
            group_plan: RwSignal::new(Vec::new()),
            group_starts_at: RwSignal::new(0.0),
            #[cfg(feature = "hydrate")]
            handle: StoredValue::new_local(None),
            #[cfg(feature = "hydrate")]
            run: StoredValue::new_local(None),
        }
    }

    /// The categorical color of a device.
    pub fn color(&self, id: Uuid) -> Series {
        self.order.with(|o| slot_of(o, id))
    }

    /// Other devices testing right now (tracked).
    pub fn others_testing(&self) -> Vec<Peer> {
        self.peers.with(|p| others_testing(p))
    }

    /// Whether this device is the host of its group.
    pub fn is_host(&self) -> bool {
        let me = self.session.get();
        self.group
            .with(|g| g.as_ref().is_some_and(|g| Some(g.host) == me))
    }

    /// The presence handle (client only, once connected).
    #[cfg(feature = "hydrate")]
    pub fn handle(&self) -> Option<crate::engine::presence::PresenceHandle> {
        self.handle.with_value(Clone::clone)
    }

    /// Announces the label (idle) to the other devices.
    pub fn announce_label(&self, label: Option<String>) {
        #[cfg(feature = "hydrate")]
        if let Some(h) = self.handle() {
            h.set_status(None, label);
        }
        #[cfg(not(feature = "hydrate"))]
        let _ = label;
    }

    /// "Test together": creates a group with this device as host.
    pub fn create_group(&self) {
        #[cfg(feature = "hydrate")]
        if let Some(h) = self.handle()
            && !h.create_group()
        {
            self.notice.set(Some("Not connected to the server".into()));
        }
    }

    pub fn join_group(&self, group: Uuid) {
        self.invite.set(None);
        #[cfg(feature = "hydrate")]
        if let Some(h) = self.handle()
            && !h.join_group(group)
        {
            self.notice.set(Some("Not connected to the server".into()));
        }
        #[cfg(not(feature = "hydrate"))]
        let _ = group;
    }

    pub fn start_group(&self) {
        #[cfg(feature = "hydrate")]
        if let Some(h) = self.handle() {
            h.start_group();
        }
    }

    /// Leaves the group (and stops this device's group run, if any).
    pub fn leave_group(&self) {
        #[cfg(feature = "hydrate")]
        {
            if let Some(h) = self.handle() {
                h.leave_group();
            }
            self.run.with_value(|r| {
                if let Some(r) = r {
                    r.abort();
                }
            });
        }
        self.group.set(None);
        self.close_result();
    }

    /// Closes the group result and returns to the solo test.
    pub fn close_result(&self) {
        self.stage.set(GroupStage::Idle);
        self.group_result.set(None);
        self.group_members.set(Vec::new());
        self.group_phase.set(Phase::Idle);
    }
}

/// The context provided by `App`.
pub fn use_presence() -> PresenceCtx {
    expect_context::<PresenceCtx>()
}

/// Creates the context and, on the client, connects once the stored device
/// name is known and the server reports presence on.
pub fn provide_presence(ui: crate::ui_state::UiState) -> PresenceCtx {
    let ctx = PresenceCtx::new();
    provide_context(ctx);

    #[cfg(feature = "hydrate")]
    {
        let started = StoredValue::new(false);
        Effect::new(move |_| {
            if !ui.loaded.get() || started.get_value() {
                return;
            }
            started.set_value(true);
            leptos::task::spawn_local(async move {
                let on = crate::pages::api::server_info()
                    .await
                    .map(|i| i.presence)
                    .unwrap_or(false);
                if on {
                    imp::connect(ctx, ui);
                }
            });
        });
        // "Hide me" follows the setting.
        Effect::new(move |prev: Option<bool>| {
            let hide = ui.hide_me.get();
            if prev.is_some()
                && let Some(h) = ctx.handle()
            {
                h.set_hidden(hide);
            }
            hide
        });
    }
    #[cfg(not(feature = "hydrate"))]
    let _ = ui;
    ctx
}

#[cfg(feature = "hydrate")]
mod imp {
    use super::*;
    use crate::engine::presence::{PresenceEvent, PresenceOptions, connect_presence};
    use crate::engine::{self, EngineEvent};
    use crate::model::profile::Profile;
    use crate::model::result::Transport;

    fn now() -> f64 {
        web_sys::window()
            .and_then(|w| w.performance())
            .map_or(0.0, |p| p.now())
    }

    pub fn connect(ctx: PresenceCtx, ui: crate::ui_state::UiState) {
        let mut device = engine::detect_device();
        let name = ui.device_name.get_untracked();
        if !name.trim().is_empty() {
            device.device_name = name.trim().chars().take(64).collect();
        }
        let mut opts = PresenceOptions::new(device);
        opts.hidden = ui.hide_me.get_untracked();
        opts.label = crate::components::label_picker::clean_label(&ctx.label.get_untracked());
        let session = opts.session;
        ctx.session.set(Some(session));
        ctx.enabled.set(true);
        let handle = connect_presence(opts, move |ev| on_event(ctx, ui, ev));
        ctx.handle.set_value(Some(handle));
    }

    fn on_event(ctx: PresenceCtx, ui: crate::ui_state::UiState, ev: PresenceEvent) {
        match ev {
            PresenceEvent::Connected { .. } => ctx.connected.set(true),
            PresenceEvent::Disconnected { .. } => {
                ctx.connected.set(false);
                ctx.peers.set(Vec::new());
                ctx.hidden_active.set(0);
                // Group membership doesn't survive a reconnect.
                if ctx
                    .group
                    .with_untracked(|g| g.as_ref().is_some_and(|g| g.state == GroupState::Forming))
                {
                    ctx.group.set(None);
                }
            }
            PresenceEvent::Peers(peers, hidden) => {
                ctx.order
                    .update(|o| extend_order(o, peers.iter().map(|p| p.id)));
                ctx.spark.update(|map| {
                    map.retain(|id, _| peers.iter().any(|p| p.id == *id));
                    for p in &peers {
                        match peer_rate(&p.state) {
                            Some(r) => {
                                let v = map.entry(p.id).or_default();
                                v.push(r);
                                if v.len() > SPARK_POINTS {
                                    v.remove(0);
                                }
                            }
                            None => {
                                map.remove(&p.id);
                            }
                        }
                    }
                });
                ctx.peers.set(peers);
                ctx.hidden_active.set(hidden);
            }
            PresenceEvent::GroupInvite { group, host } => {
                // Ignore invites while already in a group.
                if ctx.group.with_untracked(Option::is_none) {
                    ctx.invite.set(Some(Invite { group, host }));
                }
            }
            PresenceEvent::GroupUpdate {
                group,
                host,
                members,
                state,
            } => {
                let me = ctx.session.get_untracked();
                let member = members.iter().any(|m| Some(m.id) == me);
                if ctx
                    .invite
                    .with_untracked(|i| i.as_ref().is_some_and(|i| i.group == group))
                    && state != GroupState::Forming
                {
                    ctx.invite.set(None);
                }
                if member {
                    ctx.invite.set(None);
                    ctx.order.update(|o| {
                        members_first(o, &members.iter().map(|m| m.id).collect::<Vec<_>>())
                    });
                }
                match state {
                    GroupState::Cancelled => {
                        if ctx
                            .group
                            .with_untracked(|g| g.as_ref().is_some_and(|g| g.id == group))
                        {
                            ctx.group.set(None);
                            if ctx.stage.get_untracked() != GroupStage::Done {
                                ctx.notice.set(Some("The group test was cancelled".into()));
                                abort_run(ctx);
                                ctx.close_result();
                                ui.running.set(false);
                            }
                        }
                    }
                    _ if member => {
                        ctx.group.set(Some(GroupView {
                            id: group,
                            host,
                            members,
                            state,
                        }));
                    }
                    _ => {
                        // This device left (or was never in it).
                        if ctx
                            .group
                            .with_untracked(|g| g.as_ref().is_some_and(|g| g.id == group))
                        {
                            ctx.group.set(None);
                        }
                    }
                }
            }
            PresenceEvent::GroupStart {
                group,
                starts_in_ms,
                plan,
            } => start_group_run(ctx, ui, group, starts_in_ms, plan),
            PresenceEvent::GroupResult { group, result } => {
                if ctx
                    .group
                    .with_untracked(|g| g.as_ref().is_some_and(|g| g.id == group))
                    || ctx.stage.get_untracked().active()
                {
                    ctx.group_result.set(Some(*result));
                    ctx.stage.set(GroupStage::Done);
                    ctx.group_phase.set(Phase::Idle);
                    ui.running.set(false);
                }
            }
            PresenceEvent::Error { message, .. } => {
                ctx.notice.set(Some(message));
            }
        }
    }

    fn abort_run(ctx: PresenceCtx) {
        ctx.run.with_value(|r| {
            if let Some(r) = r {
                r.abort();
            }
        });
    }

    fn start_group_run(
        ctx: PresenceCtx,
        ui: crate::ui_state::UiState,
        group: Uuid,
        starts_in_ms: u32,
        plan: Profile,
    ) {
        let Some(handle) = ctx.handle() else { return };
        let members = ctx
            .group
            .with_untracked(|g| {
                g.as_ref()
                    .filter(|g| g.id == group)
                    .map(|g| g.members.clone())
            })
            .unwrap_or_default();
        ctx.group_members.set(members);
        ctx.group_result.set(None);
        ctx.group_phase.set(Phase::Idle);
        let starts_at = now() + f64::from(starts_in_ms);
        ctx.stage.set(GroupStage::Countdown { starts_at });
        ui.running.set(true);

        ctx.group_plan.set(plan.steps.clone());
        ctx.group_starts_at.set(starts_at);

        // Tick the clock (countdown, timeline) until the group test ends.
        ctx.clock.set(now());
        let interval = StoredValue::new(None::<IntervalHandle>);
        if let Ok(h) = set_interval_with_handle(
            move || {
                ctx.clock.set(now());
                if matches!(
                    ctx.stage.get_untracked(),
                    GroupStage::Idle | GroupStage::Done | GroupStage::Failed(_)
                ) && let Some(h) = interval.get_value()
                {
                    h.clear();
                }
            },
            std::time::Duration::from_millis(100),
        ) {
            interval.set_value(Some(h));
        }

        let label = crate::components::label_picker::clean_label(&ctx.label.get_untracked());
        let mut opts = handle.engine_options(Transport::Websocket, plan);
        opts.label = label.clone();
        opts.packet_loss = engine::default_packet_loss_probe();
        let session = opts.session;
        let on_event = move |ev: EngineEvent| match ev {
            EngineEvent::PhaseStarted { step, .. } => {
                if matches!(ctx.stage.get_untracked(), GroupStage::Countdown { .. }) {
                    ctx.stage.set(GroupStage::Running);
                }
                ctx.group_phase.set(step.phase());
            }
            EngineEvent::Finished(result) => {
                let mut result = *result;
                result.label = label.clone();
                ctx.group_phase.set(Phase::Idle);
                if ctx.stage.get_untracked() != GroupStage::Done {
                    ctx.stage.set(GroupStage::Waiting);
                }
                // The member's run carries its loaded latency into the group
                // result, so it is posted even when history is off.
                leptos::task::spawn_local(async move {
                    let _ = crate::pages::api::save_run(&result, session).await;
                });
            }
            EngineEvent::Error(msg) => {
                ctx.group_phase.set(Phase::Idle);
                if ctx.stage.get_untracked() != GroupStage::Done {
                    ctx.stage.set(GroupStage::Failed(msg));
                }
            }
            EngineEvent::Aborted => {
                ctx.group_phase.set(Phase::Idle);
            }
            _ => {}
        };
        let run = engine::start_at(opts, starts_in_ms, on_event);
        ctx.run.set_value(Some(run));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdown_math() {
        assert_eq!(countdown_digit(3000.0), Some(3));
        assert_eq!(countdown_digit(2001.0), Some(3));
        assert_eq!(countdown_digit(2000.0), Some(2));
        assert_eq!(countdown_digit(999.0), Some(1));
        assert_eq!(countdown_digit(1.0), Some(1));
        assert_eq!(countdown_digit(0.0), None);
        assert_eq!(countdown_digit(-50.0), None);
        assert_eq!(countdown_digit(f64::NAN), None);
    }

    #[test]
    fn fairness_labels() {
        assert_eq!(fairness_label(1.0), "Even split");
        assert_eq!(fairness_label(0.9), "Even split");
        assert_eq!(fairness_label(0.62), "Uneven split");
    }

    fn peer(n: u128, you: bool, state: PeerState) -> Peer {
        Peer {
            id: Uuid::from_u128(n),
            device_name: format!("D{n}"),
            device_kind: Default::default(),
            label: None,
            is_you: you,
            state,
        }
    }

    #[test]
    fn rates_and_testing_peers() {
        let testing = |phase| PeerState::Testing {
            phase,
            down_bps: 100.0,
            up_bps: 20.0,
            latency_ms: None,
        };
        assert_eq!(peer_rate(&PeerState::Idle), None);
        assert_eq!(peer_rate(&testing(Phase::Download)), Some(100.0));
        assert_eq!(peer_rate(&testing(Phase::Upload)), Some(20.0));
        assert_eq!(peer_rate(&testing(Phase::Latency)), Some(100.0));
        let peers = vec![
            peer(1, true, testing(Phase::Download)),
            peer(2, false, PeerState::Idle),
            peer(3, false, testing(Phase::Upload)),
        ];
        let others = others_testing(&peers);
        assert_eq!(others.len(), 1);
        assert_eq!(others[0].id, Uuid::from_u128(3));
    }

    #[test]
    fn color_order() {
        let (a, b, c) = (Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3));
        let mut order = Vec::new();
        extend_order(&mut order, [a, b]);
        extend_order(&mut order, [b, c]);
        assert_eq!(order, vec![a, b, c]);
        assert_eq!(slot_of(&order, c), Series::Device(3));
        members_first(&mut order, &[c, a]);
        assert_eq!(order, vec![c, a, b]);
        assert_eq!(slot_of(&order, c), Series::Device(1));
        assert_eq!(slot_of(&order, Uuid::from_u128(9)), Series::Device(1));
    }
}
