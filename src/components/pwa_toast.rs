//! "Update available · Reload" toast (SPEC §7.9). `public/pwa.js` sets
//! `window.__pwaUpdateAvailable` and dispatches `pwa:update-available` when a
//! new service worker is waiting; Reload calls `window.__pwaApplyUpdate()`.

use leptos::prelude::*;

#[component]
pub fn PwaToast() -> impl IntoView {
    let visible = RwSignal::new(false);

    Effect::new(move |_| {
        if js_flag("__pwaUpdateAvailable") {
            visible.set(true);
        }
    });
    let listener = window_event_listener_untyped("pwa:update-available", move |_| {
        visible.set(true);
    });
    on_cleanup(move || listener.remove());

    let reload = move |_| {
        if !call_js("__pwaApplyUpdate") {
            // Nothing waiting any more: a plain reload picks up the new version.
            let _ = window().location().reload();
        }
    };

    view! {
        <Show when=move || visible.get()>
            <div class="toast" role="status">
                <span>"Update available"</span>
                <button type="button" class="toast-action" on:click=reload>
                    "Reload"
                </button>
                <button
                    type="button"
                    class="toast-dismiss"
                    aria-label="Dismiss"
                    on:click=move |_| visible.set(false)
                >
                    "×"
                </button>
            </div>
        </Show>
    }
}

fn js_flag(name: &str) -> bool {
    js_sys::Reflect::get(&window(), &name.into())
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Calls `window[name]()`; true if it returned `true`.
fn call_js(name: &str) -> bool {
    use wasm_bindgen::JsCast;
    js_sys::Reflect::get(&window(), &name.into())
        .ok()
        .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
        .and_then(|f| f.call0(&window()).ok())
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}
