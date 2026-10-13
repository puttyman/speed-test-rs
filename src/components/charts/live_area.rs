//! Live throughput over time (SPEC §7.3): download and upload areas drawn as
//! samples arrive, with thin ticks where the payload size changes.

use leptos::html;
use leptos::prelude::*;

use super::scale::{
    Linear, area_path, format_bytes, line_path, rate_axis, tick_label, ticks_between,
    ticks_from_zero,
};
use super::{ChartTooltip, Series, Tip, TipRow, ValuesTable, pointer_xy, use_width};
use crate::model::profile::Direction;
use crate::model::units::format_rate;
use crate::ui_state::use_ui_state;

/// One live throughput reading, `t_ms` after the run started.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LivePoint {
    pub t_ms: f64,
    pub bps: f64,
    pub direction: Direction,
}

/// The start of a download/upload step with a new payload size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PayloadTick {
    pub t_ms: f64,
    pub bytes: u64,
    pub direction: Direction,
}

const HEIGHT: f64 = 220.0;
const M_LEFT: f64 = 44.0;
const M_RIGHT: f64 = 12.0;
const M_TOP: f64 = 22.0;
const M_BOTTOM: f64 = 26.0;
/// A gap longer than this starts a new segment.
const SEGMENT_GAP_MS: f64 = 1500.0;

fn series_of(d: Direction) -> Series {
    match d {
        Direction::Download => Series::Download,
        Direction::Upload => Series::Upload,
    }
}

/// Splits points into same-direction runs without long gaps.
pub fn segments(points: &[LivePoint]) -> Vec<(Direction, Vec<LivePoint>)> {
    let mut out: Vec<(Direction, Vec<LivePoint>)> = Vec::new();
    for p in points {
        match out.last_mut() {
            Some((d, seg))
                if *d == p.direction
                    && seg
                        .last()
                        .is_some_and(|l| p.t_ms - l.t_ms <= SEGMENT_GAP_MS) =>
            {
                seg.push(*p)
            }
            _ => out.push((p.direction, vec![*p])),
        }
    }
    out
}

/// Thins a segment to at most `max` points by averaging consecutive runs of
/// points (long timed tests produce thousands of readings; the path only needs
/// about one point per two pixels).
pub fn thin(seg: &[LivePoint], max: usize) -> Vec<LivePoint> {
    let max = max.max(2);
    if seg.len() <= max {
        return seg.to_vec();
    }
    let chunk = seg.len().div_ceil(max);
    seg.chunks(chunk)
        .map(|c| {
            let n = c.len() as f64;
            LivePoint {
                t_ms: c.iter().map(|p| p.t_ms).sum::<f64>() / n,
                bps: c.iter().map(|p| p.bps).sum::<f64>() / n,
                direction: c[0].direction,
            }
        })
        .collect()
}

/// Index of the point closest in time to `t_ms` (points sorted by time).
pub fn nearest(points: &[LivePoint], t_ms: f64) -> Option<usize> {
    if points.is_empty() {
        return None;
    }
    let i = points.partition_point(|p| p.t_ms < t_ms);
    if i == 0 {
        return Some(0);
    }
    if i >= points.len() {
        return Some(points.len() - 1);
    }
    let (a, b) = (points[i - 1].t_ms, points[i].t_ms);
    Some(if t_ms - a <= b - t_ms { i - 1 } else { i })
}

#[component]
pub fn LiveArea(
    #[prop(into)] points: Signal<Vec<LivePoint>>,
    #[prop(into)] ticks: Signal<Vec<PayloadTick>>,
    /// Shown instead of the plot while there are no points.
    #[prop(into)]
    empty_text: Signal<String>,
) -> impl IntoView {
    let ui = use_ui_state();
    let node = NodeRef::<html::Div>::new();
    let width = use_width(node, 640.0);
    let (tip, set_tip) = signal(None::<Tip>);

    // Scales, recomputed when data, width or units change.
    let geo = Memo::new(move |_| {
        let w = width.get().max(200.0);
        let unit = ui.units.get();
        points.with(|pts| {
            let max_bps = pts.iter().map(|p| p.bps).fold(0.0, f64::max);
            let (per, unit_label) = rate_axis(max_bps, unit);
            let yt = ticks_from_zero(max_bps / per, 4);
            let ymax = *yt.last().unwrap_or(&1.0);
            let t_end = pts.last().map_or(0.0, |p| p.t_ms).max(4000.0);
            Geo {
                w,
                x: Linear::new((0.0, t_end), (M_LEFT, w - M_RIGHT)),
                y: Linear::new((0.0, ymax * per), (HEIGHT - M_BOTTOM, M_TOP)),
                y_ticks: yt.iter().map(|v| (v * per, tick_label(*v))).collect(),
                unit_label,
                t_end,
            }
        })
    });

    let paths = Memo::new(move |_| {
        let g = geo.get();
        points.with(|pts| {
            // Points per segment, in proportion to its share of the time axis.
            let per_ms = ((g.w - M_LEFT - M_RIGHT) / 2.0) / g.t_end.max(1.0);
            segments(pts)
                .into_iter()
                .map(|(d, seg)| {
                    let span = seg.last().map_or(0.0, |l| l.t_ms) - seg[0].t_ms;
                    let seg = thin(&seg, (span * per_ms).ceil() as usize + 2);
                    let xy: Vec<(f64, f64)> = seg
                        .iter()
                        .map(|p| (g.x.map(p.t_ms), g.y.map(p.bps)))
                        .collect();
                    let lone = (xy.len() == 1).then(|| xy[0]);
                    (
                        series_of(d),
                        area_path(&xy, g.y.map(0.0)),
                        line_path(&xy),
                        lone,
                    )
                })
                .collect::<Vec<_>>()
        })
    });

    let on_move = move |ev: leptos::ev::PointerEvent| {
        let Some((px, _)) = pointer_xy(&ev) else {
            return;
        };
        let g = geo.get_untracked();
        let t = g.x.invert(px);
        let unit = ui.units.get_untracked();
        let found = points.with_untracked(|pts| nearest(pts, t).map(|i| pts[i]));
        set_tip.set(found.map(|p| {
            let (v, u) = format_rate(p.bps, unit);
            Tip {
                x: g.x.map(p.t_ms),
                y: g.y.map(p.bps).min(HEIGHT - M_BOTTOM - 40.0),
                title: format!("{:.1} s", p.t_ms / 1000.0),
                rows: vec![TipRow {
                    series: Some(series_of(p.direction)),
                    value: format!("{v} {u}"),
                    label: series_of(p.direction).label().into(),
                }],
            }
        }));
    };

    let table_rows = Signal::derive(move || {
        let unit = ui.units.get();
        points.with(|pts| {
            // About one row per second (fewer for long runs: at most ~120).
            let every = pts.last().map_or(1000.0, |l| (l.t_ms / 120.0).max(1000.0));
            let mut rows = Vec::new();
            let mut next = 0.0;
            for p in pts {
                if p.t_ms >= next {
                    let (v, u) = format_rate(p.bps, unit);
                    rows.push(vec![
                        format!("{:.1} s", p.t_ms / 1000.0),
                        series_of(p.direction).label().to_string(),
                        format!("{v} {u}"),
                    ]);
                    next = p.t_ms + every;
                }
            }
            rows
        })
    });

    let has_points = move || points.with(|p| !p.is_empty());

    view! {
        <div class="live-chart">
            <div class="chart-head">
                <h3 class="chart-title">"Throughput"</h3>
                <ul class="legend" aria-label="Legend">
                    <li>
                        <span class="line-key series-download"></span>
                        "Download"
                    </li>
                    <li>
                        <span class="line-key series-upload"></span>
                        "Upload"
                    </li>
                </ul>
            </div>
            <div class="chart-wrap" node_ref=node>
                <svg
                    class="chart-svg"
                    width=move || geo.with(|g| g.w)
                    height=HEIGHT
                    viewBox=move || geo.with(|g| format!("0 0 {} {}", g.w, HEIGHT))
                    role="img"
                    aria-label="Throughput over time"
                    on:pointermove=on_move
                    on:pointerleave=move |_| set_tip.set(None)
                >
                    // Grid and y labels
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
                    <text class="axis-unit" x=M_LEFT - 8.0 y=M_TOP - 10.0 text-anchor="end">
                        {move || geo.with(|g| g.unit_label)}
                    </text>
                    // x labels (seconds)
                    {move || {
                        let g = geo.get();
                        let target = ((g.w - M_LEFT) / 90.0).clamp(2.0, 8.0) as usize;
                        ticks_between(0.0, g.t_end / 1000.0, target)
                            .into_iter()
                            .map(|s| {
                                let x = g.x.map(s * 1000.0);
                                view! {
                                    <text class="axis-label" x=x y=HEIGHT - 6.0 text-anchor="middle">
                                        {format!("{} s", tick_label(s))}
                                    </text>
                                }
                            })
                            .collect_view()
                    }}
                    // Payload-size ticks
                    {move || {
                        let g = geo.get();
                        let mut last_label = f64::NEG_INFINITY;
                        ticks
                            .get()
                            .into_iter()
                            .map(|t| {
                                let x = g.x.map(t.t_ms);
                                let show = x - last_label >= 52.0 && x <= g.w - M_RIGHT - 20.0;
                                if show {
                                    last_label = x;
                                }
                                view! {
                                    <line class="payload-tick" x1=x x2=x y1=M_TOP y2=HEIGHT - M_BOTTOM></line>
                                    {show
                                        .then(|| {
                                            view! {
                                                <text class="tick-label" x=x + 4.0 y=M_TOP - 10.0>
                                                    {format_bytes(t.bytes)}
                                                </text>
                                            }
                                        })}
                                }
                            })
                            .collect_view()
                    }}
                    // Series
                    {move || {
                        paths
                            .get()
                            .into_iter()
                            .map(|(s, area, line, lone)| {
                                view! {
                                    <g class=s.class()>
                                        <path class="area" d=area></path>
                                        <path class="line" d=line></path>
                                        // A single reading can't draw a line: show it as a dot.
                                        {lone
                                            .map(|(x, y)| {
                                                view! { <circle class="point" cx=x cy=y r=3></circle> }
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
                    // Crosshair
                    {move || {
                        tip.get()
                            .map(|t| {
                                view! {
                                    <line class="crosshair" x1=t.x x2=t.x y1=M_TOP y2=HEIGHT - M_BOTTOM></line>
                                }
                            })
                    }}
                </svg>
                <Show when=move || !has_points()>
                    <p class="chart-empty">{move || empty_text.get()}</p>
                </Show>
                <ChartTooltip tip=tip width=width/>
            </div>
            <ValuesTable
                summary="Throughput values"
                headers=vec!["Time", "Direction", "Speed"].into_iter().map(String::from).collect()
                rows=table_rows
            />
        </div>
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Geo {
    w: f64,
    x: Linear,
    y: Linear,
    y_ticks: Vec<(f64, String)>,
    unit_label: &'static str,
    t_end: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(t: f64, d: Direction) -> LivePoint {
        LivePoint {
            t_ms: t,
            bps: 1e6,
            direction: d,
        }
    }

    #[test]
    fn splits_segments() {
        use Direction::*;
        let pts = [
            p(0.0, Download),
            p(100.0, Download),
            p(200.0, Upload),
            p(300.0, Upload),
            p(5000.0, Upload),
            p(5100.0, Download),
        ];
        let segs = segments(&pts);
        let shape: Vec<(Direction, usize)> = segs.iter().map(|(d, s)| (*d, s.len())).collect();
        assert_eq!(
            shape,
            vec![(Download, 2), (Upload, 2), (Upload, 1), (Download, 1)]
        );
        assert!(segments(&[]).is_empty());
    }

    #[test]
    fn thins_long_segments() {
        let pts: Vec<_> = (0..3000)
            .map(|i| LivePoint {
                t_ms: f64::from(i) * 100.0,
                bps: if i % 2 == 0 { 1e6 } else { 3e6 },
                direction: Direction::Upload,
            })
            .collect();
        let t = thin(&pts, 300);
        assert_eq!(t.len(), 300);
        assert!(t.iter().all(|p| (p.bps - 2e6).abs() < 1.0));
        assert_eq!(thin(&pts[..10], 300).len(), 10);
    }

    #[test]
    fn finds_nearest() {
        let pts: Vec<_> = [0.0, 100.0, 200.0]
            .iter()
            .map(|&t| p(t, Direction::Download))
            .collect();
        assert_eq!(nearest(&pts, -5.0), Some(0));
        assert_eq!(nearest(&pts, 40.0), Some(0));
        assert_eq!(nearest(&pts, 60.0), Some(1));
        assert_eq!(nearest(&pts, 500.0), Some(2));
        assert_eq!(nearest(&[], 1.0), None);
    }
}
