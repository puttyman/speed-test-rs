//! The hero (SPEC §7.4): a large count-up number over the packet-flow canvas,
//! the phase timeline, and the page's controls (passed as children).

use leptos::prelude::*;

use super::MetricKind;
use super::motion::{CountUp, FlowMode, OtherFlow, PacketFlow};
use super::phase_timeline::{PhaseTimeline, TimelineStep};
use crate::ui_state::use_ui_state;

#[component]
pub fn Hero(
    /// Eyebrow above the number, e.g. "Download".
    #[prop(into)]
    title: Signal<String>,
    #[prop(into)] value: Signal<Option<f64>>,
    #[prop(into)] kind: Signal<MetricKind>,
    /// One line under the number (current step, or the run summary).
    #[prop(into)]
    caption: Signal<String>,
    #[prop(into)] timeline: Signal<Vec<TimelineStep>>,
    #[prop(into)] flow_mode: Signal<FlowMode>,
    #[prop(into)] flow_bps: Signal<Option<f64>>,
    #[prop(into)] pulses: Signal<u32>,
    /// Muted number while idle.
    #[prop(into)]
    idle: Signal<bool>,
    /// Other devices' traffic for the packet-flow canvas.
    #[prop(optional, into)]
    others: Signal<Vec<OtherFlow>>,
    /// `view-transition-name` of the number (SPEC §7.3).
    #[prop(optional, into)]
    vt_name: Signal<String>,
    children: Children,
) -> impl IntoView {
    let ui = use_ui_state();
    let kind = Memo::new(move |_| kind.get());
    view! {
        <section class="hero" aria-labelledby="hero-title">
            <PacketFlow mode=flow_mode bps=flow_bps pulses=pulses others=others/>
            <div class="hero-inner">
                <h1 id="hero-title" class="hero-title">{move || title.get()}</h1>
                <p
                    class="hero-value"
                    class:is-idle=move || idle.get()
                    // Running, but no value yet (e.g. packet loss in progress).
                    class:is-pending=move || !idle.get() && value.with(Option::is_none)
                    style=move || {
                        let n = vt_name.get();
                        (!n.is_empty()).then(|| format!("view-transition-name:{n}"))
                    }
                >
                    // One count-up per kind, so a unit change never tweens
                    // across units.
                    {move || {
                        let k = kind.get();
                        // Idle shows a calm muted 0, and a running phase without a value
                        // yet shows "···"; never a placeholder dash, which renders as a
                        // solid bar at hero size.
                        let format = move |v: Option<f64>| match v {
                            None if idle.get() => ("0".to_string(), k.format(Some(0.0), ui.units.get()).1),
                            None => ("···".to_string(), String::new()),
                            v => k.format(v, ui.units.get()),
                        };
                        view! { <CountUp value=value format=format/> }
                    }}
                </p>
                <p class="hero-caption">{move || caption.get()}</p>
                <PhaseTimeline steps=timeline/>
                {children()}
            </div>
        </section>
    }
}
