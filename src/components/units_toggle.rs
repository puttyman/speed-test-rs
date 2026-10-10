//! Mbps ↔ MB/s (SPEC §7.6). Values auto-scale to G at ≥ 1000; the choice is
//! stored per device by [`crate::ui_state`].

use leptos::prelude::*;

use super::segmented::Segmented;
use crate::model::units::Unit;
use crate::ui_state::{parse_unit, unit_key, use_ui_state};

#[component]
pub fn UnitsToggle(#[prop(optional)] class: &'static str) -> impl IntoView {
    let ui = use_ui_state();
    let options = [Unit::Mbps, Unit::MBps]
        .into_iter()
        .map(|u| (unit_key(u).to_string(), u.label().to_string()))
        .collect();
    view! {
        <Segmented
            options=options
            value=Signal::derive(move || unit_key(ui.units.get()).to_string())
            on_change=Callback::new(move |v: String| {
                if let Some(u) = parse_unit(&v) {
                    ui.units.set(u);
                }
            })
            label="Speed units"
            class=class
        />
    }
}
