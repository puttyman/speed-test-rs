//! Theme controls: a compact header button that cycles System → Light →
//! Dark, and a segmented control for the settings drawer. The theme is
//! applied as `data-theme` on `<html>` by [`crate::ui_state`].

use leptos::prelude::*;

use super::icons;
use super::segmented::Segmented;
use crate::ui_state::{Theme, use_ui_state};

#[component]
pub fn ThemeToggle() -> impl IntoView {
    let ui = use_ui_state();
    let label = move || {
        let t = ui.theme.get();
        format!(
            "Theme: {}. Switch to {}",
            t.label(),
            t.next().label().to_lowercase()
        )
    };
    view! {
        <button
            type="button"
            class="icon-button theme-toggle"
            aria-label=label
            title=label
            on:click=move |_| ui.theme.update(|t| *t = t.next())
        >
            {move || match ui.theme.get() {
                Theme::System => icons::monitor().into_any(),
                Theme::Light => icons::sun().into_any(),
                Theme::Dark => icons::moon().into_any(),
            }}
        </button>
    }
}

#[component]
pub fn ThemeSegmented() -> impl IntoView {
    let ui = use_ui_state();
    let options = Theme::ALL
        .into_iter()
        .map(|t| (t.as_str().to_string(), t.label().to_string()))
        .collect();
    view! {
        <Segmented
            options=options
            value=Signal::derive(move || ui.theme.get().as_str().to_string())
            on_change=Callback::new(move |v: String| {
                if let Some(t) = Theme::parse(&v) {
                    ui.theme.set(t);
                }
            })
            label="Theme"
        />
    }
}
