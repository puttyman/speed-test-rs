// The test page view is a deep tachys type; release builds need a higher limit.
#![recursion_limit = "256"]

pub mod app;
pub mod components;
#[cfg(any(feature = "hydrate", test))]
pub mod engine;
pub mod model;
pub mod pages;
#[cfg(feature = "ssr")]
pub mod server;
pub mod ui_state;

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    use crate::app::*;
    console_error_panic_hook::set_once();
    leptos::mount::hydrate_body(App);
}
