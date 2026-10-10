//! Metric tile (SPEC §7.4): series-color accent, a count-up value with its
//! unit, an ⓘ explanation and a sparkline of the samples so far.

use leptos::prelude::*;

use super::MetricKind;
use super::charts::{Series, Sparkline};
use super::info_tip::InfoTip;
use super::motion::{CountUp, reveal_style};
use crate::ui_state::use_ui_state;

#[component]
pub fn MetricTile(
    label: &'static str,
    /// One plain sentence explaining the metric.
    info: &'static str,
    kind: MetricKind,
    #[prop(into)] value: Signal<Option<f64>>,
    /// Accent and sparkline color; `None` for a neutral tile.
    #[prop(optional)]
    series: Option<Series>,
    /// Samples for the sparkline (oldest first).
    #[prop(optional, into)]
    samples: Signal<Vec<f64>>,
    /// A secondary line under the value.
    #[prop(optional, into)]
    detail: Signal<Option<String>>,
    /// True while this metric's phase runs.
    #[prop(optional, into)]
    active: Signal<bool>,
    /// Position in the result reveal stagger.
    #[prop(default = 0)]
    index: usize,
) -> impl IntoView {
    let ui = use_ui_state();
    let accent = series.map_or("series-none", |s| s.class());
    let format = move |v: Option<f64>| kind.format(v, ui.units.get());
    view! {
        <article
            class=format!("metric-tile reveal {accent}")
            class:is-active=move || active.get()
            style=reveal_style(index)
            aria-label=label
        >
            <header class="metric-head">
                <span class="metric-dot" aria-hidden="true"></span>
                <h3 class="metric-label">{label}</h3>
                <InfoTip topic=label text=info/>
            </header>
            <p class="metric-value">
                <CountUp value=value format=format/>
            </p>
            <p class="metric-detail">{move || detail.get().unwrap_or_default()}</p>
            {series
                .map(|s| {
                    let from_zero = kind == MetricKind::Rate;
                    view! {
                        <div class="metric-spark">
                            <Sparkline values=samples series=s from_zero=from_zero/>
                        </div>
                    }
                })}
        </article>
    }
}
