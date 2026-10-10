//! Inline-SVG charts (SPEC §7.3, §7.4), following the dataviz rules: thin
//! marks, hairline recessive axes, text in text tokens, a legend for two or
//! more series, hover tooltips that never gate (each chart also has a values
//! table in `<details>`).
//!
//! Charts measure their container with a `ResizeObserver` and draw in real
//! pixels, so text and dots never get stretched.

pub mod box_plot;
pub mod dot_plot;
pub mod live_area;
pub mod loss_bar;
pub mod scale;
pub mod sparkline;
pub mod stacked_area;
pub mod strip_plot;

pub use box_plot::{BoxPlotChart, BoxRow};
pub use dot_plot::DotPlot;
pub use live_area::{LiveArea, LivePoint, PayloadTick};
pub use loss_bar::LossBar;
pub use sparkline::Sparkline;
pub use stacked_area::{StackLayer, StackedArea};
pub use strip_plot::{StripPlot, StripRow};

use leptos::html;
use leptos::prelude::*;

/// Data series, mapped to the `--series-*` tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Series {
    Download,
    Upload,
    Latency,
    /// A categorical slot (`1..=8`, the `--device-*` tokens): one color per
    /// device in group tests, per item in compare.
    Device(u8),
}

impl Series {
    /// CSS class suffix: `series-download` etc.
    pub fn class(self) -> &'static str {
        match self {
            Series::Download => "series-download",
            Series::Upload => "series-upload",
            Series::Latency => "series-latency",
            Series::Device(n) => DEVICE_CLASSES[usize::from(n.clamp(1, 8)) - 1],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Series::Download => "Download",
            Series::Upload => "Upload",
            Series::Latency => "Latency",
            Series::Device(_) => "Device",
        }
    }

    /// The categorical slot for the `i`-th device or item (0-based), cycling
    /// through the eight colors in their fixed order.
    pub fn slot(i: usize) -> Series {
        Series::Device((i % 8) as u8 + 1)
    }
}

const DEVICE_CLASSES: [&str; 8] = [
    "series-device-1",
    "series-device-2",
    "series-device-3",
    "series-device-4",
    "series-device-5",
    "series-device-6",
    "series-device-7",
    "series-device-8",
];

/// The width of `node` in CSS pixels, kept up to date with a
/// `ResizeObserver`. `initial` is used on the server and before mount.
pub fn use_width(node: NodeRef<html::Div>, initial: f64) -> ReadSignal<f64> {
    let (width, set_width) = signal(initial);
    #[cfg(feature = "hydrate")]
    Effect::new(move |_| {
        use send_wrapper::SendWrapper;
        use wasm_bindgen::JsCast;
        use wasm_bindgen::prelude::Closure;

        let Some(el) = node.get() else { return };
        let el: web_sys::Element = el.unchecked_into();
        let w = f64::from(el.client_width());
        if w > 0.0 {
            set_width.set(w);
        }
        let cb = Closure::<dyn Fn(js_sys::Array)>::new(move |entries: js_sys::Array| {
            let Ok(entry) = entries.get(0).dyn_into::<web_sys::ResizeObserverEntry>() else {
                return;
            };
            let w = entry.content_rect().width().round();
            if w > 0.0 && (w - width.get_untracked()).abs() >= 1.0 {
                set_width.set(w);
            }
        });
        let Ok(observer) = web_sys::ResizeObserver::new(cb.as_ref().unchecked_ref()) else {
            return;
        };
        observer.observe(&el);
        let keep = SendWrapper::new((observer, cb));
        on_cleanup(move || keep.0.disconnect());
    });
    #[cfg(not(feature = "hydrate"))]
    let _ = (node, set_width);
    width
}

/// One row of a chart tooltip: an optional series key, the value (strong)
/// and its label (secondary).
#[derive(Debug, Clone, PartialEq)]
pub struct TipRow {
    pub series: Option<Series>,
    pub value: String,
    pub label: String,
}

/// A tooltip anchored at `(x, y)` in chart pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct Tip {
    pub x: f64,
    pub y: f64,
    pub title: String,
    pub rows: Vec<TipRow>,
}

/// The floating readout shared by every chart. Place it inside the chart's
/// `position: relative` wrapper.
#[component]
pub fn ChartTooltip(tip: ReadSignal<Option<Tip>>, width: ReadSignal<f64>) -> impl IntoView {
    move || {
        tip.get().map(|t| {
            // Keep the box inside the chart: flip to the left near the edge.
            let flip = t.x > width.get() * 0.6;
            let style = if flip {
                format!(
                    "right:{:.0}px;top:{:.0}px",
                    (width.get() - t.x + 12.0).max(0.0),
                    t.y
                )
            } else {
                format!("left:{:.0}px;top:{:.0}px", t.x + 12.0, t.y)
            };
            view! {
                <div class="chart-tip" style=style role="presentation">
                    <div class="chart-tip-title">{t.title}</div>
                    {t
                        .rows
                        .into_iter()
                        .map(|r| {
                            view! {
                                <div class="chart-tip-row">
                                    {r
                                        .series
                                        .map(|s| {
                                            view! {
                                                <span class=format!("line-key {}", s.class())></span>
                                            }
                                        })}
                                    <span class="chart-tip-value">{r.value}</span>
                                    <span class="chart-tip-label">{r.label}</span>
                                </div>
                            }
                        })
                        .collect_view()}
                </div>
            }
        })
    }
}

/// A `<details>` values table: the text alternative every chart carries.
#[component]
pub fn ValuesTable(
    #[prop(into)] summary: String,
    headers: Vec<String>,
    #[prop(into)] rows: Signal<Vec<Vec<String>>>,
) -> impl IntoView {
    view! {
        <details class="values-table">
            <summary>{summary}</summary>
            <div class="table-scroll">
                <table>
                    <thead>
                        <tr>{headers.into_iter().map(|h| view! { <th scope="col">{h}</th> }).collect_view()}</tr>
                    </thead>
                    <tbody>
                        {move || {
                            rows.get()
                                .into_iter()
                                .map(|row| {
                                    view! {
                                        <tr>{row.into_iter().map(|c| view! { <td>{c}</td> }).collect_view()}</tr>
                                    }
                                })
                                .collect_view()
                        }}
                    </tbody>
                </table>
            </div>
        </details>
    }
}

/// Pointer position relative to the event's current target (the chart's
/// `<svg>`), in CSS pixels.
pub fn pointer_xy(ev: &leptos::ev::PointerEvent) -> Option<(f64, f64)> {
    use wasm_bindgen::JsCast;
    let el = ev.current_target()?.dyn_into::<web_sys::Element>().ok()?;
    let rect = el.get_bounding_client_rect();
    Some((
        f64::from(ev.client_x()) - rect.left(),
        f64::from(ev.client_y()) - rect.top(),
    ))
}
