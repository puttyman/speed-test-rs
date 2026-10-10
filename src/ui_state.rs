//! App-wide UI state (SPEC §7.4, §7.6, §7.8): units, theme, reduced motion,
//! device identity and test settings.
//!
//! Every value is persisted per browser in `localStorage`. Server rendering
//! and the first client render use the defaults, so hydration always matches;
//! the stored values are applied by an effect right after hydration. Storage
//! errors (private mode, blocked site data) are ignored: the defaults stay.

use leptos::prelude::*;

use crate::model::units::Unit;

/// `localStorage` keys. `speedtest.device_id` is shared with the engine.
pub mod keys {
    pub const UNITS: &str = "speedtest.units";
    pub const THEME: &str = "speedtest.theme";
    pub const MOTION: &str = "speedtest.motion";
    pub const DEVICE_ID: &str = "speedtest.device_id";
    pub const DEVICE_NAME: &str = "speedtest.device_name";
    pub const LAST_LABEL: &str = "speedtest.last_label";
    pub const TRANSPORT: &str = "speedtest.transport";
    pub const PROFILE: &str = "speedtest.profile";
    pub const STREAMS: &str = "speedtest.streams";
}

/// Color theme preference.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Theme {
    /// Follow `prefers-color-scheme`.
    #[default]
    System,
    Light,
    Dark,
}

impl Theme {
    pub const ALL: [Theme; 3] = [Theme::System, Theme::Light, Theme::Dark];

    pub fn as_str(self) -> &'static str {
        match self {
            Theme::System => "system",
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            Theme::System => "System",
            Theme::Light => "Light",
            Theme::Dark => "Dark",
        }
    }

    /// The next theme for the header's cycling button.
    pub fn next(self) -> Self {
        match self {
            Theme::System => Theme::Light,
            Theme::Light => Theme::Dark,
            Theme::Dark => Theme::System,
        }
    }
}

/// Reduced-motion preference: follow the OS, or force it on/off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MotionPref {
    #[default]
    System,
    Reduce,
    Full,
}

impl MotionPref {
    pub const ALL: [MotionPref; 3] = [MotionPref::System, MotionPref::Reduce, MotionPref::Full];

    pub fn as_str(self) -> &'static str {
        match self {
            MotionPref::System => "system",
            MotionPref::Reduce => "reduce",
            MotionPref::Full => "full",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            MotionPref::System => "System",
            MotionPref::Reduce => "Reduce",
            MotionPref::Full => "Full",
        }
    }
}

pub fn unit_key(unit: Unit) -> &'static str {
    match unit {
        Unit::Mbps => "mbps",
        Unit::MBps => "mbytes",
    }
}

pub fn parse_unit(s: &str) -> Option<Unit> {
    match s {
        "mbps" => Some(Unit::Mbps),
        "mbytes" => Some(Unit::MBps),
        _ => None,
    }
}

/// Transport names used by the settings (`"websocket"` / `"http"`).
pub const TRANSPORTS: [(&str, &str); 2] = [("websocket", "WebSocket"), ("http", "HTTP")];
/// Profiles offered in the settings.
pub const PROFILES: [(&str, &str); 3] = [
    ("quick", "Quick"),
    ("standard", "Standard"),
    ("extended", "Extended"),
];
pub const MAX_STREAMS: u8 = 8;

/// Shared UI state, provided once by `App` as context. Cheap to copy.
#[derive(Clone, Copy)]
pub struct UiState {
    pub units: RwSignal<Unit>,
    pub theme: RwSignal<Theme>,
    pub motion: RwSignal<MotionPref>,
    /// `prefers-reduced-motion: reduce` as reported by the browser.
    pub system_reduced_motion: RwSignal<bool>,
    /// Random per-browser ID (empty until loaded on the client).
    pub device_id: RwSignal<String>,
    /// Friendly device name (user-chosen or detected).
    pub device_name: RwSignal<String>,
    pub transport: RwSignal<String>,
    pub profile: RwSignal<String>,
    pub streams: RwSignal<u8>,
    /// True once the stored values have been applied (client only).
    pub loaded: RwSignal<bool>,
    pub settings_open: RwSignal<bool>,
    /// True while a test runs (locks the test settings).
    pub running: RwSignal<bool>,
    /// The server's stream limit (from `/api/info`).
    pub max_streams: RwSignal<u8>,
}

impl UiState {
    fn new() -> Self {
        Self {
            units: RwSignal::new(Unit::Mbps),
            theme: RwSignal::new(Theme::System),
            motion: RwSignal::new(MotionPref::System),
            system_reduced_motion: RwSignal::new(false),
            device_id: RwSignal::new(String::new()),
            device_name: RwSignal::new(String::new()),
            transport: RwSignal::new("websocket".into()),
            profile: RwSignal::new("standard".into()),
            streams: RwSignal::new(1),
            loaded: RwSignal::new(false),
            settings_open: RwSignal::new(false),
            running: RwSignal::new(false),
            max_streams: RwSignal::new(MAX_STREAMS),
        }
    }

    /// Whether animations should be replaced by static states.
    pub fn reduced_motion(&self) -> bool {
        match self.motion.get() {
            MotionPref::System => self.system_reduced_motion.get(),
            MotionPref::Reduce => true,
            MotionPref::Full => false,
        }
    }

    pub fn reduced_motion_untracked(&self) -> bool {
        match self.motion.get_untracked() {
            MotionPref::System => self.system_reduced_motion.get_untracked(),
            MotionPref::Reduce => true,
            MotionPref::Full => false,
        }
    }
}

/// Creates the UI state, provides it as context and wires persistence.
pub fn provide_ui_state() -> UiState {
    let ui = UiState::new();
    provide_context(ui);

    // Load (client only: effects don't run during SSR).
    Effect::new(move |_| {
        if ui.loaded.get_untracked() {
            return;
        }
        load(ui);
        ui.loaded.set(true);
    });

    // Persist and apply. Each effect skips its first run before loading.
    Effect::new(move |_| {
        let u = ui.units.get();
        if ui.loaded.get() {
            storage_set(keys::UNITS, unit_key(u));
        }
    });
    Effect::new(move |_| {
        let t = ui.theme.get();
        if ui.loaded.get() {
            storage_set(keys::THEME, t.as_str());
            apply_theme(t);
        }
    });
    Effect::new(move |_| {
        let m = ui.motion.get();
        if ui.loaded.get() {
            storage_set(keys::MOTION, m.as_str());
        }
        // `System` leaves it to the `prefers-reduced-motion` media query.
        set_root_attr(
            "data-motion",
            (m != MotionPref::System).then_some(m.as_str()),
        );
    });
    Effect::new(move |_| {
        let name = ui.device_name.get();
        if ui.loaded.get() && !name.trim().is_empty() {
            storage_set(keys::DEVICE_NAME, name.trim());
        }
    });
    Effect::new(move |_| {
        let (t, p, s) = (ui.transport.get(), ui.profile.get(), ui.streams.get());
        if ui.loaded.get() {
            storage_set(keys::TRANSPORT, &t);
            storage_set(keys::PROFILE, &p);
            storage_set(keys::STREAMS, &s.to_string());
        }
    });
    ui
}

/// The UI state provided by `App`.
pub fn use_ui_state() -> UiState {
    expect_context::<UiState>()
}

fn load(ui: UiState) {
    if let Some(u) = storage_get(keys::UNITS).and_then(|s| parse_unit(&s)) {
        ui.units.set(u);
    }
    if let Some(t) = storage_get(keys::THEME).and_then(|s| Theme::parse(&s)) {
        ui.theme.set(t);
    }
    if let Some(m) = storage_get(keys::MOTION).and_then(|s| MotionPref::parse(&s)) {
        ui.motion.set(m);
    }
    if let Some(t) = storage_get(keys::TRANSPORT).filter(|t| TRANSPORTS.iter().any(|(k, _)| k == t))
    {
        ui.transport.set(t);
    }
    if let Some(p) = storage_get(keys::PROFILE).filter(|p| PROFILES.iter().any(|(k, _)| k == p)) {
        ui.profile.set(p);
    }
    if let Some(s) = storage_get(keys::STREAMS)
        .and_then(|s| s.parse::<u8>().ok())
        .filter(|s| (1..=MAX_STREAMS).contains(s))
    {
        ui.streams.set(s);
    }
    browser::init(ui);
}

/// Reads a `localStorage` item; `None` on the server or when storage fails.
pub fn storage_get(key: &str) -> Option<String> {
    browser::storage_get(key)
}

/// Writes a `localStorage` item, ignoring failures.
pub fn storage_set(key: &str, value: &str) {
    browser::storage_set(key, value);
}

fn apply_theme(theme: Theme) {
    set_root_attr(
        "data-theme",
        match theme {
            Theme::System => None,
            t => Some(t.as_str()),
        },
    );
}

fn set_root_attr(name: &str, value: Option<&str>) {
    browser::set_root_attr(name, value);
}

/// Inline script for `<head>`: applies the stored theme and motion preference
/// before first paint, so a dark-theme reload doesn't flash white.
pub const THEME_BOOT_SCRIPT: &str = r#"try{var d=document.documentElement,t=localStorage.getItem("speedtest.theme");if(t==="light"||t==="dark")d.setAttribute("data-theme",t);var m=localStorage.getItem("speedtest.motion");if(m==="reduce"||m==="full")d.setAttribute("data-motion",m)}catch(e){}"#;

#[cfg(feature = "hydrate")]
mod browser {
    use super::*;
    use crate::model::device::device_kind_and_name_from_user_agent;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::Closure;

    fn storage() -> Option<web_sys::Storage> {
        web_sys::window()?.local_storage().ok().flatten()
    }

    pub fn storage_get(key: &str) -> Option<String> {
        storage()?.get_item(key).ok().flatten()
    }

    pub fn storage_set(key: &str, value: &str) {
        if let Some(s) = storage() {
            let _ = s.set_item(key, value);
        }
    }

    pub fn set_root_attr(name: &str, value: Option<&str>) {
        let Some(root) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.document_element())
        else {
            return;
        };
        let _ = match value {
            Some(v) => root.set_attribute(name, v),
            None => root.remove_attribute(name),
        };
    }

    /// Device name from User-Agent Client Hints, falling back to the UA.
    pub fn detect_device_name() -> String {
        let Some(window) = web_sys::window() else {
            return "Device".into();
        };
        let nav = window.navigator();
        let ua = nav.user_agent().unwrap_or_default();
        let (_, fallback) = device_kind_and_name_from_user_agent(&ua);
        let hints = js_sys::Reflect::get(&nav, &"userAgentData".into()).ok();
        let hinted = hints.filter(|h| h.is_object()).and_then(|h| {
            let platform = js_sys::Reflect::get(&h, &"platform".into())
                .ok()?
                .as_string()?;
            let mobile = js_sys::Reflect::get(&h, &"mobile".into())
                .ok()
                .and_then(|m| m.as_bool())
                .unwrap_or(false);
            Some(match (platform.as_str(), mobile) {
                ("Android", true) => "Android phone",
                ("Android", false) => "Android tablet",
                ("macOS", _) => "Mac",
                ("Windows", _) => "Windows PC",
                ("Chrome OS" | "ChromeOS", _) => "Chromebook",
                ("Linux", _) => "Linux PC",
                ("iOS", _) => "iPhone",
                _ => return None,
            })
        });
        hinted.map(str::to_string).unwrap_or(fallback)
    }

    pub fn init(ui: UiState) {
        // Device identity (the engine reads the same id key).
        let id = storage_get(keys::DEVICE_ID)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                let id = uuid::Uuid::new_v4().to_string();
                storage_set(keys::DEVICE_ID, &id);
                id
            });
        ui.device_id.set(id);
        let name = storage_get(keys::DEVICE_NAME)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(detect_device_name);
        ui.device_name.set(name);

        // System reduced-motion preference, live.
        if let Some(mq) = web_sys::window()
            .and_then(|w| w.match_media("(prefers-reduced-motion: reduce)").ok())
            .flatten()
        {
            ui.system_reduced_motion.set(mq.matches());
            let cb = Closure::<dyn Fn(web_sys::Event)>::new(move |ev: web_sys::Event| {
                if let Some(mq) = ev.dyn_ref::<web_sys::MediaQueryList>() {
                    ui.system_reduced_motion.set(mq.matches());
                }
            });
            let _ = mq.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
            cb.forget();
        }
    }
}

#[cfg(not(feature = "hydrate"))]
mod browser {
    use super::UiState;

    pub fn storage_get(_key: &str) -> Option<String> {
        None
    }

    pub fn storage_set(_key: &str, _value: &str) {}

    pub fn set_root_attr(_name: &str, _value: Option<&str>) {}

    pub fn init(_ui: UiState) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        for t in Theme::ALL {
            assert_eq!(Theme::parse(t.as_str()), Some(t));
        }
        for m in MotionPref::ALL {
            assert_eq!(MotionPref::parse(m.as_str()), Some(m));
        }
        for u in [Unit::Mbps, Unit::MBps] {
            assert_eq!(parse_unit(unit_key(u)), Some(u));
        }
        assert_eq!(Theme::parse("nope"), None);
        assert_eq!(Theme::Dark.next(), Theme::System);
    }
}
