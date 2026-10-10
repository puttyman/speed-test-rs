//! Server-only code: configuration, listeners, assets, HTTP/WebSocket measurement
//! endpoints and (later) storage. See `docs/SPEC.md` for the full protocol.

pub mod api;
pub mod assets;
pub mod config;
pub mod groups;
pub mod healthcheck;
pub mod http_measure;
pub mod listen;
pub mod presence;
pub mod router;
pub mod rtc;
pub mod rtc_hook;
pub mod runs_api;
pub mod sessions;
pub mod startup;
pub mod state;
pub mod storage;
pub mod ws;

pub use config::Config;
pub use router::router;
pub use state::AppState;
