//! A stacked area chart of each device's throughput over time (SPEC §7.11
//! group result): one band per device in its categorical color, the band
//! edge drawn as a 2 px line, direct labels where a band is thick enough,
//! plus a legend and a hover readout of every device and the total.

use leptos::html;
use leptos::prelude::*;

use super::scale::{Linear, line_path, rate_axis, tick_label, ticks_from_zero};
use super::{ChartTooltip, Series, Tip, TipRow, ValuesTable, pointer_xy, use_width};
use crate::model::units::format_rate;
use crate::ui_state::use_ui_state;

/// One device's series: `(t_ms, bps)` points, ascending in time.
#[derive(Debug, Clone, PartialEq)]
pub struct StackLayer {
    pub name: String,
    pub series: Series,
    pub points: Vec<(f64, f64)>,
}

const HEIGHT: f64 = 240.0;
const M_LEFT: f64 = 44.0;
const M_RIGHT: f64 = 12.0;
const M_TOP: f64 = 24.0;
const M_BOTTOM: f64 = 28.0;
/// A direct label needs a band at least this thick (px).
const LABEL_MIN_PX: f64 = 18.0;

/// `points` linearly interpolated at `t` (0 outside the series).
pub fn value_at(points: &[(f64, f64)], t: f64) -> f64 {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return 0.0;
    };
    if t < first.0 || t > last.0 {
        return 0.0;
    }
    match points.binary_search_by(|p| p.0.total_cmp(&t)) {
        Ok(i) => points[i].1,
        Err(i) => {
            let (a, b) = (points[i - 1], points[i]);
            let k = if b.0 > a.0 {
                (t - a.0) / (b.0 - a.0)
            } else {
                0.0
            };
            a.1 + (b.1 - a.1) * k
        }
    }
}

/// The union of every layer's sample times, and each layer's cumulative
/// `(lower, upper)` at those times.
pub fn stack(layers: &[Vec<(f64, f64)>]) -> (Vec<f64>, Vec<Vec<(f64, f64)>>) {
    let mut grid: Vec<f64> = layers.iter().flatten().map(|p| p.0).collect();
    grid.sort_by(f64::total_cmp);
    grid.dedup();
    let mut base = vec![0.0; grid.len()];
    let bands = layers
        .iter()
        .map(|pts| {
            grid.iter()
                .zip(base.iter_mut())
                .map(|(&t, b)| {
                    let v = value_at(pts, t).max(0.0);
                    let lo = *b;
                    *b += v;
                    (lo, *b)
                })
                .collect()
        })
        .collect();
    (grid, bands)
}

/// Where each band's direct label goes: the anchor index and the band's
/// mean lower/upper value over 60-90% of the time axis (`None` when empty).
pub fn band_label_spots(bands: &[Vec<(f64, f64)>]) -> Vec<Option<(usize, f64, f64)>> {
    bands
        .iter()
        .map(|b| {
            let n = b.len();
            if n == 0 {
                return None;
            }
            let i0 = (n * 6 / 10).min(n - 1);
            let i1 = (n * 9 / 10).clamp(i0 + 1, n);
            let k = (i1 - i0) as f64;
            let lo = b[i0..i1].iter().map(|x| x.0).sum::<f64>() / k;
            let hi = b[i0..i1].iter().map(|x| x.1).sum::<f64>() / k;
            Some((i1 - 1, lo, hi))
        })
        .collect()
}

/// Trailing rolling mean of `window` samples, to steady jagged 250 ms rates.
pub fn smooth(points: &[(f64, f64)], window: usize) -> Vec<(f64, f64)> {
    let w = window.max(1);
    points
        .iter()
        .enumerate()
        .map(|(i, &(t, _))| {
            let from = (i + 1).saturating_sub(w);
            let s = &points[from..=i];
            (t, s.iter().map(|p| p.1).sum::<f64>() / s.len() as f64)
        })
        .collect()
}

#[component]
pub fn StackedArea(
    #[prop(into)] layers: Signal<Vec<StackLayer>>,
    /// Accessible name, e.g. "Download throughput of each device over time".
    #[prop(into)]
    aria_label: String,
    /// The values table's summary line.
    #[prop(into)]
    table_summary: String,
) -> impl IntoView {
    let ui = use_ui_state();
    let node = NodeRef::<html::Div>::new();
    let width = use_width(node, 480.0);
    let (tip, set_tip) = signal(None::<Tip>);

    let geo = Memo::new(move |_| {
        let w = width.get().max(200.0);
        let unit = ui.units.get();
        layers.with(|layers| {
            let pts: Vec<Vec<(f64, f64)>> = layers.iter().map(|l| l.points.clone()).collect();
            let (grid, bands) = stack(&pts);
            let max = bands
                .last()
                .map(|b| b.iter().map(|x| x.1).fold(0.0, f64::max))
                .unwrap_or(0.0);
            let (per, unit_label) = rate_axis(max, unit);
            let yt = ticks_from_zero(max / per, 4);
            let top = *yt.last().unwrap_or(&1.0);
            let t0 = grid.first().copied().unwrap_or(0.0);
            let t1 = grid.last().copied().unwrap_or(1.0).max(t0 + 1.0);
            let secs = ticks_from_zero((t1 - t0) / 1000.0, ((w / 90.0) as usize).clamp(2, 8));
            StackGeo {
                w,
                x: Linear::new((t0, t1), (M_LEFT, w - M_RIGHT)),
                y: Linear::new((0.0, top * per), (HEIGHT - M_BOTTOM, M_TOP)),
                y_ticks: yt.iter().map(|v| (v * per, tick_label(*v))).collect(),
                x_ticks: secs
                    .into_iter()
                    .filter(|s| t0 + s * 1000.0 <= t1 + 1.0)
                    .map(|s| (t0 + s * 1000.0, format!("{} s", tick_label(s))))
                    .collect(),
                unit_label,
                grid,
                bands,
            }
        })
    });

    let on_move = move |ev: leptos::ev::PointerEvent| {
        let Some((px, _)) = pointer_xy(&ev) else {
            return;
        };
        let g = geo.get_untracked();
        if g.grid.is_empty() {
            return;
        }
        let t = g.x.invert(px);
        let i = g
            .grid
            .iter()
            .enumerate()
            .min_by(|a, b| (a.1 - t).abs().total_cmp(&(b.1 - t).abs()))
            .map_or(0, |(i, _)| i);
        let unit = ui.units.get_untracked();
        let fmt = |v: f64| {
            let (s, u) = format_rate(v, unit);
            format!("{s} {u}")
        };
        let rows = layers.with_untracked(|layers| {
            let mut rows: Vec<TipRow> = layers
                .iter()
                .zip(&g.bands)
                .rev()
                .map(|(l, b)| TipRow {
                    series: Some(l.series),
                    value: fmt(b[i].1 - b[i].0),
                    label: l.name.clone(),
                })
                .collect();
            let total = g.bands.last().map_or(0.0, |b| b[i].1);
            rows.push(TipRow {
                series: None,
                value: fmt(total),
                label: "total".into(),
            });
            rows
        });
        set_tip.set(Some(Tip {
            x: g.x.map(g.grid[i]),
            y: M_TOP,
            title: format!("{:.1} s", (g.grid[i] - g.grid[0]) / 1000.0),
            rows,
        }));
    };

    let table_rows = Signal::derive(move || {
        let unit = ui.units.get();
        let g = geo.get();
        layers.with(|layers| {
            g.grid
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let mut row = vec![format!("{:.1} s", (t - g.grid[0]) / 1000.0)];
                    let mut total = 0.0;
                    for (_, b) in layers.iter().zip(&g.bands) {
                        let v = b[i].1 - b[i].0;
                        total += v;
                        let (s, u) = format_rate(v, unit);
                        row.push(format!("{s} {u}"));
                    }
                    let (s, u) = format_rate(total, unit);
                    row.push(format!("{s} {u}"));
                    row
                })
                .collect()
        })
    });
    let headers = Signal::derive(move || {
        let mut h = vec!["Time".to_string()];
        layers.with(|l| h.extend(l.iter().map(|l| l.name.clone())));
        h.push("Total".into());
        h
    });

    view! {
        <ul class="legend stack-legend">
            {move || {
                layers
                    .get()
                    .into_iter()
                    .map(|l| {
                        view! {
                            <li class=l.series.class()>
                                <span class="swatch-key" aria-hidden="true"></span>
                                {l.name}
                            </li>
                        }
                    })
                    .collect_view()
            }}
        </ul>
        <div class="chart-wrap" node_ref=node>
            <svg
                class="chart-svg stacked-area"
                width=move || geo.with(|g| g.w)
                height=HEIGHT
                viewBox=move || geo.with(|g| format!("0 0 {} {}", g.w, HEIGHT))
                role="img"
                aria-label=aria_label
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
                    let last = g.x_ticks.len().saturating_sub(1);
                    g.x_ticks
                        .iter()
                        .enumerate()
                        .map(|(i, (t, label))| {
                            let anchor = if i == 0 { "start" } else if i == last { "end" } else { "middle" };
                            view! {
                                <text class="axis-label" x=g.x.map(*t) y=HEIGHT - 8.0 text-anchor=anchor>
                                    {label.clone()}
                                </text>
                            }
                        })
                        .collect_view()
                }}
                {move || {
                    let g = geo.get();
                    let names = layers.with(|l| l.iter().map(|l| (l.name.clone(), l.series)).collect::<Vec<_>>());
                    // Direct labels: one column near the right, skipping
                    // bands too thin to hold text and labels that would collide.
                    let mut label_at: Vec<Option<(f64, f64)>> = Vec::new();
                    let mut last_y = f64::INFINITY;
                    for spot in band_label_spots(&g.bands) {
                        let pos = spot.and_then(|(i, lo, hi)| {
                            let px = g.y.map(lo) - g.y.map(hi);
                            let y = (g.y.map(lo) + g.y.map(hi)) / 2.0 + 4.0;
                            (px >= LABEL_MIN_PX && last_y - y >= 14.0)
                                .then(|| (g.x.map(g.grid[i]).min(g.w - M_RIGHT - 6.0), y))
                        });
                        if let Some((_, y)) = pos {
                            last_y = y;
                        }
                        label_at.push(pos);
                    }
                    names
                        .into_iter()
                        .zip(g.bands.iter())
                        .enumerate()
                        .map(|(bi, ((name, series), band))| {
                            let upper: Vec<(f64, f64)> = g
                                .grid
                                .iter()
                                .zip(band)
                                .map(|(&t, &(_, hi))| (g.x.map(t), g.y.map(hi)))
                                .collect();
                            let lower: Vec<(f64, f64)> = g
                                .grid
                                .iter()
                                .zip(band)
                                .rev()
                                .map(|(&t, &(lo, _))| (g.x.map(t), g.y.map(lo)))
                                .collect();
                            let mut d = line_path(&upper);
                            for (x, y) in &lower {
                                d.push_str(&format!("L{x:.1} {y:.1}"));
                            }
                            if !upper.is_empty() {
                                d.push('Z');
                            }
                            let label = label_at.get(bi).copied().flatten();
                            view! {
                                <g class=format!("stack-band {}", series.class())>
                                    <path class="band-fill" d=d></path>
                                    <path class="line" d=line_path(&upper)></path>
                                    {label
                                        .map(|(x, y)| {
                                            view! {
                                                <text class="band-label" x=x y=y text-anchor="end">
                                                    {name.clone()}
                                                </text>
                                            }
                                        })}
                                </g>
                            }
                        })
                        .collect_view()
                }}
                <line
                    class="baseline"
                    x1=M_LEFT
                    x2=move || geo.with(|g| g.w - M_RIGHT)
                    y1=HEIGHT - M_BOTTOM
                    y2=HEIGHT - M_BOTTOM
                ></line>
                {move || {
                    tip.get()
                        .map(|t| {
                            view! {
                                <line class="crosshair" x1=t.x x2=t.x y1=M_TOP y2=HEIGHT - M_BOTTOM></line>
                            }
                        })
                }}
            </svg>
            <ChartTooltip tip=tip width=width/>
        </div>
        {move || {
            view! { <ValuesTable summary=table_summary.clone() headers=headers.get() rows=table_rows/> }
        }}
    }
}

#[derive(Debug, Clone, PartialEq)]
struct StackGeo {
    w: f64,
    x: Linear,
    y: Linear,
    y_ticks: Vec<(f64, String)>,
    x_ticks: Vec<(f64, String)>,
    unit_label: &'static str,
    grid: Vec<f64>,
    bands: Vec<Vec<(f64, f64)>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolates_inside_and_zero_outside() {
        let p = [(0.0, 0.0), (100.0, 10.0), (200.0, 30.0)];
        assert_eq!(value_at(&p, 50.0), 5.0);
        assert_eq!(value_at(&p, 100.0), 10.0);
        assert_eq!(value_at(&p, 150.0), 20.0);
        assert_eq!(value_at(&p, 250.0), 0.0);
        assert_eq!(value_at(&[], 1.0), 0.0);
    }

    #[test]
    fn smoothing_and_label_spots() {
        let p = [(0.0, 0.0), (1.0, 4.0), (2.0, 8.0)];
        assert_eq!(smooth(&p, 2), vec![(0.0, 0.0), (1.0, 2.0), (2.0, 6.0)]);
        assert_eq!(smooth(&p, 1), p.to_vec());
        let band: Vec<(f64, f64)> = (0..10).map(|_| (1.0, 3.0)).collect();
        assert_eq!(
            band_label_spots(&[band, vec![]]),
            vec![Some((8, 1.0, 3.0)), None]
        );
    }

    #[test]
    fn stacks_on_a_union_grid() {
        let a = vec![(0.0, 10.0), (200.0, 10.0)];
        let b = vec![(100.0, 5.0), (200.0, 7.0)];
        let (grid, bands) = stack(&[a, b]);
        assert_eq!(grid, vec![0.0, 100.0, 200.0]);
        assert_eq!(bands[0], vec![(0.0, 10.0), (0.0, 10.0), (0.0, 10.0)]);
        assert_eq!(bands[1], vec![(10.0, 10.0), (10.0, 15.0), (10.0, 17.0)]);
    }
}
