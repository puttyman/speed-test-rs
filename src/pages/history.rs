//! `/history` (SPEC §7.7): saved runs grouped by label, newest first, with
//! filters, inline rename, delete, export, and a 2–4 item selection to
//! compare. Group tests appear as one expandable row each.

use std::collections::{HashMap, HashSet};

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_meta::Title;
use uuid::Uuid;

use crate::components::group_view::GroupResultView;
use crate::components::icons;
use crate::components::label_picker::clean_label;
use crate::components::result_view::{SharedBadge, format_when, profile_label, shared_text};
use crate::components::segmented::Segmented;
use crate::model::history::{GroupRecord, LabelSummary, RunSummary};
use crate::model::result::{Concurrency, Transport};
use crate::model::units::{PLACEHOLDER, format_ms, format_rate};
use crate::pages::api::{self, CompareTarget, RunsFilter};
use crate::ui_state::use_ui_state;

const ARROW_DOWNLOAD: &str = "\u{2193}";
const ARROW_UPLOAD: &str = "\u{2191}";

/// Runs per page.
const PAGE_SIZE: u32 = 30;
/// Compare takes 2 to 4 items.
pub const COMPARE_MIN: usize = 2;
pub const COMPARE_MAX: usize = 4;

// ---------------------------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------------------------

/// Runs that share a label, newest first.
#[derive(Debug, Clone, PartialEq)]
pub struct LabelGroup {
    pub label: Option<String>,
    pub runs: Vec<RunSummary>,
}

impl LabelGroup {
    /// Stable key (`""` for unlabelled runs).
    pub fn key(&self) -> String {
        self.label.clone().unwrap_or_default()
    }
}

/// The member runs of one group test.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupTestRow {
    pub id: Uuid,
    pub runs: Vec<RunSummary>,
}

/// Splits newest-first runs into label groups (ordered by their newest run)
/// and group tests (one row per group id).
pub fn group_runs(runs: &[RunSummary]) -> (Vec<LabelGroup>, Vec<GroupTestRow>) {
    let mut labels: Vec<LabelGroup> = Vec::new();
    let mut groups: Vec<GroupTestRow> = Vec::new();
    for r in runs {
        if let Some(g) = r.group_id {
            match groups.iter_mut().find(|x| x.id == g) {
                Some(x) => x.runs.push(r.clone()),
                None => groups.push(GroupTestRow {
                    id: g,
                    runs: vec![r.clone()],
                }),
            }
            continue;
        }
        match labels.iter_mut().find(|x| x.label == r.label) {
            Some(x) => x.runs.push(r.clone()),
            None => labels.push(LabelGroup {
                label: r.label.clone(),
                runs: vec![r.clone()],
            }),
        }
    }
    (labels, groups)
}

/// Adds `item` to a compare selection, or removes it if present. Never grows
/// past [`COMPARE_MAX`]; returns whether the selection changed.
pub fn toggle_selection<T: PartialEq>(sel: &mut Vec<T>, item: T) -> bool {
    if let Some(i) = sel.iter().position(|x| *x == item) {
        sel.remove(i);
        true
    } else if sel.len() < COMPARE_MAX {
        sel.push(item);
        true
    } else {
        false
    }
}

fn rate_text(v: Option<f64>, unit: crate::model::units::Unit) -> (String, &'static str) {
    format_rate(v.unwrap_or(f64::NAN), unit)
}

fn ms_text(v: Option<f64>) -> String {
    v.map_or_else(
        || PLACEHOLDER.to_string(),
        |v| {
            let (s, u) = format_ms(v);
            format!("{s} {u}")
        },
    )
}

// ---------------------------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct History {
    runs: RwSignal<Vec<RunSummary>>,
    cursor: RwSignal<Option<String>>,
    loading: RwSignal<bool>,
    loaded: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    labels: RwSignal<Vec<LabelSummary>>,
    filter: Memo<RunsFilter>,
    collapsed: RwSignal<HashSet<String>>,
    sel_runs: RwSignal<Vec<Uuid>>,
    sel_labels: RwSignal<Vec<String>>,
    editing: RwSignal<Option<Uuid>>,
    confirm: RwSignal<Option<Uuid>>,
    expanded: RwSignal<HashMap<Uuid, Option<Result<GroupRecord, String>>>>,
    /// Bumped to reload the first page.
    epoch: RwSignal<u32>,
}

impl History {
    fn fetch(self, more: bool) {
        let filter = self.filter.get_untracked();
        let cursor = if more {
            self.cursor.get_untracked()
        } else {
            None
        };
        if more && cursor.is_none() {
            return;
        }
        self.loading.set(true);
        let epoch = self.epoch.get_untracked();
        spawn_local(async move {
            let page = api::list_runs(&filter, PAGE_SIZE, cursor.as_deref()).await;
            if self.epoch.get_untracked() != epoch {
                return;
            }
            match page {
                Ok(p) => {
                    if more {
                        self.runs.update(|r| r.extend(p.runs));
                    } else {
                        self.runs.set(p.runs);
                    }
                    self.cursor.set(p.next_cursor);
                    self.error.set(None);
                }
                Err(e) => self.error.set(Some(e)),
            }
            self.loading.set(false);
            self.loaded.set(true);
        });
    }

    fn fetch_labels(self) {
        let device = self.filter.with_untracked(|f| f.device.clone());
        spawn_local(async move {
            if let Ok(l) = api::all_labels(device.as_deref()).await {
                self.labels.set(l);
            }
        });
    }

    fn rename(self, id: Uuid, label: Option<String>) {
        self.editing.set(None);
        spawn_local(async move {
            match api::rename_run(id, label.as_deref()).await {
                Ok(updated) => {
                    self.runs.update(|runs| {
                        if let Some(r) = runs.iter_mut().find(|r| r.id == id) {
                            *r = updated;
                        }
                    });
                    self.fetch_labels();
                }
                Err(e) => self.error.set(Some(format!("Rename failed: {e}"))),
            }
        });
    }

    fn delete(self, id: Uuid) {
        self.confirm.set(None);
        spawn_local(async move {
            match api::delete_run(id).await {
                Ok(()) => {
                    self.runs.update(|r| r.retain(|r| r.id != id));
                    self.sel_runs.update(|s| s.retain(|x| *x != id));
                    self.fetch_labels();
                }
                Err(e) => self.error.set(Some(format!("Delete failed: {e}"))),
            }
        });
    }

    fn toggle_group(self, id: Uuid) {
        let open = self.expanded.with_untracked(|m| m.contains_key(&id));
        if open {
            self.expanded.update(|m| {
                m.remove(&id);
            });
            return;
        }
        self.expanded.update(|m| {
            m.insert(id, None);
        });
        spawn_local(async move {
            let rec = api::get_group(id).await;
            self.expanded.update(|m| {
                if let Some(slot) = m.get_mut(&id) {
                    *slot = Some(rec);
                }
            });
        });
    }
}

#[component]
pub fn HistoryPage() -> impl IntoView {
    let ui = use_ui_state();
    let info = RwSignal::new(None::<api::ServerInfo>);
    let scope = RwSignal::new("all".to_string());
    let label_filter = RwSignal::new(String::new());
    let from = RwSignal::new(String::new());
    let to = RwSignal::new(String::new());
    let exclude = RwSignal::new(false);
    let filter = Memo::new(move |_| RunsFilter {
        label: clean_label(&label_filter.get()),
        device: (scope.get() == "this")
            .then(|| ui.device_id.get())
            .filter(|d| !d.is_empty()),
        from: Some(from.get()).filter(|s| !s.is_empty()),
        to: Some(to.get()).filter(|s| !s.is_empty()),
        exclude_overlapped: exclude.get(),
    });
    let h = History {
        runs: RwSignal::new(Vec::new()),
        cursor: RwSignal::new(None),
        loading: RwSignal::new(false),
        loaded: RwSignal::new(false),
        error: RwSignal::new(None),
        labels: RwSignal::new(Vec::new()),
        filter,
        collapsed: RwSignal::new(HashSet::new()),
        sel_runs: RwSignal::new(Vec::new()),
        sel_labels: RwSignal::new(Vec::new()),
        editing: RwSignal::new(None),
        confirm: RwSignal::new(None),
        expanded: RwSignal::new(HashMap::new()),
        epoch: RwSignal::new(0),
    };

    Effect::new(move |_| {
        spawn_local(async move {
            match api::server_info().await {
                Ok(i) => info.set(Some(i)),
                Err(_) => info.set(Some(api::ServerInfo {
                    history: true,
                    ..Default::default()
                })),
            }
        });
    });
    // (Re)load when the filters change (and history is on).
    Effect::new(move |_| {
        let _ = filter.get();
        let on = info.with(|i| i.as_ref().map(|i| i.history));
        if on != Some(true) {
            return;
        }
        h.epoch.update(|e| *e += 1);
        h.fetch(false);
        h.fetch_labels();
    });

    let grouped = Memo::new(move |_| h.runs.with(|r| group_runs(r)));
    let label_options = Memo::new(move |_| {
        h.labels
            .with(|l| l.iter().map(|l| l.label.clone()).collect::<Vec<_>>())
    });
    let any_filter = Memo::new(move |_| filter.with(|f| *f != RunsFilter::default()));
    let reset = move |_| {
        scope.set("all".into());
        label_filter.set(String::new());
        from.set(String::new());
        to.set(String::new());
        exclude.set(false);
    };

    let history_off = Memo::new(move |_| info.with(|i| i.as_ref().is_some_and(|i| !i.history)));

    view! {
        <Title text="History · Speed Test"/>
        <div class="history-page">
            <header class="page-head">
                <div>
                    <h1 class="page-title">"History"</h1>
                    <p class="page-sub">
                        "Every finished run, grouped by label. Pick two to four to compare."
                    </p>
                </div>
                <Show when=move || !history_off.get()>
                    <div class="export-links" aria-label="Export">
                        <a
                            class="pill-button pill-sm pill-ghost"
                            href=move || filter.with(|f| f.export_url("csv"))
                            download=""
                            rel="external"
                        >
                            {icons::export()}
                            "CSV"
                        </a>
                        <a
                            class="pill-button pill-sm pill-ghost"
                            href=move || filter.with(|f| f.export_url("json"))
                            download=""
                            rel="external"
                        >
                            {icons::export()}
                            "JSON"
                        </a>
                    </div>
                </Show>
            </header>

            <Show
                when=move || !history_off.get()
                fallback=|| {
                    view! {
                        <section class="empty-state card">
                            <h2 class="empty-title">"History is turned off on this server"</h2>
                            <p class="muted">
                                "Runs are measured but not saved. To keep them, restart the server without "
                                <code>"--no-history"</code>
                                " (or with "
                                <code>"SPEEDTEST_HISTORY=true"</code>
                                "). Saved runs then appear here, grouped by label."
                            </p>
                            <a class="pill-button" href="/">
                                "Back to the test"
                            </a>
                        </section>
                    }
                }
            >
                <section class="filters" aria-label="Filters">
                    <label class="filter">
                        <span class="filter-label">"Label"</span>
                        <select
                            class="select-input"
                            prop:value=move || label_filter.get()
                            on:change=move |ev| label_filter.set(event_target_value(&ev))
                        >
                            <option value="">"All labels"</option>
                            {move || {
                                label_options
                                    .get()
                                    .into_iter()
                                    .map(|l| {
                                        let v = l.clone();
                                        view! { <option value=v>{l}</option> }
                                    })
                                    .collect_view()
                            }}
                        </select>
                    </label>
                    <div class="filter">
                        <span class="filter-label">"Devices"</span>
                        <Segmented
                            options=vec![
                                ("all".to_string(), "All devices".to_string()),
                                ("this".to_string(), "This device".to_string()),
                            ]
                            value=scope
                            on_change=Callback::new(move |v| scope.set(v))
                            label="Devices"
                            class="segmented-sm"
                        />
                    </div>
                    <label class="filter">
                        <span class="filter-label">"From"</span>
                        <input
                            class="text-input text-input-sm date-input"
                            type="date"
                            prop:value=move || from.get()
                            on:change=move |ev| from.set(event_target_value(&ev))
                        />
                    </label>
                    <label class="filter">
                        <span class="filter-label">"To"</span>
                        <input
                            class="text-input text-input-sm date-input"
                            type="date"
                            prop:value=move || to.get()
                            on:change=move |ev| to.set(event_target_value(&ev))
                        />
                    </label>
                    <label class="switch filter-switch">
                        <input
                            type="checkbox"
                            role="switch"
                            prop:checked=move || exclude.get()
                            on:change=move |ev| exclude.set(event_target_checked(&ev))
                        />
                        <span class="switch-track" aria-hidden="true"></span>
                        <span class="switch-text">"Exclude shared-network runs"</span>
                    </label>
                    <Show when=move || any_filter.get()>
                        <button type="button" class="link-button filter-reset" on:click=reset>
                            "Reset filters"
                        </button>
                    </Show>
                </section>

                {move || {
                    h.error
                        .get()
                        .map(|e| {
                            view! {
                                <p class="callout callout-warn" role="alert">
                                    <span class="callout-icon" aria-hidden="true">{icons::warning()}</span>
                                    {e}
                                </p>
                            }
                        })
                }}

                <Show when=move || h.loaded.get() && h.runs.with(Vec::is_empty) && h.error.with(Option::is_none)>
                    <section class="empty-state card">
                        <span class="empty-mark" aria-hidden="true"></span>
                        {move || {
                            if any_filter.get() {
                                view! {
                                    <h2 class="empty-title">"No runs match these filters"</h2>
                                    <p class="muted">"Try a wider date range or all devices."</p>
                                    <button type="button" class="pill-button" on:click=reset>
                                        "Reset filters"
                                    </button>
                                }
                                    .into_any()
                            } else {
                                view! {
                                    <h2 class="empty-title">"No runs yet"</h2>
                                    <p class="muted">
                                        "Run a test and it is saved here automatically, under its label."
                                    </p>
                                    <a class="pill-button" href="/">
                                        "Run a test"
                                    </a>
                                }
                                    .into_any()
                            }
                        }}
                    </section>
                </Show>
                <Show when=move || !h.loaded.get()>
                    <div class="history-skeleton" aria-busy="true" aria-label="Loading history">
                        <div class="skeleton-row"></div>
                        <div class="skeleton-row"></div>
                        <div class="skeleton-row"></div>
                    </div>
                </Show>

                {move || {
                    let groups = grouped.with(|g| g.1.clone());
                    (!groups.is_empty())
                        .then(|| {
                            view! {
                                <section class="run-group together-group" aria-labelledby="together-title">
                                    <header class="run-group-head">
                                        <span class="group-icon" aria-hidden="true">{icons::users()}</span>
                                        <h2 id="together-title" class="run-group-title">"Tested together"</h2>
                                        <span class="run-count">
                                            {format!(
                                                "{} group test{}",
                                                groups.len(),
                                                if groups.len() == 1 { "" } else { "s" },
                                            )}
                                        </span>
                                    </header>
                                    <ul class="run-list">
                                        {groups
                                            .into_iter()
                                            .map(|g| view! { <GroupRow h=h row=g/> })
                                            .collect_view()}
                                    </ul>
                                </section>
                            }
                        })
                }}

                <For
                    each=move || grouped.with(|g| g.0.clone())
                    key=|g| (g.key(), g.runs.iter().map(|r| (r.id, r.label.clone())).collect::<Vec<_>>())
                    let:group
                >
                    <LabelSection h=h group=group/>
                </For>

                <Show when=move || h.cursor.with(Option::is_some)>
                    <div class="load-more">
                        <button
                            type="button"
                            class="pill-button pill-ghost"
                            disabled=move || h.loading.get()
                            on:click=move |_| h.fetch(true)
                        >
                            {move || if h.loading.get() { "Loading…" } else { "Load more" }}
                        </button>
                    </div>
                </Show>

                <CompareBar h=h/>
            </Show>
        </div>
    }
}

/// One label's runs: a collapsible section with the label's medians.
#[component]
fn LabelSection(h: History, group: LabelGroup) -> impl IntoView {
    let ui = use_ui_state();
    let key = group.key();
    let label = group.label.clone();
    let title = label.clone().unwrap_or_else(|| "No label".into());
    let n = group.runs.len();
    let key2 = key.clone();
    let open = Memo::new(move |_| h.collapsed.with(|c| !c.contains(&key2)));
    let summary = {
        let label = label.clone();
        Memo::new(move |_| {
            label.as_ref().and_then(|l| {
                h.labels
                    .with(|ls| ls.iter().find(|x| &x.label == l).cloned())
            })
        })
    };
    let toggle = {
        let key = key.clone();
        move |_| {
            h.collapsed.update(|c| {
                if !c.remove(&key) {
                    c.insert(key.clone());
                }
            })
        }
    };
    let selectable = label.clone();
    let checked = {
        let l = label.clone();
        Memo::new(move |_| {
            l.as_ref()
                .is_some_and(|l| h.sel_labels.with(|s| s.contains(l)))
        })
    };
    let body_id = format!(
        "runs-{}",
        key.replace(|c: char| !c.is_ascii_alphanumeric(), "-")
    );
    view! {
        <section class="run-group" class:is-collapsed=move || !open.get()>
            <header class="run-group-head">
                <button
                    type="button"
                    class="collapse-button"
                    aria-expanded=move || if open.get() { "true" } else { "false" }
                    aria-controls=body_id.clone()
                    on:click=toggle
                >
                    <span class="chevron" aria-hidden="true">{icons::chevron_down()}</span>
                    <h2 class="run-group-title">{title.clone()}</h2>
                    <span class="run-count">
                        {move || {
                            let total = summary.get().map_or(n as u32, |s| s.runs.max(n as u32));
                            format!("{total} run{}", if total == 1 { "" } else { "s" })
                        }}
                    </span>
                </button>
                <div class="group-medians">
                    {move || {
                        summary
                            .get()
                            .map(|s| {
                                let unit = ui.units.get();
                                let (d, du) = rate_text(s.median_download_bps, unit);
                                let (u, uu) = rate_text(s.median_upload_bps, unit);
                                view! {
                                    <span class="median-note">"Median of last 5"</span>
                                    <span class="m series-download">
                                        <span class="m-arrow" aria-hidden="true">{ARROW_DOWNLOAD}</span>
                                        <span class="visually-hidden">"Download "</span>
                                        <strong>{d}</strong>
                                        " "
                                        {du}
                                    </span>
                                    <span class="m series-upload">
                                        <span class="m-arrow" aria-hidden="true">{ARROW_UPLOAD}</span>
                                        <span class="visually-hidden">"Upload "</span>
                                        <strong>{u}</strong>
                                        " "
                                        {uu}
                                    </span>
                                    <span class="m series-latency">
                                        <span class="m-key" aria-hidden="true"></span>
                                        <span class="visually-hidden">"Latency "</span>
                                        {ms_text(s.median_latency_ms)}
                                    </span>
                                }
                            })
                    }}
                </div>
                {selectable
                    .map(|l| {
                        view! {
                            <label class="check label-check" title="Compare this label">
                                <input
                                    type="checkbox"
                                    prop:checked=move || checked.get()
                                    on:change=move |_| {
                                        h.sel_labels.update(|s| {
                                            toggle_selection(s, l.clone());
                                        })
                                    }
                                />
                                <span class="check-box" aria-hidden="true">{icons::check()}</span>
                                <span class="check-text">"Compare label"</span>
                            </label>
                        }
                    })}
            </header>
            <ul class="run-list" id=body_id>
                {group.runs.into_iter().map(|r| view! { <RunRow h=h run=r/> }).collect_view()}
            </ul>
        </section>
    }
}

/// One saved run: time, device, metrics, badges, select/rename/delete.
#[component]
fn RunRow(h: History, run: RunSummary) -> impl IntoView {
    let ui = use_ui_state();
    let id = run.id;
    let href = format!("/runs/{id}");
    let when = format_when(&run.created_at);
    let device = run
        .client
        .device_name
        .clone()
        .unwrap_or_else(|| "Unknown device".into());
    let transport = match run.transport {
        Transport::Websocket => "WebSocket",
        Transport::Http => "HTTP",
    };
    let meta = format!(
        "{device} · {transport}{} · {}",
        if run.tls { " · TLS" } else { "" },
        profile_label(&run.profile)
    );
    let shared = shared_text(&Concurrency {
        peers_max: run.peers_max,
        overlap_ms: 0.0,
        overlap_ratio: run.overlap_ratio,
    });
    let s = run.summary;
    let draft = RwSignal::new(run.label.clone().unwrap_or_default());
    let selected = Memo::new(move |_| h.sel_runs.with(|s| s.contains(&id)));
    let full = Memo::new(move |_| h.sel_runs.with(|s| s.len() >= COMPARE_MAX));
    let input = NodeRef::<leptos::html::Input>::new();
    Effect::new(move |_| {
        if h.editing.get() == Some(id)
            && let Some(el) = input.get()
        {
            let _ = el.focus();
            el.select();
        }
    });

    view! {
        <li class="run-row" class:is-selected=move || selected.get()>
            <label class="check row-check">
                <input
                    type="checkbox"
                    aria-label=format!("Select the run of {when} to compare")
                    prop:checked=move || selected.get()
                    disabled=move || full.get() && !selected.get()
                    on:change=move |_| {
                        h.sel_runs.update(|s| {
                            toggle_selection(s, id);
                        })
                    }
                />
                <span class="check-box" aria-hidden="true">{icons::check()}</span>
            </label>
            {move || {
                if h.editing.get() == Some(id) {
                    view! {
                        <form
                            class="run-main rename-form"
                            on:submit=move |ev| {
                                ev.prevent_default();
                                h.rename(id, clean_label(&draft.get_untracked()));
                            }
                        >
                            <label class="visually-hidden" for=format!("rename-{id}")>
                                "Label"
                            </label>
                            <input
                                id=format!("rename-{id}")
                                class="text-input text-input-sm rename-input"
                                type="text"
                                maxlength="100"
                                node_ref=input
                                prop:value=move || draft.get()
                                on:input=move |ev| draft.set(event_target_value(&ev))
                                on:keydown=move |ev| {
                                    if ev.key() == "Escape" {
                                        h.editing.set(None);
                                    }
                                }
                            />
                            <button type="submit" class="pill-button pill-sm">
                                "Save"
                            </button>
                        </form>
                    }
                        .into_any()
                } else {
                    view! {
                        <a class="run-main" href=href.clone()>
                            <span class="run-when">{when.clone()}</span>
                            <span class="run-meta">{meta.clone()}</span>
                        </a>
                    }
                        .into_any()
                }
            }}
            <div class="run-metrics">
                <span
                    class="m series-download"
                    style=format!("view-transition-name:run-{id}")
                >
                    <span class="m-arrow" aria-hidden="true">{ARROW_DOWNLOAD}</span>
                    <span class="visually-hidden">"Download "</span>
                    {move || {
                        let (v, u) = rate_text(s.download_bps, ui.units.get());
                        view! {
                            <strong>{v}</strong>
                            " "
                            {u}
                        }
                    }}
                </span>
                <span class="m series-upload">
                    <span class="m-arrow" aria-hidden="true">{ARROW_UPLOAD}</span>
                    <span class="visually-hidden">"Upload "</span>
                    {move || {
                        let (v, u) = rate_text(s.upload_bps, ui.units.get());
                        view! {
                            <strong>{v}</strong>
                            " "
                            {u}
                        }
                    }}
                </span>
                <span class="m series-latency">
                    <span class="m-key" aria-hidden="true"></span>
                    <span class="visually-hidden">"Latency "</span>
                    {ms_text(s.latency_ms)}
                </span>
                {shared.map(|d| view! { <SharedBadge detail=d/> })}
            </div>
            <div class="run-actions">
                {move || {
                    if h.confirm.get() == Some(id) {
                        view! {
                            <span class="confirm" role="alert">
                                <span class="confirm-text">"Delete this run?"</span>
                                <button
                                    type="button"
                                    class="pill-button pill-sm pill-danger"
                                    on:click=move |_| h.delete(id)
                                >
                                    "Delete"
                                </button>
                                <button
                                    type="button"
                                    class="link-button"
                                    on:click=move |_| h.confirm.set(None)
                                >
                                    "Cancel"
                                </button>
                            </span>
                        }
                            .into_any()
                    } else {
                        view! {
                            <button
                                type="button"
                                class="icon-button icon-button-sm"
                                aria-label="Rename"
                                title="Rename"
                                on:click=move |_| {
                                    h.confirm.set(None);
                                    h.editing.set(Some(id));
                                }
                            >
                                {icons::pencil()}
                            </button>
                            <button
                                type="button"
                                class="icon-button icon-button-sm"
                                aria-label="Delete"
                                title="Delete"
                                on:click=move |_| {
                                    h.editing.set(None);
                                    h.confirm.set(Some(id));
                                }
                            >
                                {icons::trash()}
                            </button>
                        }
                            .into_any()
                    }
                }}
            </div>
        </li>
    }
}

/// One group test: expands to the server's group result.
#[component]
fn GroupRow(h: History, row: GroupTestRow) -> impl IntoView {
    let id = row.id;
    let open = Memo::new(move |_| h.expanded.with(|m| m.contains_key(&id)));
    let when = row
        .runs
        .last()
        .map(|r| format_when(&r.created_at))
        .unwrap_or_default();
    let names = row
        .runs
        .iter()
        .rev()
        .map(|r| {
            r.client
                .device_name
                .clone()
                .unwrap_or_else(|| "Device".into())
        })
        .collect::<Vec<_>>()
        .join(", ");
    let n = row.runs.len();
    let panel_id = format!("group-{id}");
    view! {
        <li class="run-row group-row" class:is-open=move || open.get()>
            <button
                type="button"
                class="group-toggle"
                aria-expanded=move || if open.get() { "true" } else { "false" }
                aria-controls=panel_id.clone()
                on:click=move |_| h.toggle_group(id)
            >
                <span class="chevron" aria-hidden="true">{icons::chevron_down()}</span>
                <span class="run-main">
                    <span class="run-when">{when}</span>
                    <span class="run-meta">
                        {format!("{n} device{} · {names}", if n == 1 { "" } else { "s" })}
                    </span>
                </span>
                <span class="group-cta">{move || if open.get() { "Hide result" } else { "Show result" }}</span>
            </button>
            <div class="group-panel" id=panel_id>
                {move || {
                    h.expanded
                        .with(|m| m.get(&id).cloned())
                        .map(|state| match state {
                            None => view! { <p class="muted group-loading">"Loading the group result…"</p> }.into_any(),
                            Some(Err(e)) => view! { <p class="callout callout-warn">{format!("Couldn't load this group test: {e}")}</p> }.into_any(),
                            Some(Ok(rec)) => match rec.detail {
                                Some(result) => view! { <GroupResultView result=result/> }.into_any(),
                                None => {
                                    view! {
                                        <p class="muted group-loading">
                                            "The server didn't record a combined result for this group test."
                                        </p>
                                    }
                                        .into_any()
                                }
                            },
                        })
                }}
            </div>
        </li>
    }
}

/// Sticky bar for the compare selection.
#[component]
fn CompareBar(h: History) -> impl IntoView {
    let target = Memo::new(move |_| {
        let runs = h.sel_runs.get();
        let labels = h.sel_labels.get();
        match (runs.is_empty(), labels.is_empty()) {
            (false, true) => Some(CompareTarget::Ids(runs)),
            (true, false) => Some(CompareTarget::Labels(labels)),
            _ => None,
        }
    });
    let count = Memo::new(move |_| h.sel_runs.with(Vec::len) + h.sel_labels.with(Vec::len));
    let mixed = Memo::new(move |_| {
        h.sel_runs.with(|r| !r.is_empty()) && h.sel_labels.with(|l| !l.is_empty())
    });
    let ready = Memo::new(move |_| {
        target.with(|t| {
            t.as_ref()
                .is_some_and(|t| (COMPARE_MIN..=COMPARE_MAX).contains(&t.len()))
        })
    });
    view! {
        <Show when=move || { count.get() > 0 }>
            <div class="compare-bar" role="region" aria-label="Compare selection">
                <p class="compare-text">
                    {move || {
                        if mixed.get() {
                            "Pick runs or labels, not both".to_string()
                        } else {
                            let n = count.get();
                            let what = if h.sel_labels.with(|l| !l.is_empty()) { "label" } else { "run" };
                            let more = if n < COMPARE_MIN { " · pick one more" } else { "" };
                            format!("{n} {what}{} selected{more}", if n == 1 { "" } else { "s" })
                        }
                    }}
                </p>
                <button
                    type="button"
                    class="link-button compare-clear"
                    on:click=move |_| {
                        h.sel_runs.set(Vec::new());
                        h.sel_labels.set(Vec::new());
                    }
                >
                    "Clear"
                </button>
                <a
                    class="toast-action compare-go"
                    class:is-disabled=move || !ready.get()
                    aria-disabled=move || (!ready.get()).then_some("true")
                    href=move || {
                        target
                            .get()
                            .filter(|_| ready.get())
                            .map_or_else(|| "#".to_string(), |t| format!("/compare?{}", t.query()))
                    }
                    on:click=move |ev| {
                        if !ready.get_untracked() {
                            ev.prevent_default();
                        }
                    }
                >
                    "Compare"
                </a>
            </div>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::history::ScoreClasses;
    use crate::model::result::{ClientInfo, Summary};

    fn run(n: u128, label: Option<&str>, group: Option<u128>) -> RunSummary {
        RunSummary {
            id: Uuid::from_u128(n),
            created_at: format!("2026-10-10T10:00:{:02}Z", 60 - n),
            label: label.map(str::to_string),
            notes: None,
            transport: Transport::Websocket,
            profile: "standard".into(),
            streams: 1,
            client: ClientInfo::default(),
            tls: false,
            group_id: group.map(Uuid::from_u128),
            peers_max: 0,
            overlap_ratio: 0.0,
            summary: Summary::default(),
            scores: ScoreClasses::default(),
            duration_ms: 1000,
        }
    }

    #[test]
    fn groups_by_label_in_newest_first_order() {
        let runs = vec![
            run(1, Some("Desk"), None),
            run(2, Some("Bedroom"), None),
            run(3, None, Some(99)),
            run(4, Some("Desk"), None),
            run(5, None, None),
            run(6, Some("Kitchen"), Some(99)),
        ];
        let (labels, groups) = group_runs(&runs);
        let keys: Vec<String> = labels.iter().map(LabelGroup::key).collect();
        assert_eq!(keys, vec!["Desk", "Bedroom", ""]);
        assert_eq!(labels[0].runs.len(), 2);
        assert_eq!(labels[0].runs[1].id, Uuid::from_u128(4));
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].runs.len(), 2);
        assert!(group_runs(&[]).0.is_empty());
    }

    #[test]
    fn selection_is_capped() {
        let mut s = vec![];
        for i in 0..5 {
            toggle_selection(&mut s, i);
        }
        assert_eq!(s, vec![0, 1, 2, 3]);
        assert!(toggle_selection(&mut s, 1));
        assert_eq!(s, vec![0, 2, 3]);
        assert!(toggle_selection(&mut s, 7));
        assert!(!toggle_selection(&mut s, 8));
    }
}
