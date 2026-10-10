//! Motion (SPEC §7.3): count-up tweens, the packet-flow canvas and the result
//! reveal. Animation only ever *reads* signals fed by the engine; nothing here
//! is awaited by measurement code.

pub mod count_up;
pub mod packet_flow;
pub mod reveal;

pub use count_up::{CountUp, use_count_up};
pub use packet_flow::{FlowMode, OtherFlow, PacketFlow};
pub use reveal::reveal_style;

/// A `requestAnimationFrame` loop that runs `frame(timestamp_ms)` until it
/// returns `false` or the returned flag is set to `false`.
#[cfg(feature = "hydrate")]
pub fn raf_loop(frame: impl FnMut(f64) -> bool + 'static) -> std::rc::Rc<std::cell::Cell<bool>> {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::Closure;

    type Slot = Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>>;
    let alive = Rc::new(Cell::new(true));
    let slot: Slot = Rc::new(RefCell::new(None));
    let frame = Rc::new(RefCell::new(frame));
    let (alive2, slot2) = (alive.clone(), slot.clone());
    *slot.borrow_mut() = Some(Closure::new(move |ts: f64| {
        let keep = alive2.get() && (frame.borrow_mut())(ts);
        if keep {
            if let (Some(w), Some(cb)) = (web_sys::window(), slot2.borrow().as_ref()) {
                let _ = w.request_animation_frame(cb.as_ref().unchecked_ref());
            }
        } else {
            alive2.set(false);
            // Break the Rc cycle once the closure has returned.
            let slot3 = slot2.clone();
            wasm_bindgen_futures::spawn_local(async move {
                slot3.borrow_mut().take();
            });
        }
    }));
    if let (Some(w), Some(cb)) = (web_sys::window(), slot.borrow().as_ref()) {
        let _ = w.request_animation_frame(cb.as_ref().unchecked_ref());
    }
    alive
}
