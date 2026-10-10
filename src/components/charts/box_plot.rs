//! Horizontal latency box plots on one shared millisecond axis (SPEC §7.4):
//! idle, during download and during upload, each with its jitter.

use leptos::html;
use leptos::prelude::*;

use super::scale::{Linear, tick_label, ticks_from_zero};
use super::{ChartTooltip, Series, Tip, TipRow, ValuesTable, pointer_xy, use_width};
use crate::model::stats::{BoxPlot, box_plot};
use crate::model::units::{PLACEHOLDER, format_ms};

/// One row of the chart.
#[derive(Debug, Clone, PartialEq)]
pub struct BoxRow {
    pub label: String,
    pub series: Series,
    pub values: Vec<f64>,
    pub jitter_ms: Option<f64>,
}

const ROW_H: f64 = 58.0;
const BOX_H: f64 = 16.0;
const LABEL_Y: f64 = 16.0;
const BOX_Y: f64 = 28.0;
const M_X: f64 = 8.0;
const AXIS_H: f64 = 26.0;

fn ms(v: f64) -> String {
    let (s, u) = format_ms(v);
    format!("{s} {u}")
}

fn opt_ms(v: Option<f64>) -> String {
    v.map_or_else(|| PLACEHOLDER.to_string(), ms)
}

#[component]
pub fn BoxPlotChart(
    #[prop(into)] rows: Signal<Vec<BoxRow>>,
    /// Accessible name of the chart.
    #[prop(default = "Latency distribution: idle, during download and during upload")]
    aria_label: &'static str,
    /// Header of the values table's first column.
    #[prop(default = "Condition")]
    row_header: &'static str,
) -> impl IntoView {
    let node = NodeRef::<html::Div>::new();
    let width = use_width(node, 480.0);
    let (tip, set_tip) = signal(None::<Tip>);

    let geo = Memo::new(move |_| {
        let w = width.get().max(200.0);
        rows.with(|rows| {
            let boxes: Vec<Option<BoxPlot>> = rows.iter().map(|r| box_plot(&r.values)).collect();
            let max = boxes.iter().flatten().map(|b| b.max).fold(0.0, f64::max);
            let ticks = ticks_from_zero(max, ((w / 110.0) as usize).clamp(2, 6));
            let top = *ticks.last().unwrap_or(&1.0);
            BoxGeo {
                w,
                x: Linear::new((0.0, top), (M_X, w - M_X)),
                ticks,
                boxes,
            }
        })
    });
    let height = move || rows.with(|r| r.len() as f64 * ROW_H + AXIS_H);

    let on_move = move |ev: leptos::ev::PointerEvent| {
        let Some((_, py)) = pointer_xy(&ev) else {
            return;
        };
        let i = (py / ROW_H).floor();
        let g = geo.get_untracked();
        let found = rows.with_untracked(|rows| {
            let i = usize::try_from(i as i64).ok()?;
            let row = rows.get(i)?;
            let b = (*g.boxes.get(i)?)?;
            Some((i, row.clone(), b))
        });
        set_tip.set(found.map(|(i, row, b)| Tip {
            x: g.x.map(b.q3),
            y: i as f64 * ROW_H + 4.0,
            title: row.label.clone(),
            rows: vec![
                TipRow {
                    series: Some(row.series),
                    value: ms(b.median),
                    label: "median".into(),
                },
                TipRow {
                    series: None,
                    value: format!("{} – {}", format_ms(b.q1).0, ms(b.q3)),
                    label: "middle half".into(),
                },
                TipRow {
                    series: None,
                    value: format!("{} – {}", format_ms(b.min).0, ms(b.max)),
                    label: "range".into(),
                },
                TipRow {
                    series: None,
                    value: opt_ms(row.jitter_ms),
                    label: "jitter".into(),
                },
            ],
        }));
    };

    let table_rows = Signal::derive(move || {
        rows.with(|rows| {
            rows.iter()
                .map(|r| {
                    let b = box_plot(&r.values);
                    vec![
                        r.label.clone(),
                        r.values.len().to_string(),
                        opt_ms(b.map(|b| b.min)),
                        opt_ms(b.map(|b| b.median)),
                        opt_ms(b.map(|b| b.max)),
                        opt_ms(r.jitter_ms),
                    ]
                })
                .collect()
        })
    });

    view! {
        <div class="chart-wrap" node_ref=node>
            <svg
                class="chart-svg"
                width=move || geo.with(|g| g.w)
                height=height
                viewBox=move || geo.with(|g| format!("0 0 {} {}", g.w, height()))
                role="img"
                aria-label=aria_label
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
                        .map(|(i, &t)| {
                            let x = g.x.map(t);
                            // The unit rides on the last tick, anchored inside the plot.
                            let (label, anchor) = if i == last {
                                (format!("{} ms", tick_label(t)), "end")
                            } else if i == 0 {
                                (tick_label(t), "start")
                            } else {
                                (tick_label(t), "middle")
                            };
                            view! {
                                <line class="grid" x1=x x2=x y1=BOX_Y - 6.0 y2=h - AXIS_H></line>
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
                        .zip(g.boxes.clone())
                        .enumerate()
                        .map(|(i, (row, b))| {
                            let y0 = i as f64 * ROW_H;
                            let caption = match row.jitter_ms {
                                Some(j) => format!("{} · jitter {}", row.label, ms(j)),
                                None => row.label.clone(),
                            };
                            let marks = b.map(|b| {
                                let (x0, x1) = (g.x.map(b.min), g.x.map(b.max));
                                let (q1, q3) = (g.x.map(b.q1), g.x.map(b.q3));
                                let m = g.x.map(b.median);
                                let cy = y0 + BOX_Y + BOX_H / 2.0;
                                view! {
                                    <line class="whisker" x1=x0 x2=q1 y1=cy y2=cy></line>
                                    <line class="whisker" x1=q3 x2=x1 y1=cy y2=cy></line>
                                    <line class="whisker-cap" x1=x0 x2=x0 y1=cy - 5.0 y2=cy + 5.0></line>
                                    <line class="whisker-cap" x1=x1 x2=x1 y1=cy - 5.0 y2=cy + 5.0></line>
                                    <rect
                                        class="box"
                                        x=q1
                                        y=y0 + BOX_Y
                                        width=(q3 - q1).max(2.0)
                                        height=BOX_H
                                        rx=4
                                    ></rect>
                                    <line class="median" x1=m x2=m y1=y0 + BOX_Y - 3.0 y2=y0 + BOX_Y + BOX_H + 3.0></line>
                                }
                            });
                            let empty = b.is_none();
                            view! {
                                <g class=format!("box-row {}", row.series.class())>
                                    <text class="row-label" x=M_X y=y0 + LABEL_Y>
                                        {caption}
                                        {empty.then_some(" · no samples")}
                                    </text>
                                    {marks}
                                </g>
                            }
                        })
                        .collect_view()
                }}
            </svg>
            <ChartTooltip tip=tip width=width/>
        </div>
        <ValuesTable
            summary="Latency values"
            headers=vec![row_header, "Samples", "Min", "Median", "Max", "Jitter"].into_iter().map(String::from).collect()
            rows=table_rows
        />
    }
}

#[derive(Debug, Clone, PartialEq)]
struct BoxGeo {
    w: f64,
    x: Linear,
    ticks: Vec<f64>,
    boxes: Vec<Option<BoxPlot>>,
}
