//! Shared Axum state.

use std::sync::Arc;

use axum::extract::FromRef;
use leptos::config::{Env, LeptosOptions, ReloadWSProtocol};

use super::assets::Assets;
use super::config::{Config, process_env};
use super::storage::Storage;
use super::ws::MeasureState;

/// State shared by every handler (`State<AppState>`, or a sub-state through [`FromRef`]).
///
/// Extension point: later modules add their shared pieces here as cheap-to-clone fields
/// (e.g. the session registry, the SQLite pool, presence), construct them in
/// [`AppState::new`] (or let `main` fill them in), and add a `FromRef` impl if handlers
/// should be able to extract them directly.
#[derive(Clone)]
pub struct AppState {
    /// Leptos options (output name, pkg dir, ...). Needed by `leptos_axum` routing.
    pub leptos: LeptosOptions,
    /// The validated configuration.
    pub config: Arc<Config>,
    /// Where UI assets are served from (embedded or disk).
    pub assets: Assets,
    /// Measurement runtime: session registry, payload buffer, limits, WebRTC hook.
    pub measure: MeasureState,
    /// Run history; `None` when history is off (`--no-history`). Opened asynchronously by
    /// `main` through [`Storage::from_config`](super::storage::Storage::from_config).
    pub storage: Option<Storage>,
    // Future: `pub sessions: SessionRegistry`, `pub db: Option<SqlitePool>`, ...
}

impl AppState {
    pub fn new(config: Config) -> Self {
        let leptos = leptos_options(&config, process_env);
        let assets = Assets::from_config(&config);
        let measure = MeasureState::new(&config);
        Self {
            leptos,
            config: Arc::new(config),
            assets,
            measure,
            storage: None,
        }
    }
}

impl FromRef<AppState> for LeptosOptions {
    fn from_ref(state: &AppState) -> Self {
        state.leptos.clone()
    }
}

impl FromRef<AppState> for Arc<Config> {
    fn from_ref(state: &AppState) -> Self {
        state.config.clone()
    }
}

/// Builds [`LeptosOptions`] from the configuration, so the binary needs no `LEPTOS_*`
/// variables. The `LEPTOS_*` variables that `cargo leptos watch` sets are still honoured as
/// fallbacks, which keeps dev hot reload working.
pub fn leptos_options(config: &Config, env: impl Fn(&str) -> Option<String>) -> LeptosOptions {
    let output_name = env("LEPTOS_OUTPUT_NAME").unwrap_or_else(|| "speed-test-rs".into());
    let site_pkg_dir = env("LEPTOS_SITE_PKG_DIR").unwrap_or_else(|| "pkg".into());
    let site_root = config
        .site_root
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "embedded".into());
    let leptos_env = match env("LEPTOS_ENV") {
        Some(value) => value.parse().unwrap_or(Env::PROD),
        None => Env::PROD,
    };
    let reload_port = env("LEPTOS_RELOAD_PORT")
        .and_then(|v| v.parse().ok())
        .unwrap_or(3001);
    let reload_external_port = env("LEPTOS_RELOAD_EXTERNAL_PORT").and_then(|v| v.parse().ok());
    let reload_ws_protocol = match env("LEPTOS_RELOAD_WS_PROTOCOL").as_deref() {
        Some("wss" | "WSS") => ReloadWSProtocol::WSS,
        _ => ReloadWSProtocol::WS,
    };
    let hash_files = env("LEPTOS_HASH_FILES").is_some_and(|v| v == "true");
    let hash_file = env("LEPTOS_HASH_FILE_NAME").unwrap_or_else(|| "hash.txt".into());

    LeptosOptions::builder()
        .output_name(output_name)
        .site_root(site_root)
        .site_pkg_dir(site_pkg_dir)
        .env(leptos_env)
        .site_addr(config.listen[0])
        .reload_port(reload_port)
        .reload_external_port(reload_external_port)
        .reload_ws_protocol(reload_ws_protocol)
        .hash_files(hash_files)
        .hash_file(Arc::from(hash_file))
        .build()
}
