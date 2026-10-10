//! Spring count-up (SPEC §7.3): numbers tween to each new value in about
//! 400 ms and never jump. Reduced motion shows the new value at once.

use leptos::prelude::*;

#[cfg(feature = "hydrate")]
use crate::ui_state::use_ui_state;

/// Natural frequency of the spring (rad/s); settles to <1% in about 400 ms.
pub const OMEGA: f64 = 16.0;

/// Advances a critically damped spring by `dt` seconds (exact solution, so
/// stable for any step). Returns the new position and velocity.
pub fn spring_step(x: f64, v: f64, target: f64, dt: f64, omega: f64) -> (f64, f64) {
    let d = x - target;
    let e = (-omega * dt).exp();
    let c = v + omega * d;
    let nd = (d + c * dt) * e;
    let nv = (v - omega * c * dt) * e;
    (target + nd, nv)
}

/// True once the spring is close enough to stop animating.
pub fn settled(x: f64, v: f64, target: f64) -> bool {
    let scale = target.abs().max(1e-9);
    (x - target).abs() <= scale * 1e-3 && v.abs() <= scale * 1e-2
}

/// A tweened copy of `target`. `None` (or reduced motion) passes through
/// immediately; numeric changes animate with [`spring_step`].
pub fn use_count_up(target: Signal<Option<f64>>) -> Signal<Option<f64>> {
    let shown = RwSignal::new(target.get_untracked());
    #[cfg(feature = "hydrate")]
    {
        use std::cell::{Cell, RefCell};
        use std::rc::Rc;

        #[derive(Default)]
        struct Anim {
            pos: f64,
            vel: f64,
            target: f64,
            last_ts: Option<f64>,
        }

        let ui = use_ui_state();
        let anim = Rc::new(RefCell::new(Anim::default()));
        let running: Rc<Cell<bool>> = Rc::new(Cell::new(false));
        let alive: Rc<RefCell<Option<Rc<Cell<bool>>>>> = Rc::new(RefCell::new(None));
        let on_drop = send_wrapper::SendWrapper::new(alive.clone());
        on_cleanup(move || {
            if let Some(a) = on_drop.borrow_mut().take() {
                a.set(false);
            }
        });

        Effect::new(move |_| {
            let t = target.get();
            let current = shown.get_untracked();
            let reduce = ui.reduced_motion_untracked();
            let (Some(t), Some(cur)) = (t, current) else {
                // Appearing or clearing: no tween.
                if let Some(a) = alive.borrow_mut().take() {
                    a.set(false);
                }
                running.set(false);
                *anim.borrow_mut() = Anim {
                    pos: t.unwrap_or(0.0),
                    target: t.unwrap_or(0.0),
                    ..Anim::default()
                };
                shown.set(t);
                return;
            };
            if reduce || !t.is_finite() || !cur.is_finite() {
                if let Some(a) = alive.borrow_mut().take() {
                    a.set(false);
                }
                running.set(false);
                *anim.borrow_mut() = Anim {
                    pos: t,
                    target: t,
                    ..Anim::default()
                };
                shown.set(Some(t));
                return;
            }
            {
                let mut a = anim.borrow_mut();
                if !running.get() {
                    a.pos = cur;
                    a.vel = 0.0;
                    a.last_ts = None;
                }
                a.target = t;
            }
            if running.get() {
                return;
            }
            running.set(true);
            let (anim2, running2) = (anim.clone(), running.clone());
            let handle = super::raf_loop(move |ts| {
                let mut a = anim2.borrow_mut();
                let dt = a
                    .last_ts
                    .map_or(1.0 / 60.0, |l| ((ts - l) / 1000.0).clamp(0.0, 0.064));
                a.last_ts = Some(ts);
                let (x, v) = spring_step(a.pos, a.vel, a.target, dt, OMEGA);
                a.pos = x;
                a.vel = v;
                if settled(x, v, a.target) {
                    a.pos = a.target;
                    shown.set(Some(a.target));
                    running2.set(false);
                    return false;
                }
                shown.set(Some(x));
                true
            });
            *alive.borrow_mut() = Some(handle);
        });
    }
    shown.into()
}

/// A tweened value with its unit, rendered as `<span class="value">` and
/// `<span class="unit">`.
#[component]
pub fn CountUp<F>(
    #[prop(into)] value: Signal<Option<f64>>,
    /// Formats the (tweened) value into number and unit text.
    format: F,
) -> impl IntoView
where
    F: Fn(Option<f64>) -> (String, &'static str) + Send + Sync + Copy + 'static,
{
    let shown = use_count_up(value);
    let parts = Memo::new(move |_| format(shown.get()));
    view! {
        <span class="value">{move || parts.with(|p| p.0.clone())}</span>
        <span class="unit">{move || parts.with(|p| p.1)}</span>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_converges_without_overshoot() {
        let (mut x, mut v) = (0.0, 0.0);
        let mut t = 0.0;
        while t < 0.4 {
            (x, v) = spring_step(x, v, 100.0, 1.0 / 60.0, OMEGA);
            assert!(x <= 100.0 + 1e-9, "overshoot at {t}: {x}");
            t += 1.0 / 60.0;
        }
        assert!(x > 98.0, "too slow: {x}");
        for _ in 0..60 {
            (x, v) = spring_step(x, v, 100.0, 1.0 / 60.0, OMEGA);
        }
        assert!(settled(x, v, 100.0));
    }

    #[test]
    fn spring_is_step_size_independent() {
        let (a, _) = spring_step(0.0, 0.0, 10.0, 0.2, OMEGA);
        let (mut b, mut v) = (0.0, 0.0);
        for _ in 0..20 {
            (b, v) = spring_step(b, v, 10.0, 0.01, OMEGA);
        }
        assert!((a - b).abs() < 1e-9);
        // A huge step lands on the target.
        let (c, _) = spring_step(0.0, 0.0, 10.0, 10.0, OMEGA);
        assert!((c - 10.0).abs() < 1e-6);
    }

    #[test]
    fn settle_threshold() {
        assert!(settled(99.95, 0.0, 100.0));
        assert!(!settled(99.0, 0.0, 100.0));
        assert!(!settled(100.0, 50.0, 100.0));
        assert!(settled(0.0, 0.0, 0.0));
    }
}
