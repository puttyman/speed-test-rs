//! Server-only code: configuration, listeners, assets, HTTP/WebSocket measurement
//! endpoints and (later) storage. See `docs/SPEC.md` for the full protocol.

pub mod api;
pub mod assets;
pub mod config;
pub mod healthcheck;
pub mod listen;
pub mod router;
pub mod startup;
pub mod state;

pub use config::Config;
pub use router::router;
pub use state::AppState;
