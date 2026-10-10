//! Compare's throughput chart (SPEC §7.7): one row per compared item on a
//! shared rate axis, every sample as a dot in the item's color and the
//! item's headline value as a tick.

use leptos::html;
use leptos::prelude::*;

use super::scale::{Linear, rate_axis, spread, tick_label, ticks_from_zero};
use super::{ChartTooltip, Series, Tip, TipRow, ValuesTable, pointer_xy, use_width};
use crate::model::units::{PLACEHOLDER, format_rate};
use crate::ui_state::use_ui_state;

/// One compared item.
#[derive(Debug, Clone, PartialEq)]
pub struct StripRow {
    pub label: String,
    pub series: Series,
    /// Every sample (bps).
    pub values: Vec<f64>,
    /// The headline value (bps), drawn as a tick.
    pub value: Option<f64>,
}

const ROW_H: f64 = 56.0;
const LABEL_Y: f64 = 16.0;
const STRIP_Y: f64 = 38.0;
const M_X: f64 = 8.0;
const AXIS_H: f64 = 26.0;
const R: f64 = 3.5;

#[component]
pub fn StripPlot(
    #[prop(into)] rows: Signal<Vec<StripRow>>,
    /// e.g. "Download"
    what: &'static str,
) -> impl IntoView {
    let ui = use_ui_state();
    let node = NodeRef::<html::Div>::new();
    let width = use_width(node, 480.0);
    let (tip, set_tip) = signal(None::<Tip>);
    let fmt = move |v: Option<f64>| {
        v.map_or_else(
            || PLACEHOLDER.to_string(),
            |v| {
                let (s, u) = format_rate(v, ui.units.get());
                format!("{s} {u}")
            },
        )
    };

    let geo = Memo::new(move |_| {
        let w = width.get().max(200.0);
        let unit = ui.units.get();
        rows.with(|rows| {
            // Far outliers don't stretch the scale: they sit on the right edge.
            let all: Vec<f64> = rows
                .iter()
                .flat_map(|r| r.values.iter().copied().chain(r.value))
                .filter(|v| v.is_finite())
                .collect();
            let reference = rows.iter().filter_map(|r| r.value).fold(0.0, f64::max);
            let max = super::dot_plot::scale_max(&all, Some(reference).filter(|r| *r > 0.0))
                .max(reference);
            let (per, unit_label) = rate_axis(max, unit);
            let ticks = ticks_from_zero(max / per, ((w / 110.0) as usize).clamp(2, 6));
            let top = *ticks.last().unwrap_or(&1.0);
            StripGeo {
                w,
                x: Linear::new((0.0, top * per), (M_X, w - M_X)),
                ticks: ticks.iter().map(|t| (t * per, tick_label(*t))).collect(),
                unit_label,
            }
        })
    });
    let height = move || rows.with(|r| r.len() as f64 * ROW_H + AXIS_H);

    let on_move = move |ev: leptos::ev::PointerEvent| {
        let Some((_, py)) = pointer_xy(&ev) else {
            return;
        };
        let g = geo.get_untracked();
        let i = (py / ROW_H).floor();
        let found = rows.with_untracked(|rows| {
            let i = usize::try_from(i as i64).ok()?;
            rows.get(i).cloned().map(|r| (i, r))
        });
        set_tip.set(found.map(|(i, r)| Tip {
            x: g.x.map(r.value.unwrap_or(0.0)),
            y: i as f64 * ROW_H + 4.0,
            title: r.label.clone(),
            rows: vec![
                TipRow {
                    series: Some(r.series),
                    value: fmt(r.value),
                    label: what.to_lowercase(),
                },
                TipRow {
                    series: None,
                    value: r.values.len().to_string(),
                    label: "samples".into(),
                },
            ],
        }));
    };

    let table_rows = Signal::derive(move || {
        rows.with(|rows| {
            rows.iter()
                .map(|r| {
                    let min = r.values.iter().copied().reduce(f64::min);
                    let max = r.values.iter().copied().reduce(f64::max);
                    vec![
                        r.label.clone(),
                        fmt(r.value),
                        r.values.len().to_string(),
                        fmt(min),
                        fmt(max),
                    ]
                })
                .collect()
        })
    });

    view! {
        <div class="chart-wrap" node_ref=node>
            <svg
                class="chart-svg strip-plot"
                width=move || geo.with(|g| g.w)
                height=height
                viewBox=move || geo.with(|g| format!("0 0 {} {}", g.w, height()))
                role="img"
                aria-label=format!("{what} of each compared item")
                on:pointermove=on_move
                on:pointerleave=move |_| set_tip.set(None)
            >
                {move || {
                    let g = geo.get();
                    let h = height();
                    let last = g.ticks.len().saturating_sub(1);
                    g.ticks
                        .iter()
                        .enumerate()
                        .map(|(i, (t, label))| {
                            let x = g.x.map(*t);
                            let (label, anchor) = if i == last {
                                (format!("{label} {}", g.unit_label), "end")
                            } else if i == 0 {
                                (label.clone(), "start")
                            } else {
                                (label.clone(), "middle")
                            };
                            view! {
                                <line class="grid" x1=x x2=x y1=STRIP_Y - 14.0 y2=h - AXIS_H></line>
                                <text class="axis-label" x=x y=h - 8.0 text-anchor=anchor>
                                    {label}
                                </text>
                            }
                        })
                        .collect_view()
                }}
                {move || {
                    let g = geo.get();
                    rows.get()
                        .into_iter()
                        .enumerate()
                        .map(|(i, r)| {
                            let y0 = i as f64 * ROW_H;
                            let cy = y0 + STRIP_Y;
                            let caption = format!("{} · {}", r.label, fmt(r.value));
                            let dots = r
                                .values
                                .iter()
                                .enumerate()
                                .map(|(k, v)| {
                                    let x = g.x.map(*v);
                                    let edge = g.w - M_X;
                                    let y = cy + spread(k) * 8.0;
                                    if x > edge + 0.5 {
                                        // Off the scale: a marker pointing right at the edge.
                                        let d = format!("M{:.1} {:.1}l-8 -5v10z", edge + 2.0, y);
                                        view! { <path class="dot dot-clipped" d=d></path> }.into_any()
                                    } else {
                                        view! { <circle class="dot" cx=x cy=y r=R></circle> }.into_any()
                                    }
                                })
                                .collect_view();
                            let tick = r
                                .value
                                .map(|v| {
                                    let x = g.x.map(v);
                                    view! { <line class="median" x1=x x2=x y1=cy - 13.0 y2=cy + 13.0></line> }
                                });
                            view! {
                                <g class=format!("strip-row {}", r.series.class())>
                                    <text class="row-label" x=M_X y=y0 + LABEL_Y>
                                        {caption}
                                    </text>
                                    {dots}
                                    {tick}
                                </g>
                            }
                        })
                        .collect_view()
                }}
            </svg>
            <ChartTooltip tip=tip width=width/>
        </div>
        <ValuesTable
            summary=format!("{what} values")
            headers=vec!["Item", what, "Samples", "Min", "Max"].into_iter().map(String::from).collect()
            rows=table_rows
        />
    }
}

#[derive(Debug, Clone, PartialEq)]
struct StripGeo {
    w: f64,
    x: Linear,
    ticks: Vec<(f64, String)>,
    unit_label: &'static str,
}
