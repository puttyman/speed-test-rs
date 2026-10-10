//! Shared model: types and pure logic used by both the server (`ssr`) and the
//! browser engine (`hydrate`).
//!
//! Nothing here performs I/O or depends on a platform, so every function is
//! deterministic and unit-tested natively.
//!
//! - [`protocol`]: WebSocket messages (SPEC §6.1, §5.11)
//! - [`profile`]: test profiles, engine config and the ramp planner (§5.3)
//! - [`result`]: run and group results plus the measurement reducers (§6.3)
//! - [`stats`]: percentile, median, jitter, box plot, Jain's fairness (§5.10)
//! - [`scores`]: Cloudflare AIM scoring (§5.9)
//! - [`units`]: rate, latency and percentage formatting (§7.6)
//! - [`device`]: device detection from the User-Agent and default labels (§7.7)

pub mod device;
pub mod profile;
pub mod protocol;
pub mod result;
pub mod scores;
pub mod stats;
pub mod units;
