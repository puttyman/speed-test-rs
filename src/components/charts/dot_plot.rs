//! Every download or upload sample as a dot, grouped by payload size, with
//! the p90 line that the headline number comes from (SPEC §7.4).

use leptos::html;
use leptos::prelude::*;

use super::scale::{Linear, format_bytes, rate_axis, spread, tick_label, ticks_from_zero};
use super::{ChartTooltip, Series, Tip, TipRow, ValuesTable, pointer_xy, use_width};
use crate::model::result::BandwidthPoint;
use crate::model::units::format_rate;
use crate::ui_state::use_ui_state;

const HEIGHT: f64 = 240.0;
const M_LEFT: f64 = 44.0;
const M_RIGHT: f64 = 12.0;
const M_TOP: f64 = 24.0;
const M_BOTTOM: f64 = 28.0;
const R: f64 = 4.0;
/// Hit radius: well over the 8 px dot (dataviz: ≥ 24 px targets).
const HIT_R: f64 = 12.0;

/// Top of the y scale: the largest value, unless a few outliers sit far
/// above the reference (p90). Then the scale stops at 2× the reference and
/// those dots are drawn as markers on the top edge, so one bogus sample can't
/// flatten the rest.
pub fn scale_max(values: &[f64], reference: Option<f64>) -> f64 {
    let max = values.iter().copied().fold(0.0, f64::max);
    let reference = reference
        .filter(|r| r.is_finite() && *r > 0.0)
        .or_else(|| crate::model::stats::percentile(values, 0.9))
        .unwrap_or(max);
    if reference > 0.0 && max > 3.0 * reference {
        2.0 * reference
    } else {
        max
    }
}

/// Distinct payload sizes, ascending.
pub fn bands(points: &[BandwidthPoint]) -> Vec<u64> {
    let mut sizes: Vec<u64> = points.iter().map(|p| p.bytes).collect();
    sizes.sort_unstable();
    sizes.dedup();
    sizes
}

/// x position of each point: its band's center plus a deterministic spread
/// of at most ±28% of the band (and ±36 px).
pub fn dot_xs(points: &[BandwidthPoint], sizes: &[u64], x0: f64, x1: f64) -> Vec<f64> {
    let n = sizes.len().max(1) as f64;
    let band = (x1 - x0) / n;
    let half = (band * 0.28).min(36.0);
    let mut seen = vec![0usize; sizes.len()];
    points
        .iter()
        .map(|p| {
            let b = sizes.binary_search(&p.bytes).unwrap_or(0);
            let k = seen[b];
            seen[b] += 1;
            x0 + band * (b as f64 + 0.5) + spread(k) * half
        })
        .collect()
}

#[component]
pub fn DotPlot(
    #[prop(into)] points: Signal<Vec<BandwidthPoint>>,
    series: Series,
    /// The run's reduced value (p90), drawn as a reference line.
    #[prop(into)]
    p90: Signal<Option<f64>>,
) -> impl IntoView {
    let ui = use_ui_state();
    let node = NodeRef::<html::Div>::new();
    let width = use_width(node, 480.0);
    let (tip, set_tip) = signal(None::<Tip>);

    let geo = Memo::new(move |_| {
        let w = width.get().max(200.0);
        let unit = ui.units.get();
        points.with(|pts| {
            let vals: Vec<f64> = pts.iter().map(|p| p.bps).collect();
            let max = scale_max(&vals, p90.get());
            let (per, unit_label) = rate_axis(max, unit);
            let yt = ticks_from_zero(max / per, 4);
            let top = *yt.last().unwrap_or(&1.0);
            let sizes = bands(pts);
            let xs = dot_xs(pts, &sizes, M_LEFT, w - M_RIGHT);
            let clipped = vals.iter().filter(|&&v| v > top * per).count();
            DotGeo {
                w,
                clipped,
                y: Linear::new((0.0, top * per), (HEIGHT - M_BOTTOM, M_TOP)),
                y_ticks: yt.iter().map(|v| (v * per, tick_label(*v))).collect(),
                unit_label,
                sizes,
                xs,
            }
        })
    });

    let on_move = move |ev: leptos::ev::PointerEvent| {
        let Some((px, py)) = pointer_xy(&ev) else {
            return;
        };
        let g = geo.get_untracked();
        let unit = ui.units.get_untracked();
        let hit = points.with_untracked(|pts| {
            pts.iter()
                .zip(&g.xs)
                .map(|(p, &x)| {
                    let (dx, dy) = (x - px, g.y.map(p.bps).max(M_TOP) - py);
                    (dx * dx + dy * dy, *p, x)
                })
                .filter(|(d2, _, _)| *d2 <= HIT_R * HIT_R)
                .min_by(|a, b| a.0.total_cmp(&b.0))
        });
        set_tip.set(hit.map(|(_, p, x)| {
            let (v, u) = format_rate(p.bps, unit);
            Tip {
                x,
                y: (g.y.map(p.bps).max(M_TOP) - 20.0).max(0.0),
                title: format!("{} request", format_bytes(p.bytes)),
                rows: vec![
                    TipRow {
                        series: Some(series),
                        value: format!("{v} {u}"),
                        label: series.label().into(),
                    },
                    TipRow {
                        series: None,
                        value: format!("{:.0} ms", p.duration_ms),
                        label: "duration".into(),
                    },
                ],
            }
        }));
    };

    let table_rows = Signal::derive(move || {
        let unit = ui.units.get();
        points.with(|pts| {
            pts.iter()
                .map(|p| {
                    let (v, u) = format_rate(p.bps, unit);
                    vec![
                        format_bytes(p.bytes),
                        format!("{v} {u}"),
                        format!("{:.0} ms", p.duration_ms),
                    ]
                })
                .collect()
        })
    });

    let aria = format!("{} speed of every request, by payload size", series.label());
    view! {
        <div class="chart-wrap" node_ref=node>
            <svg
                class=format!("chart-svg dot-plot {}", series.class())
                width=move || geo.with(|g| g.w)
                height=HEIGHT
                viewBox=move || geo.with(|g| format!("0 0 {} {}", g.w, HEIGHT))
                role="img"
                aria-label=aria
                on:pointermove=on_move
                on:pointerleave=move |_| set_tip.set(None)
            >
                {move || {
                    let g = geo.get();
                    g.y_ticks
                        .iter()
                        .map(|(v, label)| {
                            let y = g.y.map(*v);
                            view! {
                                <line class="grid" x1=M_LEFT x2=g.w - M_RIGHT y1=y y2=y></line>
                                <text class="axis-label" x=M_LEFT - 8.0 y=y + 4.0 text-anchor="end">
                                    {label.clone()}
                                </text>
                            }
                        })
                        .collect_view()
                }}
                <text class="axis-unit" x=M_LEFT - 8.0 y=M_TOP - 12.0 text-anchor="end">
                    {move || geo.with(|g| g.unit_label)}
                </text>
                {move || {
                    let g = geo.get();
                    let n = g.sizes.len().max(1) as f64;
                    let band = (g.w - M_RIGHT - M_LEFT) / n;
                    let every = if band < 44.0 { 2 } else { 1 };
                    g.sizes
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| i % every == 0)
                        .map(|(i, &b)| {
                            let x = M_LEFT + band * (i as f64 + 0.5);
                            view! {
                                <text class="axis-label" x=x y=HEIGHT - 8.0 text-anchor="middle">
                                    {format_bytes(b)}
                                </text>
                            }
                        })
                        .collect_view()
                }}
                // p90 reference line, labelled in text tokens.
                {move || {
                    let g = geo.get();
                    let unit = ui.units.get();
                    p90.get()
                        .map(|v| {
                            let y = g.y.map(v);
                            let (s, u) = format_rate(v, unit);
                            view! {
                                <line class="ref-line" x1=M_LEFT x2=g.w - M_RIGHT y1=y y2=y></line>
                                <text class="ref-label" x=g.w - M_RIGHT y=y - 6.0 text-anchor="end">
                                    {format!("p90 · {s} {u}")}
                                </text>
                            }
                        })
                }}
                <line
                    class="baseline"
                    x1=M_LEFT
                    x2=move || geo.with(|g| g.w - M_RIGHT)
                    y1=HEIGHT - M_BOTTOM
                    y2=HEIGHT - M_BOTTOM
                ></line>
                <g class="dots">
                    {move || {
                        let g = geo.get();
                        points
                            .get()
                            .into_iter()
                            .zip(g.xs.clone())
                            .map(|(p, x)| {
                                let y = g.y.map(p.bps);
                                if y < M_TOP {
                                    // Off the scale: an upward marker on the top edge.
                                    let d = format!("M{:.1} {:.1}l5 8h-10z", x, M_TOP - 4.0);
                                    view! { <path class="dot dot-clipped" d=d></path> }.into_any()
                                } else {
                                    view! { <circle class="dot" cx=x cy=y r=R></circle> }.into_any()
                                }
                            })
                            .collect_view()
                    }}
                </g>
            </svg>
            <ChartTooltip tip=tip width=width/>
        </div>
        {move || {
            let n = geo.with(|g| g.clipped);
            (n > 0)
                .then(|| {
                    let what = if n == 1 { "sample is" } else { "samples are" };
                    view! {
                        <p class="chart-note">
                            {format!("\u{25B2} {n} {what} far above the rest, shown on the top edge.")}
                        </p>
                    }
                })
        }}
        <ValuesTable
            summary=format!("{} values", series.label())
            headers=vec!["Payload", "Speed", "Duration"].into_iter().map(String::from).collect()
            rows=table_rows
        />
    }
}

#[derive(Debug, Clone, PartialEq)]
struct DotGeo {
    w: f64,
    /// Samples above the scale, drawn on the top edge.
    clipped: usize,
    y: Linear,
    y_ticks: Vec<(f64, String)>,
    unit_label: &'static str,
    sizes: Vec<u64>,
    xs: Vec<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(bytes: u64) -> BandwidthPoint {
        BandwidthPoint {
            bytes,
            bps: 1e8,
            duration_ms: 10.0,
            server_ms: None,
        }
    }

    #[test]
    fn groups_into_bands() {
        let pts = [pt(1_000_000), pt(100_000), pt(1_000_000), pt(100_000)];
        let sizes = bands(&pts);
        assert_eq!(sizes, vec![100_000, 1_000_000]);
        let xs = dot_xs(&pts, &sizes, 0.0, 200.0);
        // Band centers at 50 and 150; the first dot of each band sits on it.
        assert_eq!(xs[0], 150.0);
        assert_eq!(xs[1], 50.0);
        // Others stay inside their band.
        assert!((122.0..=178.0).contains(&xs[2]));
        assert!((22.0..=78.0).contains(&xs[3]));
        assert!(dot_xs(&[], &[], 0.0, 10.0).is_empty());
    }

    #[test]
    fn outliers_dont_stretch_the_scale() {
        assert_eq!(scale_max(&[1.0, 2.0, 3.0], Some(2.5)), 3.0);
        assert_eq!(scale_max(&[1.0, 2.0, 30.0], Some(2.0)), 4.0);
        assert_eq!(scale_max(&[], None), 0.0);
        // Without a reference, the 90th percentile is used.
        let mut v = vec![1.0; 20];
        v.push(100.0);
        assert_eq!(scale_max(&v, None), 2.0);
    }
}
