//! Settings drawer (SPEC §7.8): transport, profile, parallel streams, units,
//! theme, reduced motion and device name. Escape or the backdrop closes it;
//! focus moves in on open and back to the settings button on close.

use leptos::html;
use leptos::prelude::*;

use super::icons;
use super::segmented::Segmented;
use super::theme_toggle::ThemeSegmented;
use super::units_toggle::UnitsToggle;
use crate::ui_state::{MAX_STREAMS, MotionPref, PROFILES, TRANSPORTS, use_ui_state};

/// Id of the header button that opens the drawer (focus returns there).
pub const SETTINGS_BUTTON_ID: &str = "settings-button";

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[component]
pub fn SettingsButton() -> impl IntoView {
    let ui = use_ui_state();
    view! {
        <button
            type="button"
            id=SETTINGS_BUTTON_ID
            class="icon-button"
            aria-label="Settings"
            title="Settings"
            aria-haspopup="dialog"
            aria-expanded=move || if ui.settings_open.get() { "true" } else { "false" }
            aria-controls="settings-drawer"
            on:click=move |_| ui.settings_open.set(true)
        >
            {icons::settings()}
        </button>
    }
}

#[component]
pub fn SettingsDrawer(
    /// True while a test runs: test settings can't change then.
    #[prop(into)]
    locked: Signal<bool>,
    /// The server's stream limit.
    #[prop(into)]
    max_streams: Signal<u8>,
) -> impl IntoView {
    let ui = use_ui_state();
    let open = ui.settings_open;
    let close_ref = NodeRef::<html::Button>::new();

    // Focus in on open, back to the button on close.
    Effect::new(move |was_open: Option<bool>| {
        let is_open = open.get();
        if is_open {
            if let Some(b) = close_ref.get() {
                let _ = b.focus();
            }
        } else if was_open == Some(true)
            && let Some(el) = document().get_element_by_id(SETTINGS_BUTTON_ID)
        {
            use wasm_bindgen::JsCast;
            if let Ok(el) = el.dyn_into::<web_sys::HtmlElement>() {
                let _ = el.focus();
            }
        }
        is_open
    });

    let on_key = move |ev: leptos::ev::KeyboardEvent| {
        if ev.key() == "Escape" {
            ev.prevent_default();
            open.set(false);
        }
    };

    let streams_options: Signal<Vec<(String, String)>> = Signal::derive(move || {
        let max = max_streams.get().clamp(1, MAX_STREAMS);
        (1..=max).map(|n| (n.to_string(), n.to_string())).collect()
    });
    let motion_options = MotionPref::ALL
        .into_iter()
        .map(|m| (m.as_str().to_string(), m.label().to_string()))
        .collect::<Vec<_>>();

    view! {
        <div
            class="drawer-backdrop"
            class:is-open=move || open.get()
            on:click=move |_| open.set(false)
            aria-hidden="true"
        ></div>
        <aside
            id="settings-drawer"
            class="drawer"
            class:is-open=move || open.get()
            role="dialog"
            aria-modal="true"
            aria-labelledby="settings-title"
            inert=move || !open.get()
            on:keydown=on_key
        >
            <header class="drawer-head">
                <h2 id="settings-title" class="drawer-title">"Settings"</h2>
                <button
                    type="button"
                    class="icon-button"
                    aria-label="Close settings"
                    node_ref=close_ref
                    on:click=move |_| open.set(false)
                >
                    {icons::close()}
                </button>
            </header>

            <div class="drawer-body">
                <section class="setting-group" aria-labelledby="set-test">
                    <h3 id="set-test" class="setting-group-title">"Test"</h3>
                    <div class="setting">
                        <span class="setting-label">"Transport"</span>
                        <Segmented
                            options=pairs(&TRANSPORTS)
                            value=ui.transport
                            on_change=Callback::new(move |v| ui.transport.set(v))
                            label="Transport"
                            disabled=locked
                        />
                    </div>
                    <div class="setting">
                        <span class="setting-label">"Profile"</span>
                        <Segmented
                            options=pairs(&PROFILES)
                            value=ui.profile
                            on_change=Callback::new(move |v| ui.profile.set(v))
                            label="Profile"
                            disabled=locked
                        />
                    </div>
                    <div class="setting">
                        <span class="setting-label">"Parallel streams"</span>
                        {move || {
                            view! {
                                <Segmented
                                    options=streams_options.get()
                                    value=Signal::derive(move || ui.streams.get().to_string())
                                    on_change=Callback::new(move |v: String| {
                                        if let Ok(n) = v.parse::<u8>() {
                                            ui.streams.set(n.clamp(1, MAX_STREAMS));
                                        }
                                    })
                                    label="Parallel streams"
                                    disabled=locked
                                    class="segmented-dense"
                                />
                            }
                        }}
                    </div>
                    <p class="setting-note">
                        {move || {
                            if locked.get() {
                                "Test settings apply to the next run."
                            } else {
                                "Quick takes about 5 s. Extended adds 4 streams for the largest payloads."
                            }
                        }}
                    </p>
                </section>

                <section class="setting-group" aria-labelledby="set-display">
                    <h3 id="set-display" class="setting-group-title">"Display"</h3>
                    <div class="setting">
                        <span class="setting-label">"Units"</span>
                        <UnitsToggle/>
                    </div>
                    <div class="setting">
                        <span class="setting-label">"Theme"</span>
                        <ThemeSegmented/>
                    </div>
                    <div class="setting">
                        <span class="setting-label">"Reduce motion"</span>
                        <Segmented
                            options=motion_options
                            value=Signal::derive(move || ui.motion.get().as_str().to_string())
                            on_change=Callback::new(move |v: String| {
                                if let Some(m) = MotionPref::parse(&v) {
                                    ui.motion.set(m);
                                }
                            })
                            label="Reduce motion"
                        />
                    </div>
                </section>

                <section class="setting-group" aria-labelledby="set-device">
                    <h3 id="set-device" class="setting-group-title">"This device"</h3>
                    <label class="setting">
                        <span class="setting-label">"Device name"</span>
                        <input
                            class="text-input"
                            type="text"
                            maxlength="64"
                            autocomplete="off"
                            prop:value=move || ui.device_name.get()
                            on:change=move |ev| {
                                let v = event_target_value(&ev);
                                if !v.trim().is_empty() {
                                    ui.device_name.set(v.trim().to_string());
                                }
                            }
                        />
                    </label>
                    <p class="setting-note">"Shown in history and used for default labels."</p>
                    <label class="switch setting-switch">
                        <input
                            type="checkbox"
                            role="switch"
                            id="hide-me"
                            prop:checked=move || ui.hide_me.get()
                            on:change=move |ev| ui.hide_me.set(event_target_checked(&ev))
                        />
                        <span class="switch-track" aria-hidden="true"></span>
                        <span class="switch-text">"Hide me"</span>
                    </label>
                    <p class="setting-note">
                        "Other devices see “1 hidden device testing” instead of this device's name and label."
                    </p>
                </section>
            </div>
        </aside>
    }
}
