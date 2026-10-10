//! Pieces of a finished run shared by the test page and the run page
//! (`/runs/{id}`, SPEC §7.8): metric tiles from a saved result, the detail
//! charts, the run facts card and the "Shared network" badge.

use leptos::prelude::*;

use super::MetricKind;
use super::charts::{BoxPlotChart, BoxRow, DotPlot, LossBar, Series};
use super::metric_tile::MetricTile;
use super::motion::reveal_style;
use super::server_card::connection_label;
use crate::model::device::month_abbrev;
use crate::model::result::{Concurrency, RunResult, Transport};
use crate::model::units::{PLACEHOLDER, format_ms};
use crate::ui_state::PROFILES;

pub const INFO_DOWNLOAD: &str =
    "How fast data arrives from the server: the 90th percentile of all download requests.";
pub const INFO_UPLOAD: &str =
    "How fast data reaches the server: the 90th percentile of all upload requests.";
pub const INFO_LATENCY: &str =
    "The time a small message takes to reach the server and come back, on an idle network.";
pub const INFO_JITTER: &str = "How much latency varies between measurements.";
pub const INFO_LOSS: &str = "The share of small WebRTC packets that never arrived.";

/// `"10 Oct 2026, 14:02 UTC"` from an RFC 3339 UTC timestamp (the server's
/// rendering; the browser shows local time, see [`format_when`]).
pub fn fallback_when(iso: &str) -> String {
    let b = iso.as_bytes();
    let num = |r: std::ops::Range<usize>| iso.get(r).and_then(|s| s.parse::<u32>().ok());
    match (num(0..4), num(5..7), num(8..10), iso.get(11..16)) {
        (Some(y), Some(m), Some(d), Some(hm)) if b.len() >= 16 => {
            format!("{d} {} {y}, {hm} UTC", month_abbrev(m).unwrap_or("?"))
        }
        _ => iso.to_string(),
    }
}

/// A run's time in the browser's locale and time zone ("10 Oct 2026, 14:02").
pub fn format_when(iso: &str) -> String {
    #[cfg(feature = "hydrate")]
    {
        let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(iso));
        if d.get_time().is_finite() {
            let (day, month, year) = (d.get_date(), d.get_month() + 1, d.get_full_year());
            return format!(
                "{day} {} {year}, {:02}:{:02}",
                month_abbrev(month).unwrap_or("?"),
                d.get_hours(),
                d.get_minutes()
            );
        }
    }
    fallback_when(iso)
}

/// Copies the page URL to the clipboard (`navigator.clipboard.writeText`);
/// false where the Clipboard API is unavailable (e.g. plain HTTP).
pub fn copy_page_url() -> bool {
    #[cfg(feature = "hydrate")]
    {
        use wasm_bindgen::JsCast;
        let w = window();
        let href = w.location().href().unwrap_or_default();
        let clip = js_sys::Reflect::get(&w.navigator(), &"clipboard".into()).ok();
        let write = clip.as_ref().and_then(|c| {
            js_sys::Reflect::get(c, &"writeText".into())
                .ok()?
                .dyn_into::<js_sys::Function>()
                .ok()
        });
        if let (Some(c), Some(f)) = (clip, write) {
            return f.call1(&c, &href.into()).is_ok();
        }
        false
    }
    #[cfg(not(feature = "hydrate"))]
    false
}

/// "Shared network" text for an overlapped run, or `None`.
pub fn shared_text(c: &Concurrency) -> Option<String> {
    // History rows carry only the ratio, saved runs both.
    let overlapped = c.peers_max > 0 && (c.overlap_ms > 0.0 || c.overlap_ratio > 0.0);
    overlapped.then(|| match c.peers_max {
        1 => "Tested while 1 other device was active".to_string(),
        n => format!("Tested while {n} other devices were active"),
    })
}

/// The "Shared network" badge (SPEC §7.11), with its explanation as a title.
#[component]
pub fn SharedBadge(
    #[prop(into)] detail: String,
    /// Show the explanation next to the badge (result screens), not only
    /// as a tooltip (history rows).
    #[prop(optional)]
    long: bool,
) -> impl IntoView {
    view! {
        <span class="shared-badge" class:is-long=long title=detail.clone()>
            <span class="shared-dot" aria-hidden="true"></span>
            "Shared network"
            {if long {
                view! { <span class="shared-detail">{format!(" · {detail}")}</span> }.into_any()
            } else {
                view! { <span class="visually-hidden">{format!(": {detail}")}</span> }.into_any()
            }}
        </span>
    }
}

/// Display name of a profile key.
pub fn profile_label(p: &str) -> String {
    PROFILES
        .iter()
        .find(|(k, _)| *k == p)
        .map_or_else(|| p.to_string(), |(_, v)| v.to_string())
}

pub fn transport_key(t: Transport) -> &'static str {
    match t {
        Transport::Websocket => "websocket",
        Transport::Http => "http",
    }
}

/// Five metric tiles from a saved run.
#[component]
pub fn ResultTiles(#[prop(into)] result: Signal<Option<RunResult>>) -> impl IntoView {
    let pick = move |f: fn(&RunResult) -> Option<f64>| {
        Signal::derive(move || result.with(|r| r.as_ref().and_then(f)))
    };
    let samples = move |f: fn(&RunResult) -> Vec<f64>| {
        Signal::derive(move || result.with(|r| r.as_ref().map(f).unwrap_or_default()))
    };
    let loaded = move |f: fn(&RunResult) -> Option<f64>| {
        Signal::derive(move || {
            result.with(|r| r.as_ref().and_then(f)).map(|m| {
                let (s, u) = format_ms(m);
                format!("Under load {s} {u}")
            })
        })
    };
    let latency_detail = Signal::derive(move || {
        result.with(|r| {
            r.as_ref()
                .map(|r| r.points.latency.len())
                .filter(|n| *n > 0)
                .map(|n| format!("Median of {n} probes"))
        })
    });
    let loss_detail = Signal::derive(move || {
        result.with(|r| {
            r.as_ref().map(|r| match r.points.packet_loss {
                Some(c) => format!("{} of {} lost", c.lost, c.sent),
                None => "Unavailable".into(),
            })
        })
    });
    view! {
        <section class="metric-grid" aria-label="Results">
            <MetricTile
                label="Download"
                info=INFO_DOWNLOAD
                kind=MetricKind::Rate
                value=pick(|r| r.summary.download_bps)
                series=Series::Download
                samples=samples(|r| r.points.download.iter().map(|p| p.bps).collect())
                detail=loaded(|r| r.summary.down_loaded_latency_ms)
                index=0
            />
            <MetricTile
                label="Upload"
                info=INFO_UPLOAD
                kind=MetricKind::Rate
                value=pick(|r| r.summary.upload_bps)
                series=Series::Upload
                samples=samples(|r| r.points.upload.iter().map(|p| p.bps).collect())
                detail=loaded(|r| r.summary.up_loaded_latency_ms)
                index=1
            />
            <MetricTile
                label="Latency"
                info=INFO_LATENCY
                kind=MetricKind::Ms
                value=pick(|r| r.summary.latency_ms)
                series=Series::Latency
                samples=samples(|r| r.points.latency.clone())
                detail=latency_detail
                index=2
            />
            <MetricTile
                label="Jitter"
                info=INFO_JITTER
                kind=MetricKind::Ms
                value=pick(|r| r.summary.jitter_ms)
                index=3
            />
            <MetricTile
                label="Packet loss"
                info=INFO_LOSS
                kind=MetricKind::Percent
                value=pick(|r| r.summary.packet_loss_ratio)
                detail=loss_detail
                index=4
            />
        </section>
    }
}

/// The detail cards of a finished run (latency box plots, packet loss,
/// download and upload dot plots).
#[component]
pub fn ResultDetails(
    #[prop(into)] result: Signal<Option<RunResult>>,
    /// Why packet loss is unavailable, when it is.
    #[prop(into)]
    loss_reason: Signal<String>,
) -> impl IntoView {
    let rows = Signal::derive(move || {
        result.with(|r| {
            r.as_ref()
                .map(|r| {
                    vec![
                        BoxRow {
                            label: "Idle".into(),
                            series: Series::Latency,
                            values: r.points.latency.clone(),
                            jitter_ms: r.summary.jitter_ms,
                        },
                        BoxRow {
                            label: "During download".into(),
                            series: Series::Download,
                            values: r.points.down_loaded_latency.clone(),
                            jitter_ms: r.summary.down_loaded_jitter_ms,
                        },
                        BoxRow {
                            label: "During upload".into(),
                            series: Series::Upload,
                            values: r.points.up_loaded_latency.clone(),
                            jitter_ms: r.summary.up_loaded_jitter_ms,
                        },
                    ]
                })
                .unwrap_or_default()
        })
    });
    let points = move |f: fn(&RunResult) -> Vec<crate::model::result::BandwidthPoint>| {
        Signal::derive(move || result.with(|r| r.as_ref().map(f).unwrap_or_default()))
    };
    let value = move |f: fn(&RunResult) -> Option<f64>| {
        Signal::derive(move || result.with(|r| r.as_ref().and_then(f)))
    };
    let loss =
        Signal::derive(move || result.with(|r| r.as_ref().and_then(|r| r.points.packet_loss)));
    view! {
        <section class="details" aria-labelledby="details-title">
            <h2 id="details-title" class="section-title">"Details"</h2>
            <div class="details-grid">
                <article class="card detail-card reveal" style=reveal_style(8)>
                    <h3 class="card-title">"Latency"</h3>
                    <p class="card-sub">"Round trips while idle and while the link is busy."</p>
                    <BoxPlotChart rows=rows/>
                </article>
                <article class="card detail-card reveal" style=reveal_style(9)>
                    <h3 class="card-title">"Packet loss"</h3>
                    <p class="card-sub">"Small packets sent over WebRTC."</p>
                    <LossBar counts=loss reason=loss_reason/>
                </article>
                <article class="card detail-card reveal" style=reveal_style(10)>
                    <h3 class="card-title">"Download"</h3>
                    <p class="card-sub">"Every request, grouped by payload size."</p>
                    <DotPlot
                        points=points(|r| r.points.download.clone())
                        series=Series::Download
                        p90=value(|r| r.summary.download_bps)
                    />
                </article>
                <article class="card detail-card reveal" style=reveal_style(11)>
                    <h3 class="card-title">"Upload"</h3>
                    <p class="card-sub">"Every request, grouped by payload size."</p>
                    <DotPlot
                        points=points(|r| r.points.upload.clone())
                        series=Series::Upload
                        p90=value(|r| r.summary.upload_bps)
                    />
                </article>
            </div>
        </section>
    }
}

/// Facts about a saved run: device, time, connection, profile, overlap.
#[component]
pub fn RunInfoCard(result: RunResult) -> impl IntoView {
    let row = |k: &'static str, v: String| {
        view! {
            <div>
                <dt>{k}</dt>
                <dd>{v}</dd>
            </div>
        }
    };
    let or_dash = |v: Option<String>| {
        v.filter(|s| !s.is_empty())
            .unwrap_or_else(|| PLACEHOLDER.to_string())
    };
    let c = result.concurrency;
    let overlap = if shared_text(&c).is_some() {
        format!(
            "{} other device{} · {:.0}% of the run",
            c.peers_max,
            if c.peers_max == 1 { "" } else { "s" },
            c.overlap_ratio * 100.0
        )
    } else {
        "None".to_string()
    };
    view! {
        <section class="card server-card" aria-labelledby="run-info-title">
            <h2 id="run-info-title" class="card-title">"This run"</h2>
            <dl class="kv">
                {row("Device", or_dash(result.client.device_name.clone()))}
                {row("When", format_when(&result.created_at))}
                {row("Connection", connection_label(transport_key(result.transport), result.tls))}
                {row(
                    "Profile",
                    format!(
                        "{} · {} stream{}",
                        profile_label(&result.profile),
                        result.streams,
                        if result.streams == 1 { "" } else { "s" },
                    ),
                )}
                {row("Duration", format!("{:.1} s", result.duration_ms as f64 / 1000.0))}
                {row("Client IP", or_dash(result.client.ip.clone()))}
                {row("Other devices testing", overlap)}
            </dl>
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn when_fallback() {
        assert_eq!(
            fallback_when("2026-10-10T14:02:33.123Z"),
            "10 Oct 2026, 14:02 UTC"
        );
        assert_eq!(fallback_when("bogus"), "bogus");
    }

    #[test]
    fn shared_texts() {
        let mut c = Concurrency::default();
        assert_eq!(shared_text(&c), None);
        c.peers_max = 2;
        c.overlap_ratio = 0.5;
        assert_eq!(
            shared_text(&c).as_deref(),
            Some("Tested while 2 other devices were active")
        );
        c.peers_max = 1;
        assert!(shared_text(&c).unwrap().contains("1 other device was"));
    }
}
