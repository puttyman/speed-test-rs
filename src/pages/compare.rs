//! `/compare?labels=a,b` or `?ids=…` (SPEC §7.7): two to four labels (the
//! median of their last 5 runs) or runs side by side, with delta badges
//! against the first, the best value per metric highlighted and overlaid
//! charts in one categorical color per item. Without parameters it shows a
//! label picker. The URL is the shareable state.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_meta::Title;
use leptos_router::hooks::{use_navigate, use_query_map};
use uuid::Uuid;

use crate::components::charts::{BoxPlotChart, BoxRow, Series, StripPlot, StripRow};
use crate::components::icons;
use crate::components::motion::reveal_style;
use crate::model::history::{CompareItem, CompareKind, CompareResult, Delta, LabelSummary};
use crate::model::result::{RunResult, Summary};
use crate::model::units::{PLACEHOLDER, Unit, format_ms, format_percent, format_rate};
use crate::pages::api::{self, CompareTarget};
use crate::pages::history::{COMPARE_MAX, COMPARE_MIN, toggle_selection};
use crate::ui_state::use_ui_state;

// ---------------------------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------------------------

/// A compared metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    Download,
    Upload,
    Latency,
    Jitter,
    DownLoaded,
    UpLoaded,
    Loss,
}

impl Metric {
    pub const ALL: [Metric; 7] = [
        Metric::Download,
        Metric::Upload,
        Metric::Latency,
        Metric::Jitter,
        Metric::DownLoaded,
        Metric::UpLoaded,
        Metric::Loss,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Metric::Download => "Download",
            Metric::Upload => "Upload",
            Metric::Latency => "Latency",
            Metric::Jitter => "Jitter",
            Metric::DownLoaded => "Latency during download",
            Metric::UpLoaded => "Latency during upload",
            Metric::Loss => "Packet loss",
        }
    }

    /// Higher is better (throughput) or lower is better.
    pub fn higher_is_better(self) -> bool {
        matches!(self, Metric::Download | Metric::Upload)
    }

    pub fn value(self, s: &Summary) -> Option<f64> {
        match self {
            Metric::Download => s.download_bps,
            Metric::Upload => s.upload_bps,
            Metric::Latency => s.latency_ms,
            Metric::Jitter => s.jitter_ms,
            Metric::DownLoaded => s.down_loaded_latency_ms,
            Metric::UpLoaded => s.up_loaded_latency_ms,
            Metric::Loss => s.packet_loss_ratio,
        }
    }

    pub fn delta(self, d: &crate::model::history::SummaryDeltas) -> Option<Delta> {
        match self {
            Metric::Download => d.download_bps,
            Metric::Upload => d.upload_bps,
            Metric::Latency => d.latency_ms,
            Metric::Jitter => d.jitter_ms,
            Metric::DownLoaded => d.down_loaded_latency_ms,
            Metric::UpLoaded => d.up_loaded_latency_ms,
            Metric::Loss => d.packet_loss_ratio,
        }
    }

    /// Number and unit.
    pub fn format(self, v: Option<f64>, unit: Unit) -> (String, &'static str) {
        let v = v.unwrap_or(f64::NAN);
        match self {
            Metric::Download | Metric::Upload => format_rate(v, unit),
            Metric::Loss if !v.is_finite() => (PLACEHOLDER.to_string(), ""),
            Metric::Loss => format_percent(v),
            _ => format_ms(v),
        }
    }
}

/// The text of a delta badge with its arrow: "+23% ↑" for throughput,
/// "−8 ms ↓" for latency (abs), "+0.5 pts ↑" for loss. `None` for no change.
pub fn delta_text(metric: Metric, d: &Delta) -> Option<String> {
    if d.abs == 0.0 || !d.abs.is_finite() {
        return None;
    }
    let arrow = if d.abs > 0.0 { "↑" } else { "↓" };
    let sign = if d.abs > 0.0 { "+" } else { "\u{2212}" };
    let body = match metric {
        Metric::Download | Metric::Upload => match d.ratio {
            // Small changes keep a decimal, so they never read "+0%".
            Some(r) if (r * 100.0).abs() < 1.0 => format!("{:.1}%", (r * 100.0).abs()),
            Some(r) => format!("{:.0}%", (r * 100.0).abs()),
            None => return Some(format!("new {arrow}")),
        },
        Metric::Loss => format!("{:.1} pts", (d.abs * 100.0).abs()),
        _ => {
            let (s, u) = format_ms(d.abs.abs());
            format!("{s} {u}")
        }
    };
    // A rounded-away change ("+0%") still reads as a change in the right direction.
    Some(format!("{sign}{body} {arrow}"))
}

/// Index of the best item for a metric (`None` if fewer than two have it or all tie).
pub fn best_index(metric: Metric, items: &[Summary]) -> Option<usize> {
    let vals: Vec<(usize, f64)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, s)| metric.value(s).filter(|v| v.is_finite()).map(|v| (i, v)))
        .collect();
    if vals.len() < 2 {
        return None;
    }
    let pick = if metric.higher_is_better() {
        vals.iter().copied().max_by(|a, b| a.1.total_cmp(&b.1))
    } else {
        vals.iter().copied().min_by(|a, b| a.1.total_cmp(&b.1))
    }?;
    let ties = vals.iter().filter(|(_, v)| *v == pick.1).count();
    (ties < vals.len()).then_some(pick.0)
}

/// Parses `?labels=` / `?ids=` into a compare target.
pub fn parse_target(labels: Option<&str>, ids: Option<&str>) -> Option<CompareTarget> {
    let split = |v: &str| -> Vec<String> {
        v.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    };
    if let Some(l) = labels.filter(|l| !l.trim().is_empty()) {
        return Some(CompareTarget::Labels(split(l)));
    }
    let ids: Vec<Uuid> = split(ids?)
        .iter()
        .filter_map(|s| Uuid::parse_str(s).ok())
        .collect();
    (!ids.is_empty()).then_some(CompareTarget::Ids(ids))
}

/// Name of an item: its label, or for an unlabelled run "Run 1" etc.
fn item_name(item: &CompareItem, i: usize) -> String {
    item.label
        .clone()
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| format!("Run {}", i + 1))
}

// ---------------------------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------------------------

#[component]
pub fn ComparePage() -> impl IntoView {
    let query = use_query_map();
    let target = Memo::new(move |_| {
        query.with(|q| parse_target(q.get("labels").as_deref(), q.get("ids").as_deref()))
    });
    view! {
        <Title text="Compare · Speed Test"/>
        <div class="compare-page">
            {move || match target.get() {
                Some(t) if t.len() >= COMPARE_MIN => view! { <CompareView target=t/> }.into_any(),
                _ => view! { <ComparePicker/> }.into_any(),
            }}
        </div>
    }
}

/// Label picker shown without parameters.
#[component]
fn ComparePicker() -> impl IntoView {
    let labels = RwSignal::new(None::<Result<Vec<LabelSummary>, String>>);
    let picked = RwSignal::new(Vec::<String>::new());
    let navigate = use_navigate();
    let ui = use_ui_state();
    Effect::new(move |_| {
        spawn_local(async move {
            labels.set(Some(api::all_labels(None).await));
        });
    });
    let go = move |_| {
        let t = CompareTarget::Labels(picked.get_untracked());
        navigate(&format!("/compare?{}", t.query()), Default::default());
    };
    view! {
        <header class="page-head">
            <div>
                <h1 class="page-title">"Compare"</h1>
                <p class="page-sub">
                    "Pick two to four labels. Each one stands for the median of its last 5 runs."
                </p>
            </div>
        </header>
        <section class="card picker-card">
            {move || match labels.get() {
                None => view! { <p class="muted">"Loading labels…"</p> }.into_any(),
                Some(Err(e)) => view! { <p class="callout callout-warn">{format!("Couldn't load labels: {e}")}</p> }.into_any(),
                Some(Ok(list)) if list.len() < 2 => {
                    view! {
                        <div class="empty-state">
                            <h2 class="empty-title">"Nothing to compare yet"</h2>
                            <p class="muted">
                                "Compare needs runs under at least two labels, for example one per room or access point."
                            </p>
                            <a class="pill-button" href="/">
                                "Run a test"
                            </a>
                        </div>
                    }
                        .into_any()
                }
                Some(Ok(list)) => {
                    view! {
                        <ul class="label-picks">
                            {list
                                .into_iter()
                                .map(|l| {
                                    let name = l.label.clone();
                                    let n2 = name.clone();
                                    let checked = Memo::new(move |_| picked.with(|p| p.contains(&n2)));
                                    let full = Memo::new(move |_| picked.with(|p| p.len() >= COMPARE_MAX));
                                    view! {
                                        <li>
                                            <label class="label-pick" class:is-selected=move || checked.get()>
                                                <span class="check">
                                                    <input
                                                        type="checkbox"
                                                        prop:checked=move || checked.get()
                                                        disabled=move || full.get() && !checked.get()
                                                        on:change=move |_| {
                                                            picked.update(|p| {
                                                                toggle_selection(p, name.clone());
                                                            })
                                                        }
                                                    />
                                                    <span class="check-box" aria-hidden="true">{icons::check()}</span>
                                                </span>
                                                <span class="pick-text">
                                                    <span class="pick-name">{l.label.clone()}</span>
                                                    <span class="pick-meta">
                                                        {move || {
                                                            let unit = ui.units.get();
                                                            let (d, du) = format_rate(l.median_download_bps.unwrap_or(f64::NAN), unit);
                                                            format!(
                                                                "{} run{} · median {d} {du}",
                                                                l.runs,
                                                                if l.runs == 1 { "" } else { "s" },
                                                            )
                                                        }}
                                                    </span>
                                                </span>
                                            </label>
                                        </li>
                                    }
                                })
                                .collect_view()}
                        </ul>
                        <div class="picker-actions">
                            <button
                                type="button"
                                class="pill-button"
                                disabled=move || !(COMPARE_MIN..=COMPARE_MAX).contains(&picked.with(Vec::len))
                                on:click=go.clone()
                            >
                                {move || {
                                    let n = picked.with(Vec::len);
                                    if n < COMPARE_MIN {
                                        "Pick at least two".to_string()
                                    } else {
                                        format!("Compare {n} labels")
                                    }
                                }}
                            </button>
                        </div>
                    }
                        .into_any()
                }
            }}
        </section>
    }
}

/// The comparison itself.
#[component]
fn CompareView(target: CompareTarget) -> impl IntoView {
    let data = RwSignal::new(None::<Result<CompareResult, String>>);
    // Each item's runs with their points (for the overlaid charts).
    let runs = RwSignal::new(Vec::<Vec<RunResult>>::new());
    let copied = RwSignal::new(false);
    let t2 = target.clone();
    Effect::new(move |_| {
        let t = t2.clone();
        spawn_local(async move {
            let res = api::compare(&t, false).await;
            if let Ok(r) = &res {
                let ids: Vec<Vec<Uuid>> = r.items.iter().map(|i| i.run_ids.clone()).collect();
                data.set(Some(res));
                let mut all = Vec::new();
                for item in ids {
                    let mut got = Vec::new();
                    for id in item {
                        if let Ok(r) = api::get_run(id).await {
                            got.push(r);
                        }
                    }
                    all.push(got);
                }
                runs.set(all);
            } else {
                data.set(Some(res));
            }
        });
    });

    let share = move |_| {
        #[cfg(feature = "hydrate")]
        {
            copied.set(crate::components::result_view::copy_page_url());
            set_timeout(
                move || copied.set(false),
                std::time::Duration::from_millis(2000),
            );
        }
    };
    let kind_label = match &target {
        CompareTarget::Labels(_) => "labels",
        CompareTarget::Ids(_) => "runs",
    };

    view! {
        <header class="page-head">
            <div>
                <h1 class="page-title">"Compare"</h1>
                <p class="page-sub">
                    {format!(
                        "{} {kind_label} side by side. Deltas are against the first one.",
                        target.len(),
                    )}
                </p>
            </div>
            <div class="export-links">
                <button type="button" class="pill-button pill-sm pill-ghost" on:click=share>
                    {icons::share()}
                    {move || if copied.get() { "Link copied" } else { "Copy link" }}
                </button>
                <a class="pill-button pill-sm pill-ghost" href="/compare">
                    "Change"
                </a>
            </div>
        </header>
        {move || match data.get() {
            None => {
                view! {
                    <div class="history-skeleton" aria-busy="true" aria-label="Loading">
                        <div class="skeleton-row tall"></div>
                    </div>
                }
                    .into_any()
            }
            Some(Err(e)) => {
                view! {
                    <section class="empty-state card">
                        <h2 class="empty-title">"Couldn't compare these"</h2>
                        <p class="muted">{e}</p>
                        <a class="pill-button" href="/history">
                            "Back to history"
                        </a>
                    </section>
                }
                    .into_any()
            }
            Some(Ok(res)) => {
                let items = res.items.clone();
                let summaries: Vec<Summary> = items.iter().map(|i| i.summary).collect();
                let names: Vec<String> = items.iter().enumerate().map(|(i, it)| item_name(it, i)).collect();
                let names2 = names.clone();
                let names3 = names.clone();
                let strip = move |down: bool| {
                    let names = names2.clone();
                    let summaries = summaries.clone();
                    Signal::derive(move || {
                        runs.with(|runs| {
                            names
                                .iter()
                                .enumerate()
                                .map(|(i, n)| StripRow {
                                    label: n.clone(),
                                    series: Series::slot(i),
                                    values: runs
                                        .get(i)
                                        .map(|rs| {
                                            rs.iter()
                                                .flat_map(|r| if down { &r.points.download } else { &r.points.upload })
                                                .map(|p| p.bps)
                                                .collect()
                                        })
                                        .unwrap_or_default(),
                                    value: summaries.get(i).and_then(|s| if down { s.download_bps } else { s.upload_bps }),
                                })
                                .collect()
                        })
                    })
                };
                let box_rows = Signal::derive(move || {
                    runs.with(|runs| {
                        names3
                            .iter()
                            .enumerate()
                            .map(|(i, n)| {
                                let values: Vec<f64> = runs
                                    .get(i)
                                    .map(|rs| rs.iter().flat_map(|r| r.points.latency.iter().copied()).collect())
                                    .unwrap_or_default();
                                BoxRow {
                                    label: n.clone(),
                                    series: Series::slot(i),
                                    jitter_ms: crate::model::stats::jitter(&values),
                                    values,
                                }
                            })
                            .collect()
                    })
                });
                let cols = items.len();
                view! {
                    <section
                        class="compare-grid"
                        style=format!("--cols:{cols}")
                        aria-label="Compared items"
                    >
                        {items
                            .iter()
                            .enumerate()
                            .map(|(i, item)| {
                                view! { <CompareColumn item=item.clone() index=i name=names[i].clone() all=res.items.iter().map(|x| x.summary).collect()/> }
                            })
                            .collect_view()}
                    </section>
                    <section class="details" aria-labelledby="compare-charts">
                        <h2 id="compare-charts" class="section-title">"Overlaid"</h2>
                        <div class="details-grid">
                            <article class="card detail-card reveal" style=reveal_style(6)>
                                <h3 class="card-title">"Download"</h3>
                                <p class="card-sub">"Every request of every run; the tick is the headline value."</p>
                                <StripPlot rows=strip(true) what="Download"/>
                            </article>
                            <article class="card detail-card reveal" style=reveal_style(7)>
                                <h3 class="card-title">"Upload"</h3>
                                <p class="card-sub">"Every request of every run; the tick is the headline value."</p>
                                <StripPlot rows=strip(false) what="Upload"/>
                            </article>
                            <article class="card detail-card detail-wide reveal" style=reveal_style(8)>
                                <h3 class="card-title">"Idle latency"</h3>
                                <p class="card-sub">"Round trips of every run, with each item's jitter."</p>
                                <BoxPlotChart
                                    rows=box_rows
                                    aria_label="Idle latency distribution of each compared item"
                                    row_header="Item"
                                />
                            </article>
                        </div>
                    </section>
                }
                    .into_any()
            }
        }}
    }
}

/// One compared item: its metrics, delta badges and best markers.
#[component]
fn CompareColumn(
    item: CompareItem,
    index: usize,
    name: String,
    all: Vec<Summary>,
) -> impl IntoView {
    let ui = use_ui_state();
    let series = Series::slot(index);
    let sub = match item.kind {
        CompareKind::Label => format!(
            "Median of {} run{}",
            item.runs,
            if item.runs == 1 { "" } else { "s" }
        ),
        CompareKind::Run => "Single run".to_string(),
    };
    let link = match item.kind {
        CompareKind::Run => item.run_ids.first().map(|id| format!("/runs/{id}")),
        CompareKind::Label => None,
    };
    let first = index == 0;
    let rows = Metric::ALL
        .into_iter()
        // Only metrics at least one item has (e.g. no loaded latency for Quick runs).
        .filter(|m| all.iter().any(|s| m.value(s).is_some()))
        .map(|m| {
            let value = m.value(&item.summary);
            let best = best_index(m, &all) == Some(index);
            let delta = item.deltas.as_ref().and_then(|d| m.delta(d));
            let badge = delta.and_then(|d| {
                delta_text(m, &d).map(|text| {
                    let class = match d.better {
                        Some(true) => "delta is-better",
                        Some(false) => "delta is-worse",
                        None => "delta",
                    };
                    let verdict = match d.better {
                        Some(true) => ", better",
                        Some(false) => ", worse",
                        None => "",
                    };
                    view! {
                        <span class=class>
                            {text}
                            <span class="visually-hidden">{verdict}</span>
                        </span>
                    }
                })
            });
            view! {
                <div class="cmp-metric" class:is-best=best>
                    <dt>{m.label()}</dt>
                    <dd>
                        <span class="cmp-value">
                            {move || {
                                let (v, u) = m.format(value, ui.units.get());
                                let u = if value.is_some() { u } else { "" };
                                view! {
                                    <strong>{v}</strong>
                                    " "
                                    <span class="unit">{u}</span>
                                }
                            }}
                        </span>
                        {best.then(|| view! { <span class="best-tag">"Best"</span> })}
                        {badge}
                        {(first && value.is_some()).then(|| view! { <span class="delta is-ref">"Reference"</span> })}
                    </dd>
                </div>
            }
        })
        .collect_view();
    view! {
        <article class=format!("card cmp-col reveal {}", series.class()) style=reveal_style(index)>
            <header class="cmp-head">
                <span class="swatch-key" aria-hidden="true"></span>
                <div class="cmp-titles">
                    <h2 class="cmp-name">
                        {match link {
                            Some(href) => view! { <a href=href>{name.clone()}</a> }.into_any(),
                            None => view! { <span>{name.clone()}</span> }.into_any(),
                        }}
                    </h2>
                    <p class="cmp-sub">{sub}</p>
                </div>
            </header>
            <dl class="cmp-metrics">{rows}</dl>
        </article>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(abs: f64, ratio: Option<f64>, better: Option<bool>) -> Delta {
        Delta { abs, ratio, better }
    }

    #[test]
    fn delta_formatting() {
        assert_eq!(
            delta_text(Metric::Download, &delta(23e6, Some(0.23), Some(true))).as_deref(),
            Some("+23% ↑")
        );
        assert_eq!(
            delta_text(Metric::Latency, &delta(-8.0, Some(-0.4), Some(true))).as_deref(),
            Some("\u{2212}8.00 ms ↓")
        );
        assert_eq!(
            delta_text(Metric::Loss, &delta(0.005, None, Some(false))).as_deref(),
            Some("+0.5 pts ↑")
        );
        assert_eq!(
            delta_text(Metric::Upload, &delta(0.0, Some(0.0), None)),
            None
        );
        assert_eq!(
            delta_text(Metric::Upload, &delta(5.0, None, Some(true))).as_deref(),
            Some("new ↑")
        );
    }

    #[test]
    fn best_values() {
        let s = |d: Option<f64>, l: Option<f64>| Summary {
            download_bps: d,
            latency_ms: l,
            ..Default::default()
        };
        let items = [
            s(Some(100.0), Some(5.0)),
            s(Some(300.0), Some(9.0)),
            s(None, Some(3.0)),
        ];
        assert_eq!(best_index(Metric::Download, &items), Some(1));
        assert_eq!(best_index(Metric::Latency, &items), Some(2));
        assert_eq!(best_index(Metric::Upload, &items), None);
        let tie = [s(Some(1.0), None), s(Some(1.0), None)];
        assert_eq!(best_index(Metric::Download, &tie), None);
    }

    #[test]
    fn targets_from_the_query() {
        assert_eq!(
            parse_target(Some("Desk, Bedroom"), None),
            Some(CompareTarget::Labels(vec!["Desk".into(), "Bedroom".into()]))
        );
        let id = Uuid::from_u128(7);
        assert_eq!(
            parse_target(None, Some(&format!("{id},bogus"))),
            Some(CompareTarget::Ids(vec![id]))
        );
        assert_eq!(parse_target(None, None), None);
        assert_eq!(parse_target(Some(" "), Some("")), None);
    }
}
