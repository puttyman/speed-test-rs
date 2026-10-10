//! The Devices panel (SPEC §7.11): every browser with the app open on this
//! server, with its state and live speed, and the "Test together" controls.

use leptos::prelude::*;
use uuid::Uuid;

use super::charts::Sparkline;
use super::icons;
use super::presence::{GroupView, peer_phase, peer_rate, use_presence};
use crate::components::phase_timeline::PHASES;
use crate::model::protocol::{GroupState, Peer, Phase};
use crate::model::units::format_rate;
use crate::ui_state::use_ui_state;

/// How long a leaving card takes to fade out (`--dur-slow`).
const LEAVE_MS: u64 = 600;

/// Display name of a phase ("Download"…).
pub fn phase_title(phase: Phase) -> &'static str {
    match phase {
        Phase::Pause => "Pause",
        Phase::Idle => "Starting",
        p => PHASES
            .iter()
            .find(|(x, _)| *x == p)
            .map_or("Test", |(_, n)| n),
    }
}

/// The next list of shown cards: current peers (this device first), and
/// cards of peers that just left marked as leaving.
pub fn next_cards(shown: &[(Peer, bool)], peers: &[Peer]) -> Vec<(Peer, bool)> {
    let mut out: Vec<(Peer, bool)> = Vec::with_capacity(peers.len() + shown.len());
    // Keep the existing order for stability; new devices are appended.
    for (p, _) in shown {
        match peers.iter().find(|q| q.id == p.id) {
            Some(q) => out.push((q.clone(), false)),
            None => out.push((p.clone(), true)),
        }
    }
    for q in peers {
        if !out.iter().any(|(p, _)| p.id == q.id) {
            out.push((q.clone(), false));
        }
    }
    // This device first.
    out.sort_by_key(|(p, _)| !p.is_you);
    out
}

#[component]
pub fn DevicesPanel(
    /// True while this device runs a solo test (disables "Test together").
    #[prop(into)]
    busy: Signal<bool>,
) -> impl IntoView {
    let presence = use_presence();
    let ui = use_ui_state();
    let shown = RwSignal::new(Vec::<(Peer, bool)>::new());

    Effect::new(move |_| {
        let peers = presence.peers.get();
        let next = shown.with_untracked(|s| next_cards(s, &peers));
        let leaving: Vec<Uuid> = next.iter().filter(|(_, l)| *l).map(|(p, _)| p.id).collect();
        shown.set(next);
        if !leaving.is_empty() {
            set_timeout(
                move || {
                    shown.update(|s| s.retain(|(p, l)| !(*l && leaving.contains(&p.id))));
                },
                std::time::Duration::from_millis(LEAVE_MS),
            );
        }
    });

    let others = Memo::new(move |_| {
        presence
            .peers
            .with(|p| p.iter().filter(|p| !p.is_you).count())
    });
    let hidden = presence.hidden_active;
    let alone = Memo::new(move |_| others.get() == 0 && hidden.get() == 0);
    let in_group = Memo::new(move |_| presence.group.with(Option::is_some));
    let can_invite = Signal::derive(move || {
        presence.connected.get()
            && others.get() > 0
            && !in_group.get()
            && !busy.get()
            && !presence.stage.with(|s| s.active())
    });

    let card = move |id: Uuid| {
        let peer = Memo::new(move |_| shown.with(|s| s.iter().find(|(p, _)| p.id == id).cloned()));
        let leaving = move || peer.with(|p| p.as_ref().is_some_and(|(_, l)| *l));
        let get = move || peer.with(|p| p.as_ref().map(|(p, _)| p.clone()));
        let color = move || presence.color(id);
        let testing = move || get().is_some_and(|p| peer_phase(&p.state).is_some());
        let spark = Signal::derive(move || {
            presence
                .spark
                .with(|m| m.get(&id).cloned().unwrap_or_default())
        });
        view! {
            <li
                class=move || format!("device-card {}", color().class())
                class:is-you=move || get().is_some_and(|p| p.is_you)
                class:is-testing=testing
                class:is-leaving=leaving
            >
                <span class="device-icon" aria-hidden="true">
                    {move || get().map(|p| icons::device_kind(p.device_kind))}
                </span>
                <div class="device-main">
                    <p class="device-name">
                        <span class="device-name-text">
                            {move || get().map(|p| p.device_name).unwrap_or_default()}
                        </span>
                        {move || {
                            get().filter(|p| p.is_you).map(|_| view! { <span class="you-tag">"You"</span> })
                        }}
                    </p>
                    <p class="device-label">
                        {move || get().and_then(|p| p.label).unwrap_or_else(|| "No label".into())}
                    </p>
                </div>
                <div class="device-state">
                    {move || match get().and_then(|p| peer_phase(&p.state).map(|ph| (ph, peer_rate(&p.state)))) {
                        None => view! { <span class="device-idle">"Idle"</span> }.into_any(),
                        Some((phase, rate)) => {
                            let speed = matches!(phase, Phase::Download | Phase::Upload)
                                .then(|| {
                                    let (v, u) = format_rate(rate.unwrap_or(0.0), ui.units.get());
                                    format!("{v} {u}")
                                });
                            view! {
                                <span class="device-phase">
                                    <span class="live-dot" aria-hidden="true"></span>
                                    {format!("Testing · {}", phase_title(phase))}
                                </span>
                                {speed.map(|s| view! { <span class="device-speed">{s}</span> })}
                            }
                                .into_any()
                        }
                    }}
                    <div class="device-spark" class:is-visible=testing>
                        {move || view! { <Sparkline values=spark series=color()/> }}
                    </div>
                </div>
            </li>
        }
    };

    view! {
        <Show when=move || presence.enabled.get()>
            <section
                class="devices"
                class:is-collapsed=move || alone.get() && !in_group.get()
                aria-labelledby="devices-title"
            >
                <header class="devices-head">
                    <h2 id="devices-title" class="devices-title">
                        <span class="presence-dot" class:is-on=move || presence.connected.get()></span>
                        {move || {
                            if !presence.connected.get() {
                                "Connecting to the server…".to_string()
                            } else if alone.get() && !in_group.get() {
                                "Only this device is connected".to_string()
                            } else {
                                let n = others.get() + 1;
                                format!("{n} devices on this server")
                            }
                        }}
                    </h2>
                    <Show when=move || can_invite.get()>
                        <button
                            type="button"
                            class="pill-button pill-sm pill-ghost"
                            on:click=move |_| presence.create_group()
                        >
                            {icons::users()}
                            "Test together"
                        </button>
                    </Show>
                </header>
                {move || presence.group.get().filter(|g| g.state == GroupState::Forming).map(|g| view! { <GroupBar group=g/> })}
                <Show when=move || !(alone.get() && !in_group.get())>
                    <ul class="device-list" aria-label="Devices">
                        <For each=move || shown.with(|s| s.iter().map(|(p, _)| p.id).collect::<Vec<_>>()) key=|id| *id let:id>
                            {card(id)}
                        </For>
                    </ul>
                </Show>
                {move || {
                    let n = hidden.get();
                    (n > 0)
                        .then(|| {
                            view! {
                                <p class="hidden-note">
                                    {if n == 1 {
                                        "1 hidden device testing".to_string()
                                    } else {
                                        format!("{n} hidden devices testing")
                                    }}
                                </p>
                            }
                        })
                }}
            </section>
        </Show>
    }
}

/// The forming group: members so far and Start (host) or a waiting note.
#[component]
fn GroupBar(group: GroupView) -> impl IntoView {
    let presence = use_presence();
    let me = presence.session.get_untracked();
    let host = Some(group.host) == me;
    let n = group.members.len();
    let host_name = group
        .members
        .iter()
        .find(|m| m.id == group.host)
        .map(|m| m.device_name.clone())
        .unwrap_or_else(|| "the host".into());
    view! {
        <div class="group-bar" role="group" aria-label="Test together">
            <div class="group-bar-text">
                <p class="group-bar-title">
                    {if host {
                        if n < 2 {
                            "Invite sent. Waiting for devices to join…".to_string()
                        } else {
                            format!("{n} devices ready to test together")
                        }
                    } else {
                        format!("Joined. Waiting for {host_name} to start…")
                    }}
                </p>
                <ul class="member-chips">
                    {group
                        .members
                        .iter()
                        .map(|m| {
                            let color = presence.color(m.id);
                            let you = Some(m.id) == me;
                            view! {
                                <li class=format!("member-chip {}", color.class())>
                                    <span class="key-dot" aria-hidden="true"></span>
                                    {m.device_name.clone()}
                                    {you.then_some(" (you)")}
                                </li>
                            }
                        })
                        .collect_view()}
                </ul>
            </div>
            <div class="group-bar-actions">
                {host
                    .then(|| {
                        view! {
                            <button
                                type="button"
                                class="pill-button pill-sm group-start"
                                disabled=n < 2
                                on:click=move |_| presence.start_group()
                            >
                                "Start"
                            </button>
                        }
                    })}
                <button
                    type="button"
                    class="pill-button pill-sm pill-ghost"
                    on:click=move |_| presence.leave_group()
                >
                    {if host { "Cancel" } else { "Leave" }}
                </button>
            </div>
        </div>
    }
}

/// The "wants to test together" toast with Join, shown on every page.
#[component]
pub fn InviteToast() -> impl IntoView {
    let presence = use_presence();
    let navigate = leptos_router::hooks::use_navigate();
    // Notices disappear after a few seconds.
    Effect::new(move |_| {
        if presence.notice.with(Option::is_some) {
            set_timeout(
                move || presence.notice.set(None),
                std::time::Duration::from_millis(4000),
            );
        }
    });
    view! {
        {move || {
            presence
                .invite
                .get()
                .map(|inv| {
                    let navigate = navigate.clone();
                    let group = inv.group;
                    view! {
                        <div class="toast invite-toast" role="alert">
                            <span class="toast-icon" aria-hidden="true">{icons::users()}</span>
                            <span class="toast-text">{format!("{} wants to test together", inv.host)}</span>
                            <button
                                type="button"
                                class="toast-action"
                                on:click=move |_| {
                                    presence.join_group(group);
                                    navigate("/", Default::default());
                                }
                            >
                                "Join"
                            </button>
                            <button
                                type="button"
                                class="toast-dismiss"
                                aria-label="Dismiss"
                                on:click=move |_| presence.invite.set(None)
                            >
                                "×"
                            </button>
                        </div>
                    }
                })
        }}
        {move || {
            presence
                .notice
                .get()
                .filter(|_| presence.invite.with(Option::is_none))
                .map(|msg| {
                    view! {
                        <div class="toast" role="status">
                            <span class="toast-text">{msg}</span>
                            <button
                                type="button"
                                class="toast-dismiss"
                                aria-label="Dismiss"
                                on:click=move |_| presence.notice.set(None)
                            >
                                "×"
                            </button>
                        </div>
                    }
                })
        }}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(n: u128, you: bool) -> Peer {
        Peer {
            id: Uuid::from_u128(n),
            device_name: format!("D{n}"),
            device_kind: Default::default(),
            label: None,
            is_you: you,
            state: Default::default(),
        }
    }

    #[test]
    fn cards_keep_order_and_mark_leavers() {
        let first = next_cards(&[], &[peer(2, false), peer(1, true)]);
        assert_eq!(
            first
                .iter()
                .map(|(p, _)| p.id.as_u128())
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        let next = next_cards(&first, &[peer(3, false), peer(1, true)]);
        let ids: Vec<(u128, bool)> = next.iter().map(|(p, l)| (p.id.as_u128(), *l)).collect();
        assert_eq!(ids, vec![(1, false), (2, true), (3, false)]);
    }

    #[test]
    fn phase_titles() {
        assert_eq!(phase_title(Phase::Download), "Download");
        assert_eq!(phase_title(Phase::Pause), "Pause");
    }
}
