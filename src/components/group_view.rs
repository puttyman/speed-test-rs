//! "Test together" views (SPEC §7.11): the countdown, one live lane per
//! member during the test, and the server-computed group result (stacked
//! throughput over time, aggregate, shares, fairness, bottleneck warning).

use leptos::prelude::*;
use uuid::Uuid;

use super::charts::{Series, StackLayer, StackedArea};
use super::devices_panel::phase_title;
use super::icons;
use super::motion::reveal_style;
use super::presence::{GroupStage, countdown_digit, fairness_label, peer_phase, use_presence};
use crate::model::profile::Direction;
use crate::model::protocol::PeerState;
use crate::model::result::{GroupMember, GroupResult};
use crate::model::units::{PLACEHOLDER, format_ms, format_rate};
use crate::ui_state::use_ui_state;

// ---------------------------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------------------------

fn dir_value(s: &crate::model::result::RateSample, dir: Direction) -> f64 {
    match dir {
        Direction::Download => s.down_bps,
        Direction::Upload => s.up_bps,
    }
}

/// The time range (ms) in which any member moved data in `dir`, padded by
/// one sample on each side.
pub fn active_range(members: &[GroupMember], dir: Direction) -> Option<(u32, u32)> {
    let mut times: Vec<u32> = members
        .iter()
        .flat_map(|m| m.series.iter().map(|s| s.t_ms))
        .collect();
    times.sort_unstable();
    times.dedup();
    let busy = |t: u32| {
        members.iter().any(|m| {
            m.series
                .iter()
                .any(|s| s.t_ms == t && dir_value(s, dir) > 0.0)
        })
    };
    let first = times.iter().position(|&t| busy(t))?;
    let last = times.iter().rposition(|&t| busy(t))?;
    Some((
        times[first.saturating_sub(1)],
        times[(last + 1).min(times.len() - 1)],
    ))
}

/// Stacked-area layers of each member's throughput in `dir` (series
/// colors from `color`).
pub fn layers(
    members: &[GroupMember],
    dir: Direction,
    color: impl Fn(usize, &GroupMember) -> Series,
) -> Vec<StackLayer> {
    let Some((a, b)) = active_range(members, dir) else {
        return Vec::new();
    };
    members
        .iter()
        .enumerate()
        .map(|(i, m)| StackLayer {
            name: m.device_name.clone(),
            series: color(i, m),
            // Server samples are 250 ms apart; a 1 s rolling mean steadies them.
            points: super::charts::stacked_area::smooth(
                &m.series
                    .iter()
                    .filter(|s| (a..=b).contains(&s.t_ms))
                    .map(|s| (f64::from(s.t_ms), dir_value(s, dir)))
                    .collect::<Vec<_>>(),
                SMOOTH_SAMPLES,
            ),
        })
        .collect()
}

/// Where a timed plan is `elapsed_ms` after its start: per-step statuses
/// (finished steps `Done`), the running step and its progress. Steps without
/// a duration count as 5 s.
pub fn plan_progress(
    steps: &[crate::model::profile::Step],
    elapsed_ms: f64,
) -> (
    Vec<Option<crate::model::profile::PhaseStatus>>,
    Option<usize>,
    f64,
) {
    use crate::model::profile::PhaseStatus;
    let mut statuses = vec![None; steps.len()];
    let mut t = 0.0;
    for (i, s) in steps.iter().enumerate() {
        let d = f64::from(s.duration_ms().unwrap_or(5000)).max(1.0);
        if elapsed_ms < t + d {
            let p = ((elapsed_ms - t) / d).clamp(0.0, 1.0);
            return (statuses, (elapsed_ms >= 0.0).then_some(i), p);
        }
        statuses[i] = Some(PhaseStatus::Done);
        t += d;
    }
    (statuses, None, 0.0)
}

/// Samples in the rolling mean of the stacked charts (4 x 250 ms).
pub const SMOOTH_SAMPLES: usize = 4;

/// A share as a whole percentage ("42%").
pub fn share_text(share: Option<f64>) -> String {
    share.map_or_else(
        || PLACEHOLDER.to_string(),
        |s| format!("{:.0}%", (s * 100.0).clamp(0.0, 100.0)),
    )
}

// ---------------------------------------------------------------------------------------------
// Live: countdown and lanes
// ---------------------------------------------------------------------------------------------

/// The 3-2-1 overlay shown during a group countdown.
#[component]
pub fn Countdown() -> impl IntoView {
    let presence = use_presence();
    let digit = Memo::new(move |_| match presence.stage.get() {
        GroupStage::Countdown { starts_at } => countdown_digit(starts_at - presence.clock.get()),
        _ => None,
    });
    view! {
        {move || {
            digit
                .get()
                .map(|d| {
                    view! {
                        <div class="countdown" role="timer" aria-live="assertive">
                            <span class="countdown-label">"Starting together in"</span>
                            // Keyed by the digit so each one animates in.
                            <span class="countdown-digit" data-digit=d.to_string()>
                                {d.to_string()}
                            </span>
                        </div>
                    }
                })
        }}
    }
}

/// One lane per member: live bar and speed (from the server's counters).
#[component]
pub fn GroupLanes() -> impl IntoView {
    let presence = use_presence();
    let ui = use_ui_state();
    // The bars share a scale that only grows during the test.
    let scale = RwSignal::new(1e6_f64);
    let lanes = Memo::new(move |_| {
        let members = presence.group_members.get();
        let peers = presence.peers.get();
        members
            .into_iter()
            .map(|m| {
                let live = peers.iter().find(|p| p.id == m.id);
                let state = live.map(|p| p.state).unwrap_or_default();
                let rate = match state {
                    PeerState::Testing {
                        down_bps, up_bps, ..
                    } => down_bps + up_bps,
                    PeerState::Idle => 0.0,
                };
                (m, state, rate, live.is_none())
            })
            .collect::<Vec<_>>()
    });
    Effect::new(move |_| {
        let max = lanes.with(|l| l.iter().map(|x| x.2).fold(0.0, f64::max));
        if max > scale.get_untracked() {
            scale.set(max * 1.15);
        }
    });
    let me = presence.session;
    view! {
        <section class="card lanes-card" aria-label="Devices testing together">
            <ul class="lanes">
                {move || {
                    lanes
                        .get()
                        .into_iter()
                        .enumerate()
                        .map(|(i, (m, state, rate, gone))| {
                            let color = Series::slot(i);
                            let pct = (rate / scale.get() * 100.0).clamp(0.0, 100.0);
                            let (v, u) = if peer_phase(&state).is_some() {
                                format_rate(rate, ui.units.get())
                            } else {
                                (PLACEHOLDER.to_string(), "")
                            };
                            let phase = peer_phase(&state)
                                .map_or_else(
                                    || if gone { "Disconnected" } else { "Waiting" }.to_string(),
                                    |p| phase_title(p).to_string(),
                                );
                            let you = me.get() == Some(m.id);
                            view! {
                                <li class=format!("lane {}", color.class()) class:is-gone=gone>
                                    <div class="lane-head">
                                        <span class="key-dot" aria-hidden="true"></span>
                                        <span class="lane-name">
                                            {m.device_name.clone()}
                                            {you.then(|| view! { <span class="you-tag">"You"</span> })}
                                        </span>
                                        <span class="lane-phase">{phase}</span>
                                        <span class="lane-speed">
                                            <strong>{v}</strong>
                                            " "
                                            {u}
                                        </span>
                                    </div>
                                    <div class="lane-track" aria-hidden="true">
                                        <span class="lane-bar" style=format!("transform:scaleX({:.4})", pct / 100.0)></span>
                                    </div>
                                </li>
                            }
                        })
                        .collect_view()
                }}
            </ul>
        </section>
    }
}

/// Total live throughput of the group's members (bps).
pub fn use_group_total() -> Signal<Option<f64>> {
    let presence = use_presence();
    Signal::derive(move || {
        let ids: Vec<Uuid> = presence
            .group_members
            .with(|m| m.iter().map(|m| m.id).collect());
        presence.peers.with(|peers| {
            let total: f64 = peers
                .iter()
                .filter(|p| ids.contains(&p.id))
                .map(|p| match p.state {
                    PeerState::Testing {
                        down_bps, up_bps, ..
                    } => down_bps + up_bps,
                    PeerState::Idle => 0.0,
                })
                .sum();
            (total > 0.0).then_some(total)
        })
    })
}

// ---------------------------------------------------------------------------------------------
// Result
// ---------------------------------------------------------------------------------------------

/// The server-computed group result. `me` marks this device's row.
#[component]
pub fn GroupResultView(result: GroupResult, #[prop(optional)] me: Option<Uuid>) -> impl IntoView {
    let ui = use_ui_state();
    let rate = move |v: Option<f64>| {
        let (s, u) = format_rate(v.unwrap_or(f64::NAN), ui.units.get());
        (s, u)
    };
    let n = result.members.len();
    let complete = result.members.iter().filter(|m| !m.incomplete).count();
    let color = |i: usize, _: &GroupMember| Series::slot(i);
    let down_layers = layers(&result.members, Direction::Download, color);
    let up_layers = layers(&result.members, Direction::Upload, color);
    let link = result.server_link_bps;
    let warn = result.bottleneck_warning;
    let members = result.members.clone();
    let agg_down = result.aggregate_download_bps;
    let agg_up = result.aggregate_upload_bps;
    let fd = result.fairness_download;
    let fu = result.fairness_upload;
    let window_s = result.window_ms as f64 / 1000.0;

    let fairness = move |label: &'static str, index: Option<f64>| {
        let pct = index.map_or(0.0, |i| (i * 100.0).clamp(0.0, 100.0));
        let even = index.is_some_and(|i| i >= 0.9);
        view! {
            <div class="fairness" class:is-even=even class:is-uneven=index.is_some() && !even>
                <div class="fairness-head">
                    <span class="fairness-dir">{label}</span>
                    <span class="fairness-verdict">
                        {index.map_or("Not enough data", fairness_label)}
                    </span>
                </div>
                <div
                    class="meter"
                    role="meter"
                    aria-valuemin="0"
                    aria-valuemax="1"
                    aria-valuenow=index.map(|i| format!("{i:.2}"))
                    aria-label=format!("{label} fairness")
                >
                    <span class="meter-fill" style=format!("width:{pct:.1}%")></span>
                </div>
                <span class="fairness-index">
                    {index.map_or_else(String::new, |i| format!("Jain's index {i:.2}"))}
                </span>
            </div>
        }
    };

    view! {
        <section class="group-result" aria-labelledby="group-result-title">
            <header class="group-result-head">
                <h2 id="group-result-title" class="section-title">"Tested together"</h2>
                <p class="muted">
                    {format!(
                        "{n} devices · {complete} finished · measured by the server over {window_s:.0} s",
                    )}
                </p>
            </header>

            <div class="group-totals">
                <article class="metric-tile series-download reveal" style=reveal_style(0)>
                    <header class="metric-head">
                        <span class="metric-dot" aria-hidden="true"></span>
                        <h3 class="metric-label">"Total download"</h3>
                    </header>
                    <p class="metric-value">
                        <span class="value">{move || rate(agg_down).0}</span>
                        <span class="unit">{move || rate(agg_down).1}</span>
                    </p>
                    <p class="metric-detail">"Sum of every device"</p>
                </article>
                <article class="metric-tile series-upload reveal" style=reveal_style(1)>
                    <header class="metric-head">
                        <span class="metric-dot" aria-hidden="true"></span>
                        <h3 class="metric-label">"Total upload"</h3>
                    </header>
                    <p class="metric-value">
                        <span class="value">{move || rate(agg_up).0}</span>
                        <span class="unit">{move || rate(agg_up).1}</span>
                    </p>
                    <p class="metric-detail">"Sum of every device"</p>
                </article>
                <article class="metric-tile fairness-tile reveal" style=reveal_style(2)>
                    <header class="metric-head">
                        <h3 class="metric-label">"How evenly it was shared"</h3>
                    </header>
                    {fairness("Download", fd)}
                    {fairness("Upload", fu)}
                </article>
            </div>

            {warn
                .then(|| {
                    let (s, u) = format_rate(link.unwrap_or(f64::NAN), ui.units.get_untracked());
                    view! {
                        <p class="callout callout-warn" role="note">
                            <span class="callout-icon" aria-hidden="true">{icons::warning()}</span>
                            {format!(
                                "Total is close to the server's {s} {u} link: the server may be the bottleneck, not the Wi-Fi.",
                            )}
                        </p>
                    }
                })}
            {(complete < n)
                .then(|| {
                    view! {
                        <p class="callout" role="note">
                            <span class="callout-icon" aria-hidden="true">{icons::info()}</span>
                            "A device dropped out during the test. Totals and shares cover the devices that finished."
                        </p>
                    }
                })}

            <div class="details-grid">
                <article class="card detail-card reveal" style=reveal_style(3)>
                    <h3 class="card-title">"Download over time"</h3>
                    <p class="card-sub">"Each device's throughput, stacked (1 s rolling average)."</p>
                    <StackedArea
                        layers=Signal::stored(down_layers)
                        aria_label="Download throughput of each device over time, stacked"
                        table_summary="Download values"
                    />
                </article>
                <article class="card detail-card reveal" style=reveal_style(4)>
                    <h3 class="card-title">"Upload over time"</h3>
                    <p class="card-sub">"Each device's throughput, stacked (1 s rolling average)."</p>
                    <StackedArea
                        layers=Signal::stored(up_layers)
                        aria_label="Upload throughput of each device over time, stacked"
                        table_summary="Upload values"
                    />
                </article>
            </div>

            <article class="card members-card reveal" style=reveal_style(5)>
                <h3 class="card-title">"Each device"</h3>
                <div class="members-table-wrap">
                    <table class="members-table">
                        <thead>
                            <tr>
                                <th scope="col">"Device"</th>
                                <th scope="col">"Download"</th>
                                <th scope="col">"Upload"</th>
                                <th scope="col" class="col-latency">"Latency under load"</th>
                            </tr>
                        </thead>
                        <tbody>
                            {members
                                .into_iter()
                                .enumerate()
                                .map(|(i, m)| {
                                    let series = Series::slot(i);
                                    let you = me == Some(m.session);
                                    let loaded = m
                                        .down_loaded_latency_ms
                                        .into_iter()
                                        .chain(m.up_loaded_latency_ms)
                                        .reduce(f64::max)
                                        .map_or_else(
                                            || PLACEHOLDER.to_string(),
                                            |v| {
                                                let (s, u) = format_ms(v);
                                                format!("{s} {u}")
                                            },
                                        );
                                    let cell = move |v: Option<f64>, share: Option<f64>| {
                                        let pct = share.map_or(0.0, |s| (s * 100.0).clamp(0.0, 100.0));
                                        view! {
                                            <div class="share-cell">
                                                <span class="share-value">
                                                    {move || {
                                                        let (s, u) = rate(v);
                                                        format!("{s} {u}")
                                                    }}
                                                </span>
                                                <span class="share-pct">{share_text(share)}</span>
                                                <span class="share-track" aria-hidden="true">
                                                    <span class="share-bar" style=format!("width:{pct:.1}%")></span>
                                                </span>
                                            </div>
                                        }
                                    };
                                    view! {
                                        <tr class=series.class() class:is-incomplete=m.incomplete>
                                            <th scope="row">
                                                <span class="member-cell">
                                                    <span class="device-icon" aria-hidden="true">
                                                        {icons::device_kind(m.device_kind)}
                                                    </span>
                                                    <span class="member-text">
                                                        <span class="member-name">
                                                            {m.device_name.clone()}
                                                            {you.then(|| view! { <span class="you-tag">"You"</span> })}
                                                        </span>
                                                        <span class="member-sub">
                                                            {if m.incomplete {
                                                                "Dropped out".to_string()
                                                            } else {
                                                                m.label.clone().unwrap_or_default()
                                                            }}
                                                        </span>
                                                    </span>
                                                </span>
                                            </th>
                                            <td data-label="Download">{cell(m.download_bps, m.download_share)}</td>
                                            <td data-label="Upload">{cell(m.upload_bps, m.upload_share)}</td>
                                            <td class="col-latency">{loaded}</td>
                                        </tr>
                                    }
                                })
                                .collect_view()}
                        </tbody>
                    </table>
                </div>
            </article>
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::result::RateSample;

    fn member(name: &str, series: &[(u32, f64, f64)]) -> GroupMember {
        GroupMember {
            session: Uuid::new_v4(),
            device_name: name.into(),
            device_kind: Default::default(),
            label: None,
            download_bps: None,
            upload_bps: None,
            download_share: None,
            upload_share: None,
            down_loaded_latency_ms: None,
            up_loaded_latency_ms: None,
            incomplete: false,
            series: series
                .iter()
                .map(|&(t_ms, down_bps, up_bps)| RateSample {
                    t_ms,
                    down_bps,
                    up_bps,
                })
                .collect(),
        }
    }

    #[test]
    fn active_ranges_per_direction() {
        let a = member(
            "A",
            &[
                (0, 0.0, 0.0),
                (250, 5.0, 0.0),
                (500, 6.0, 0.0),
                (750, 0.0, 0.0),
                (1000, 0.0, 0.0),
                (1250, 0.0, 3.0),
                (1500, 0.0, 0.0),
            ],
        );
        let b = member("B", &[(0, 0.0, 0.0), (500, 2.0, 0.0), (1500, 0.0, 0.0)]);
        let m = [a, b];
        assert_eq!(active_range(&m, Direction::Download), Some((0, 750)));
        assert_eq!(active_range(&m, Direction::Upload), Some((1000, 1500)));
        let l = layers(&m, Direction::Download, |i, _| Series::slot(i));
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].points.len(), 4);
        assert_eq!(l[1].series, Series::Device(2));
        assert!(
            layers(
                &[member("C", &[(0, 0.0, 0.0)])],
                Direction::Upload,
                |i, _| Series::slot(i)
            )
            .is_empty()
        );
    }

    #[test]
    fn plan_progress_follows_the_clock() {
        use crate::model::profile::{PhaseStatus, Profile};
        let steps = Profile::concurrent().steps; // 3 s, 10 s, 2 s, 10 s
        let (st, cur, p) = plan_progress(&steps, -500.0);
        assert_eq!((cur, p), (None, 0.0));
        assert!(st.iter().all(Option::is_none));
        let (st, cur, p) = plan_progress(&steps, 8000.0);
        assert_eq!(cur, Some(1));
        assert!((p - 0.5).abs() < 1e-9);
        assert_eq!(st[0], Some(PhaseStatus::Done));
        let (st, cur, _) = plan_progress(&steps, 30_000.0);
        assert_eq!(cur, None);
        assert!(st.iter().all(|s| *s == Some(PhaseStatus::Done)));
    }

    #[test]
    fn shares() {
        assert_eq!(share_text(Some(0.4249)), "42%");
        assert_eq!(share_text(Some(1.2)), "100%");
        assert_eq!(share_text(None), PLACEHOLDER);
    }
}
