//! Live presence (SPEC §5.11 "Live presence"): the `Peers` snapshots pushed to every
//! control socket.
//!
//! - One [`Peer`] per session that has a control socket. Device name and kind come from the
//!   control socket's `Hello`, the label and phase from its last `Status`. Live rates come
//!   from the registry's byte counters (sampled every 250 ms), never from client reports.
//! - A session is `Testing` while it reports a non-idle phase or has a transfer in flight
//!   (the phase is then guessed from the dominant direction).
//! - Hidden sessions (`SetHidden{true}`) are left out of everybody else's list. While they
//!   test, they are counted in `hidden_active`, together with sessions that move data but
//!   have no control socket (scripts, HTTP-only clients). A hidden device still sees itself
//!   (`is_you`).
//! - The broadcaster sends a snapshot to every control socket at most every
//!   [`BROADCAST_INTERVAL`] (4 Hz), and only when something changed: a registry bump
//!   (socket attached/detached, status, hidden flag) or a meaningful rate change
//!   ([`rates_differ`]).
//! - A new control socket gets one immediate snapshot right after its `Welcome`
//!   ([`snapshot_for`]), outside the throttle.
//!
//! Peers are listed in a stable order: by device name, then session ID.

use std::time::Duration;

use tokio::task::JoinHandle;
use tokio::time::Instant;
use uuid::Uuid;

use super::sessions::{SessionRegistry, SessionSnapshot};
use crate::model::protocol::{DeviceKind, Peer, PeerState, Phase, ServerMsg};

/// Minimum time between two broadcasts.
pub const BROADCAST_INTERVAL: Duration = Duration::from_millis(250);
/// Rates closer than this (relative) are "unchanged"...
const RATE_REL_EPSILON: f64 = 0.05;
/// ...and so are rates closer than this (absolute, bps).
const RATE_ABS_EPSILON: f64 = 100_000.0;
/// Name shown for a session that never sent device info.
pub const UNKNOWN_DEVICE: &str = "Unknown device";

/// One session as seen by presence.
#[derive(Debug, Clone, PartialEq)]
struct Entry {
    peer: Peer,
    hidden: bool,
    /// Has a control socket (listed); otherwise only counted when testing.
    listed: bool,
}

impl Entry {
    fn testing(&self) -> bool {
        matches!(self.peer.state, PeerState::Testing { .. })
    }
}

/// The presence state of a session (`is_you` false).
pub fn peer_of(s: &SessionSnapshot) -> Peer {
    let (device_name, device_kind) = match &s.device {
        Some(d) => (d.device_name.clone(), d.device_kind),
        None => (UNKNOWN_DEVICE.to_owned(), DeviceKind::Other),
    };
    let phase = s.phase.filter(|p| *p != Phase::Idle).or_else(|| {
        let dominant = if s.rates.up_bps > s.rates.down_bps {
            Phase::Upload
        } else {
            Phase::Download
        };
        (s.active_transfers > 0).then_some(dominant)
    });
    let state = match phase {
        None => PeerState::Idle,
        Some(phase) => PeerState::Testing {
            phase,
            down_bps: s.rates.down_bps,
            up_bps: s.rates.up_bps,
            // The server can't time the client's probes; latency stays client-side.
            latency_ms: None,
        },
    };
    Peer {
        id: s.id,
        device_name,
        device_kind,
        label: s.label.clone(),
        is_you: false,
        state,
    }
}

fn entries(snapshots: Vec<SessionSnapshot>) -> Vec<Entry> {
    let mut out: Vec<Entry> = snapshots
        .into_iter()
        .filter(|s| !s.closed)
        .map(|s| Entry {
            peer: peer_of(&s),
            hidden: s.hidden,
            listed: s.control_sockets > 0,
        })
        .collect();
    out.sort_by(|a, b| {
        a.peer
            .device_name
            .cmp(&b.peer.device_name)
            .then(a.peer.id.cmp(&b.peer.id))
    });
    out
}

/// The `Peers` message for `recipient`.
fn message_for(entries: &[Entry], recipient: Uuid) -> ServerMsg {
    let mut peers = Vec::new();
    let mut hidden_active = 0u32;
    for e in entries {
        let you = e.peer.id == recipient;
        if e.listed && (!e.hidden || you) {
            peers.push(Peer {
                is_you: you,
                ..e.peer.clone()
            });
        } else if !you && e.testing() {
            hidden_active += 1;
        }
    }
    ServerMsg::Peers {
        peers,
        hidden_active,
    }
}

/// The current `Peers` snapshot as `recipient` should see it.
pub fn snapshot_for(registry: &SessionRegistry, recipient: Uuid) -> ServerMsg {
    message_for(&entries(registry.snapshot()), recipient)
}

/// True if two rates differ enough to be worth a new frame.
pub fn rates_differ(a: f64, b: f64) -> bool {
    let diff = (a - b).abs();
    diff > RATE_ABS_EPSILON && diff > RATE_REL_EPSILON * a.abs().max(b.abs())
}

fn state_differs(a: &PeerState, b: &PeerState) -> bool {
    match (a, b) {
        (PeerState::Idle, PeerState::Idle) => false,
        (
            PeerState::Testing {
                phase: pa,
                down_bps: da,
                up_bps: ua,
                ..
            },
            PeerState::Testing {
                phase: pb,
                down_bps: db,
                up_bps: ub,
                ..
            },
        ) => pa != pb || rates_differ(*da, *db) || rates_differ(*ua, *ub),
        _ => true,
    }
}

fn entries_differ(a: &[Entry], b: &[Entry]) -> bool {
    a.len() != b.len()
        || a.iter().zip(b).any(|(x, y)| {
            x.hidden != y.hidden
                || x.listed != y.listed
                || x.peer.id != y.peer.id
                || x.peer.device_name != y.peer.device_name
                || x.peer.device_kind != y.peer.device_kind
                || x.peer.label != y.peer.label
                || state_differs(&x.peer.state, &y.peer.state)
        })
}

/// Sends one snapshot to every control socket. Returns how many frames were queued.
fn broadcast(registry: &SessionRegistry, entries: &[Entry]) -> usize {
    let mut sent = 0;
    let mut cache: Option<(Uuid, ServerMsg)> = None;
    let mut senders = registry.control_senders();
    senders.sort_by_key(|(id, _)| *id);
    for (session, tx) in senders {
        let msg = match &cache {
            Some((id, msg)) if *id == session => msg.clone(),
            _ => {
                let msg = message_for(entries, session);
                cache = Some((session, msg.clone()));
                msg
            }
        };
        // A full queue drops this frame; the next snapshot is complete anyway.
        if tx.try_send(msg).is_ok() {
            sent += 1;
        }
    }
    sent
}

/// Spawns the presence broadcaster on the current Tokio runtime. It ends when the registry
/// is dropped. Returns `None` outside a runtime.
pub fn spawn_broadcaster(registry: &SessionRegistry) -> Option<JoinHandle<()>> {
    let handle = tokio::runtime::Handle::try_current().ok()?;
    let weak = registry.downgrade();
    let mut changes = registry.subscribe();
    Some(handle.spawn(async move {
        let mut last: Option<Vec<Entry>> = None;
        let mut last_sent: Option<Instant> = None;
        loop {
            // Wake on a registry bump, or after one sampler interval to look at the rates.
            let bumped = tokio::select! {
                r = changes.changed() => {
                    if r.is_err() {
                        break;
                    }
                    true
                }
                _ = tokio::time::sleep(BROADCAST_INTERVAL) => false,
            };
            if let Some(at) = last_sent {
                tokio::time::sleep_until(at + BROADCAST_INTERVAL).await;
            }
            // Anything that changed while throttled is included in this snapshot.
            let bumped = bumped || changes.has_changed().unwrap_or(false);
            changes.mark_unchanged();
            let Some(registry) = weak.upgrade() else {
                break;
            };
            let current = entries(registry.snapshot());
            let changed = bumped || last.as_deref().is_none_or(|l| entries_differ(l, &current));
            if changed {
                broadcast(&registry, &current);
                last = Some(current);
                last_sent = Some(Instant::now());
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::protocol::DeviceInfo;
    use crate::server::sessions::Rates;

    fn snap(n: u128, name: &str, hidden: bool, control: usize, transfers: u32) -> SessionSnapshot {
        SessionSnapshot {
            id: Uuid::from_u128(n),
            device: Some(DeviceInfo {
                device_id: format!("d{n}"),
                device_name: name.into(),
                device_kind: DeviceKind::Phone,
            }),
            phase: None,
            label: None,
            hidden,
            control_sockets: control,
            data_sockets: 0,
            probe_sockets: 0,
            active_transfers: transfers,
            bytes_sent: 0,
            bytes_received: 0,
            rates: Rates {
                down_bps: 1e8,
                up_bps: 0.0,
            },
            idle_for: Duration::ZERO,
            closed: false,
        }
    }

    #[test]
    fn hidden_and_unlisted_sessions_are_counted_not_listed() {
        let e = entries(vec![
            snap(1, "B", false, 1, 0),
            snap(2, "A", true, 1, 1),
            snap(3, "C", false, 0, 1),
            snap(4, "D", true, 1, 0),
        ]);
        let ServerMsg::Peers {
            peers,
            hidden_active,
        } = message_for(&e, Uuid::from_u128(1))
        else {
            unreachable!()
        };
        assert_eq!(
            peers
                .iter()
                .map(|p| p.device_name.as_str())
                .collect::<Vec<_>>(),
            ["B"]
        );
        assert!(peers[0].is_you);
        assert_eq!(hidden_active, 2);
        // A hidden device sees itself, and isn't counted for itself.
        let ServerMsg::Peers {
            peers,
            hidden_active,
        } = message_for(&e, Uuid::from_u128(2))
        else {
            unreachable!()
        };
        assert_eq!(peers.len(), 2);
        assert!(peers[0].is_you && peers[0].device_name == "A");
        assert!(matches!(
            peers[0].state,
            PeerState::Testing {
                phase: Phase::Download,
                ..
            }
        ));
        assert_eq!(hidden_active, 1);
    }

    #[test]
    fn rate_changes_need_to_be_meaningful() {
        assert!(!rates_differ(1e8, 1.02e8));
        assert!(rates_differ(1e8, 1.2e8));
        assert!(!rates_differ(0.0, 50_000.0));
        assert!(rates_differ(0.0, 500_000.0));
        let a = entries(vec![snap(1, "A", false, 1, 1)]);
        let mut b = a.clone();
        assert!(!entries_differ(&a, &b));
        b[0].peer.label = Some("Desk".into());
        assert!(entries_differ(&a, &b));
    }
}
