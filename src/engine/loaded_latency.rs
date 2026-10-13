//! Side tasks that run alongside a transfer step: loaded-latency probes
//! (SPEC §5.6) and the live throughput meter.

use std::cell::{Cell, RefCell};

use super::EngineEvent;
use super::js::{now, sleep};
use super::runner::Link;
use super::util::{LIVE_INTERVAL_MS, LiveMeter};
use crate::model::profile::Direction;
use crate::model::result::Measurements;

/// Every `throttle_ms`, while a request is running (`request_start` is not
/// NaN), sends one probe and records its RTT with the request's elapsed time
/// at send. Returns when the probe channel fails.
pub async fn probe_loop(
    link: &Link,
    direction: Direction,
    throttle_ms: u32,
    request_start: &Cell<f64>,
    measurements: &RefCell<Measurements>,
    emit: &dyn Fn(EngineEvent),
) {
    loop {
        sleep(throttle_ms).await;
        let started = request_start.get();
        if started.is_nan() {
            continue;
        }
        let elapsed = now() - started;
        match link.ping().await {
            Ok(ms) => {
                measurements
                    .borrow_mut()
                    .push_loaded_latency(direction, ms, elapsed);
                emit(EngineEvent::LoadedLatency { direction, ms });
            }
            Err(_) => return,
        }
    }
}

/// Emits [`EngineEvent::Live`] every [`LIVE_INTERVAL_MS`] from the link's
/// cumulative byte counter, and records each reading (time since `t0`) in
/// `measurements` for the throughput series and the timed peak/average.
/// Never returns.
pub async fn live_loop(
    link: &Link,
    direction: Direction,
    t0: f64,
    measurements: &RefCell<Measurements>,
    emit: &dyn Fn(EngineEvent),
) {
    let mut meter = LiveMeter::new();
    loop {
        let (t, bytes) = (now(), link.progress(direction));
        measurements
            .borrow_mut()
            .push_progress(direction, t - t0, bytes);
        if let Some(bps) = meter.push(t, bytes) {
            emit(EngineEvent::Live { direction, bps });
        }
        sleep(LIVE_INTERVAL_MS).await;
    }
}
