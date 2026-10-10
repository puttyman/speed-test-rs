//! The packet-flow canvas behind the hero (SPEC §7.3).
//!
//! Particles stream down during download and up during upload, tinted with
//! the series color; speed and density follow live throughput on a log
//! scale. Latency probes send single pulses out and back. Idle is a slow
//! drift. The particle count is capped by area, device pixel ratio and CPU
//! cores; if frames take over 20 ms for a second the canvas switches to a
//! lightweight mode. The loop stops while the tab is hidden, and reduced
//! motion replaces it with a static gradient.

use leptos::html;
use leptos::prelude::*;

/// What the canvas shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FlowMode {
    #[default]
    Idle,
    Latency,
    Download,
    Upload,
}

/// 0.0 at ≤ 1 Mbps to 1.0 at ≥ 10 Gbps, logarithmic.
pub fn flow_intensity(bps: f64) -> f64 {
    if !(bps.is_finite() && bps > 1e6) {
        return 0.0;
    }
    ((bps / 1e6).log10() / 4.0).clamp(0.0, 1.0)
}

/// Particle speed in CSS px/s for an intensity.
pub fn flow_speed(intensity: f64) -> f64 {
    90.0 + 520.0 * intensity
}

/// Share of the particle cap in use for a mode and intensity.
pub fn flow_density(mode: FlowMode, intensity: f64) -> f64 {
    match mode {
        FlowMode::Idle => 0.22,
        FlowMode::Latency => 0.16,
        FlowMode::Download | FlowMode::Upload => 0.3 + 0.7 * intensity,
    }
}

/// Maximum particles for a canvas of `w`×`h` CSS px.
pub fn particle_cap(w: f64, h: f64, dpr: f64, cores: u32) -> usize {
    let by_area = (w * h / 2400.0).clamp(30.0, 240.0);
    let cpu = match cores {
        0..=2 => 0.4,
        3..=4 => 0.65,
        5..=7 => 0.85,
        _ => 1.0,
    };
    let pixels = if dpr > 2.0 {
        0.7
    } else if dpr > 1.0 {
        0.85
    } else {
        1.0
    };
    (by_area * cpu * pixels).round().max(16.0) as usize
}

/// Frame-time watchdog: true once frames have been slower than 20 ms for a
/// cumulative second without a fast frame in between.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Watchdog {
    slow_ms: f64,
    pub tripped: bool,
}

impl Watchdog {
    pub fn frame(&mut self, dt_ms: f64) -> bool {
        if dt_ms > 20.0 && dt_ms < 250.0 {
            self.slow_ms += dt_ms;
        } else if dt_ms <= 20.0 {
            self.slow_ms = (self.slow_ms - dt_ms * 2.0).max(0.0);
        }
        if self.slow_ms >= 1000.0 {
            self.tripped = true;
        }
        self.tripped
    }
}

#[component]
pub fn PacketFlow(
    #[prop(into)] mode: Signal<FlowMode>,
    /// Live throughput in bps.
    #[prop(into)]
    bps: Signal<Option<f64>>,
    /// Increments once per latency probe.
    #[prop(into)]
    pulses: Signal<u32>,
) -> impl IntoView {
    let canvas = NodeRef::<html::Canvas>::new();
    let ui = crate::ui_state::use_ui_state();
    let reduced = Memo::new(move |_| ui.reduced_motion());

    #[cfg(feature = "hydrate")]
    imp::run(canvas, reduced, mode, bps, pulses);
    #[cfg(not(feature = "hydrate"))]
    let _ = (mode, bps, pulses);

    view! {
        <div class="packet-flow" class:is-static=move || reduced.get() aria-hidden="true">
            <div class="flow-static"></div>
            <canvas node_ref=canvas class="flow-canvas"></canvas>
        </div>
    }
}

#[cfg(feature = "hydrate")]
mod imp {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use leptos::html;
    use send_wrapper::SendWrapper;
    use wasm_bindgen::JsCast;

    use super::*;
    use crate::components::motion::raf_loop;

    #[derive(Clone, Copy)]
    struct Particle {
        x: f64,
        y: f64,
        vx: f64,
        vy: f64,
        len: f64,
        r: f64,
        alpha: f64,
        kind: FlowMode,
        /// Fades out when the mode changed.
        dying: bool,
    }

    struct Pulse {
        t0: f64,
        x: f64,
    }

    #[derive(Default)]
    struct Colors {
        download: String,
        upload: String,
        latency: String,
        idle: String,
    }

    struct Sim {
        particles: Vec<Particle>,
        pulses: Vec<Pulse>,
        seen_pulses: u32,
        rng: u32,
        last_ts: Option<f64>,
        colors: Colors,
        colors_at: f64,
        watchdog: Watchdog,
        w: f64,
        h: f64,
        dpr: f64,
    }

    impl Sim {
        fn rand(&mut self) -> f64 {
            // xorshift32
            let mut x = self.rng;
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            self.rng = x;
            f64::from(x) / f64::from(u32::MAX)
        }
    }

    pub fn run(
        canvas: NodeRef<html::Canvas>,
        reduced: Memo<bool>,
        mode: Signal<FlowMode>,
        bps: Signal<Option<f64>>,
        pulses: Signal<u32>,
    ) {
        let sim = Rc::new(RefCell::new(Sim {
            particles: Vec::new(),
            pulses: Vec::new(),
            seen_pulses: 0,
            rng: 0x9e37_79b9,
            last_ts: None,
            colors: Colors::default(),
            colors_at: f64::NEG_INFINITY,
            watchdog: Watchdog::default(),
            w: 0.0,
            h: 0.0,
            dpr: 1.0,
        }));
        let alive: Rc<RefCell<Option<Rc<Cell<bool>>>>> = Rc::new(RefCell::new(None));

        let stop = {
            let alive = alive.clone();
            move || {
                if let Some(a) = alive.borrow_mut().take() {
                    a.set(false);
                }
            }
        };
        let start = {
            let (alive, sim) = (alive.clone(), sim.clone());
            move |el: web_sys::HtmlCanvasElement| {
                if alive.borrow().as_ref().is_some_and(|a| a.get()) {
                    return;
                }
                sim.borrow_mut().last_ts = None;
                let sim = sim.clone();
                let handle =
                    raf_loop(move |ts| frame(&el, &mut sim.borrow_mut(), ts, mode, bps, pulses));
                *alive.borrow_mut() = Some(handle);
            }
        };

        // Start/stop with reduced motion and mount.
        let (start2, stop2) = (start.clone(), stop.clone());
        Effect::new(move |_| {
            let Some(el) = canvas.get() else { return };
            if reduced.get() || document().hidden() {
                stop2();
            } else {
                start2(el);
            }
        });

        // Pause while the tab is hidden.
        let doc_cb = SendWrapper::new(wasm_bindgen::prelude::Closure::<dyn Fn()>::new({
            let (start, stop) = (start.clone(), stop.clone());
            move || {
                if document().hidden() {
                    stop();
                } else if !reduced.get_untracked()
                    && let Some(el) = canvas.get_untracked()
                {
                    start(el);
                }
            }
        }));
        let _ = document()
            .add_event_listener_with_callback("visibilitychange", doc_cb.as_ref().unchecked_ref());

        let stop = SendWrapper::new(stop);
        on_cleanup(move || {
            let _ = document().remove_event_listener_with_callback(
                "visibilitychange",
                doc_cb.as_ref().unchecked_ref(),
            );
            stop();
        });
    }

    fn css_var(el: &web_sys::Element, name: &str) -> String {
        window()
            .get_computed_style(el)
            .ok()
            .flatten()
            .and_then(|s| s.get_property_value(name).ok())
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "#888888".into())
    }

    fn frame(
        el: &web_sys::HtmlCanvasElement,
        sim: &mut Sim,
        ts: f64,
        mode: Signal<FlowMode>,
        bps: Signal<Option<f64>>,
        pulses: Signal<u32>,
    ) -> bool {
        if !el.is_connected() {
            return false;
        }
        let dt_ms = sim.last_ts.map_or(16.7, |l| ts - l);
        sim.last_ts = Some(ts);
        let light = sim.watchdog.frame(dt_ms);
        let dt = (dt_ms / 1000.0).clamp(0.0, 0.05);

        // Size the backing store to the element (DPR capped at 2).
        let (cw, ch) = (f64::from(el.client_width()), f64::from(el.client_height()));
        if cw <= 0.0 || ch <= 0.0 {
            return true;
        }
        let dpr = window().device_pixel_ratio().clamp(1.0, 2.0);
        if (cw - sim.w).abs() >= 1.0 || (ch - sim.h).abs() >= 1.0 || dpr != sim.dpr {
            sim.w = cw;
            sim.h = ch;
            sim.dpr = dpr;
            el.set_width((cw * dpr) as u32);
            el.set_height((ch * dpr) as u32);
        }
        if ts - sim.colors_at > 500.0 {
            sim.colors_at = ts;
            sim.colors = Colors {
                download: css_var(el, "--series-download"),
                upload: css_var(el, "--series-upload"),
                latency: css_var(el, "--series-latency"),
                idle: css_var(el, "--color-text-muted"),
            };
        }
        let Some(ctx) = el
            .get_context("2d")
            .ok()
            .flatten()
            .and_then(|c| c.dyn_into::<web_sys::CanvasRenderingContext2d>().ok())
        else {
            return false;
        };

        let mode = mode.get_untracked();
        let intensity = flow_intensity(bps.get_untracked().unwrap_or(0.0));
        let cores = window().navigator().hardware_concurrency() as u32;
        let mut cap = particle_cap(cw, ch, dpr, cores);
        if light {
            cap = (cap / 3).max(12);
        }
        let target = (cap as f64 * flow_density(mode, intensity)).round() as usize;
        let speed = flow_speed(intensity);

        // Mark particles of another mode as dying.
        for p in &mut sim.particles {
            if p.kind != mode {
                p.dying = true;
            }
        }
        let live = sim.particles.iter().filter(|p| !p.dying).count();
        let spawn = target
            .saturating_sub(live)
            .min(if mode == FlowMode::Idle { 2 } else { 6 });
        for _ in 0..spawn {
            let (rx, ry, rs) = (sim.rand(), sim.rand(), sim.rand());
            let p = match mode {
                FlowMode::Download | FlowMode::Upload => {
                    let down = mode == FlowMode::Download;
                    let v = speed * (0.6 + 0.8 * rs);
                    Particle {
                        x: rx * cw,
                        y: if down {
                            -20.0 - ry * ch * 0.5
                        } else {
                            ch + 20.0 + ry * ch * 0.5
                        },
                        vx: 0.0,
                        vy: if down { v } else { -v },
                        len: (v * 0.05).clamp(6.0, 34.0),
                        r: 1.0 + rs,
                        alpha: 0.35 + 0.5 * rs,
                        kind: mode,
                        dying: false,
                    }
                }
                FlowMode::Idle | FlowMode::Latency => {
                    let a = rs * std::f64::consts::TAU;
                    Particle {
                        x: rx * cw,
                        y: ry * ch,
                        vx: a.cos() * 9.0,
                        vy: a.sin() * 9.0 - 3.0,
                        len: 0.0,
                        r: 1.0 + rs * 1.2,
                        alpha: 0.0,
                        kind: mode,
                        dying: false,
                    }
                }
            };
            sim.particles.push(p);
        }

        // Latency pulses: one per probe.
        let n = pulses.get_untracked();
        if n < sim.seen_pulses {
            sim.seen_pulses = n;
        }
        while sim.seen_pulses < n {
            sim.seen_pulses += 1;
            let x = cw * (0.35 + 0.3 * sim.rand());
            if sim.pulses.len() < 12 {
                sim.pulses.push(Pulse { t0: ts, x });
            }
        }

        // Update.
        for p in &mut sim.particles {
            p.x += p.vx * dt;
            p.y += p.vy * dt;
            match p.kind {
                FlowMode::Idle | FlowMode::Latency => {
                    let target_alpha = if p.dying { 0.0 } else { 0.45 };
                    p.alpha += (target_alpha - p.alpha) * (dt * 1.5).min(1.0);
                    if p.x < -10.0 {
                        p.x = cw + 10.0;
                    } else if p.x > cw + 10.0 {
                        p.x = -10.0;
                    }
                    if p.y < -10.0 {
                        p.y = ch + 10.0;
                    } else if p.y > ch + 10.0 {
                        p.y = -10.0;
                    }
                }
                _ => {
                    if p.dying {
                        p.alpha -= dt * 2.5;
                    }
                }
            }
        }
        sim.particles.retain(|p| {
            let dead_dir = match p.kind {
                FlowMode::Download => p.y - p.len > ch + 4.0,
                FlowMode::Upload => p.y + p.len < -4.0,
                _ => false,
            };
            !dead_dir && !(p.dying && p.alpha <= 0.01)
        });
        if sim.particles.len() > cap * 2 {
            let excess = sim.particles.len() - cap * 2;
            sim.particles.drain(..excess);
        }

        // Draw.
        ctx.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0).ok();
        ctx.clear_rect(0.0, 0.0, cw, ch);
        ctx.set_line_cap("round");
        for p in &sim.particles {
            let color = match p.kind {
                FlowMode::Download => &sim.colors.download,
                FlowMode::Upload => &sim.colors.upload,
                _ => &sim.colors.idle,
            };
            // Fade near the top and bottom edges.
            let edge = ((p.y / (ch * 0.2)).min((ch - p.y) / (ch * 0.2))).clamp(0.0, 1.0);
            let a = (p.alpha * edge).clamp(0.0, 1.0);
            if a <= 0.01 {
                continue;
            }
            ctx.set_global_alpha(a);
            if p.len > 0.0 && !light {
                ctx.set_stroke_style_str(color);
                ctx.set_line_width(p.r * 1.4);
                ctx.begin_path();
                let tail = if p.vy > 0.0 { -p.len } else { p.len };
                ctx.move_to(p.x, p.y + tail);
                ctx.line_to(p.x, p.y);
                ctx.stroke();
            } else {
                ctx.set_fill_style_str(color);
                ctx.begin_path();
                let _ = ctx.arc(p.x, p.y, p.r, 0.0, std::f64::consts::TAU);
                ctx.fill();
            }
        }
        // Pulses travel up from the lower third and back (≈ 900 ms).
        const PULSE_MS: f64 = 900.0;
        sim.pulses.retain(|p| ts - p.t0 < PULSE_MS);
        ctx.set_fill_style_str(&sim.colors.latency);
        ctx.set_stroke_style_str(&sim.colors.latency);
        for p in &sim.pulses {
            let k = ((ts - p.t0) / PULSE_MS).clamp(0.0, 1.0);
            let up = 1.0 - (2.0 * k - 1.0).abs();
            let eased = 1.0 - (1.0 - up).powi(2);
            let y0 = ch * 0.82;
            let y = y0 - eased * ch * 0.62;
            // Faint trail back to the origin.
            ctx.set_global_alpha(0.18 * (1.0 - k));
            ctx.set_line_width(1.5);
            ctx.begin_path();
            ctx.move_to(p.x, y0);
            ctx.line_to(p.x, y);
            ctx.stroke();
            ctx.set_global_alpha(0.85 * (1.0 - k * 0.6));
            ctx.begin_path();
            let _ = ctx.arc(p.x, y, 3.5, 0.0, std::f64::consts::TAU);
            ctx.fill();
        }
        ctx.set_global_alpha(1.0);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intensity_is_logarithmic_and_clamped() {
        assert_eq!(flow_intensity(0.0), 0.0);
        assert_eq!(flow_intensity(f64::NAN), 0.0);
        assert_eq!(flow_intensity(1e6), 0.0);
        assert!((flow_intensity(100e6) - 0.5).abs() < 1e-9);
        assert!((flow_intensity(1e9) - 0.75).abs() < 1e-9);
        assert_eq!(flow_intensity(1e12), 1.0);
        assert!(flow_speed(1.0) > flow_speed(0.0));
        assert!(flow_density(FlowMode::Download, 1.0) <= 1.0);
    }

    #[test]
    fn cap_scales_with_hardware() {
        let fast = particle_cap(1200.0, 480.0, 1.0, 8);
        let slow = particle_cap(1200.0, 480.0, 1.0, 2);
        let retina = particle_cap(1200.0, 480.0, 3.0, 8);
        assert_eq!(fast, 240);
        assert!(slow < fast && retina < fast);
        assert_eq!(particle_cap(10.0, 10.0, 1.0, 1), 16);
    }

    #[test]
    fn watchdog_trips_after_a_slow_second() {
        let mut w = Watchdog::default();
        for _ in 0..30 {
            assert!(!w.frame(16.0));
        }
        for _ in 0..39 {
            w.frame(25.0);
        }
        assert!(!w.tripped);
        w.frame(25.0);
        assert!(w.tripped);
        // Once tripped it stays light.
        assert!(w.frame(10.0));
        // Fast frames in between reset the count.
        let mut w = Watchdog::default();
        for _ in 0..100 {
            w.frame(25.0);
            w.frame(16.0);
            w.frame(16.0);
        }
        assert!(!w.tripped);
    }
}
