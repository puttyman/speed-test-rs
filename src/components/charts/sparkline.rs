//! A small trend line for metric tiles.

use leptos::prelude::*;

use super::Series;
use super::scale::{area_path, line_path, sparkline_points};

const W: f64 = 120.0;
const H: f64 = 32.0;

/// Sparkline of `values` (oldest first) in the series color, with a 10% wash.
/// Stretches to its container's width; strokes don't scale.
#[component]
pub fn Sparkline(
    #[prop(into)] values: Signal<Vec<f64>>,
    series: Series,
    /// Start the y axis at 0 (throughput) or at the minimum (latency).
    #[prop(default = true)]
    from_zero: bool,
) -> impl IntoView {
    let points = Memo::new(move |_| values.with(|v| sparkline_points(v, W, H, 3.0, from_zero)));
    view! {
        <svg
            class=format!("sparkline {}", series.class())
            viewBox=format!("0 0 {W} {H}")
            preserveAspectRatio="none"
            aria-hidden="true"
            focusable="false"
        >
            <path class="spark-area" d=move || points.with(|p| if p.len() > 1 { area_path(p, H) } else { String::new() })></path>
            <path
                class="spark-line"
                d=move || points.with(|p| line_path(p))
                vector-effect="non-scaling-stroke"
            ></path>
        </svg>
    }
}
