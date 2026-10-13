//! `/runs/{id}` (SPEC §7.8): a read-only, shareable result page with the
//! finished test page's layout (hero number, tiles, quality, details),
//! from `GET /api/runs/{id}`.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_meta::Title;
use leptos_router::hooks::use_params_map;
use uuid::Uuid;

use crate::components::icons;
use crate::components::quality_card::QualityCard;
use crate::components::result_view::{
    ResultDetails, ResultTiles, RunInfoCard, SharedBadge, ThroughputCard, format_when, shared_text,
};
use crate::model::result::RunResult;
use crate::model::units::{format_ms, format_rate};
use crate::pages::api;
use crate::pages::test::loss_reason;
use crate::ui_state::use_ui_state;

#[component]
pub fn RunPage() -> impl IntoView {
    let ui = use_ui_state();
    let params = use_params_map();
    let id =
        Memo::new(move |_| params.with(|p| p.get("id").and_then(|s| Uuid::parse_str(&s).ok())));
    let run = RwSignal::new(None::<Result<RunResult, String>>);
    let copied = RwSignal::new(false);
    Effect::new(move |_| {
        let Some(id) = id.get() else {
            run.set(Some(Err("This isn't a valid run link.".into())));
            return;
        };
        run.set(None);
        spawn_local(async move {
            run.set(Some(api::get_run(id).await.map_err(|e| {
                if e.starts_with("HTTP 404") {
                    "This run doesn't exist any more. It may have been deleted.".to_string()
                } else {
                    e
                }
            })));
        });
    });
    let result =
        Signal::derive(move || run.with(|r| r.as_ref().and_then(|r| r.as_ref().ok().cloned())));
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

    view! {
        <Title text="Run · Speed Test"/>
        <div class="run-page test-page is-revealed">
            {move || match run.get() {
                None => {
                    view! {
                        <div class="history-skeleton" aria-busy="true" aria-label="Loading the run">
                            <div class="skeleton-row tall"></div>
                            <div class="skeleton-row"></div>
                        </div>
                    }
                        .into_any()
                }
                Some(Err(e)) => {
                    view! {
                        <section class="empty-state card">
                            <h1 class="empty-title">"Run not found"</h1>
                            <p class="muted">{e}</p>
                            <a class="pill-button" href="/history">
                                "Back to history"
                            </a>
                        </section>
                    }
                        .into_any()
                }
                Some(Ok(r)) => {
                    let label = r.label.clone().unwrap_or_else(|| "Unlabelled run".into());
                    let when = format_when(&r.created_at);
                    let shared = shared_text(&r.concurrency);
                    let vt = format!("view-transition-name:run-{}", r.id);
                    let s = r.summary;
                    let caption = move || {
                        let unit = ui.units.get();
                        let (u, uu) = format_rate(s.upload_bps.unwrap_or(f64::NAN), unit);
                        let (l, lu) = format_ms(s.latency_ms.unwrap_or(f64::NAN));
                        format!("Upload {u} {uu} · Latency {l} {lu}")
                    };
                    let group = r.group_id;
                    view! {
                        <section class="hero run-hero" aria-labelledby="run-title">
                            <div class="hero-inner">
                                <p class="run-eyebrow">{when}</p>
                                <h1 id="run-title" class="hero-title">{label}</h1>
                                <p class="hero-value" style=vt>
                                    {move || {
                                        let (v, u) = format_rate(s.download_bps.unwrap_or(f64::NAN), ui.units.get());
                                        view! {
                                            <span class="value">{v}</span>
                                            <span class="unit">{u}</span>
                                        }
                                    }}
                                </p>
                                <p class="hero-caption">"Download · " {caption}</p>
                                <div class="hero-actions run-actions-row">
                                    {shared.map(|d| view! { <SharedBadge detail=d long=true/> })}
                                    {group
                                        .map(|_| {
                                            view! {
                                                <a class="pill-button pill-sm pill-ghost" href="/history">
                                                    {icons::users()}
                                                    "Part of a group test"
                                                </a>
                                            }
                                        })}
                                    <button type="button" class="pill-button pill-sm pill-ghost" on:click=share>
                                        {icons::share()}
                                        {move || if copied.get() { "Link copied" } else { "Copy link" }}
                                    </button>
                                    <a class="pill-button pill-sm" href="/">
                                        "Run a new test"
                                    </a>
                                </div>
                            </div>
                        </section>
                        <ResultTiles result=result/>
                        <ThroughputCard result=result/>
                        <div class="card-row">
                            <QualityCard scores=Signal::derive(move || result.with(|r| r.as_ref().map(|r| r.scores)))/>
                            <RunInfoCard result=r.clone()/>
                        </div>
                        <ResultDetails
                            result=result
                            loss_reason=Signal::derive(move || {
                                result
                                    .with(|r| {
                                        r.as_ref()
                                            .map(|r| {
                                                let steps = crate::model::profile::Profile::by_name(&r.profile)
                                                    .unwrap_or_default()
                                                    .steps;
                                                loss_reason(&r.profile, &steps)
                                            })
                                            .unwrap_or_default()
                                    })
                            })
                        />
                    }
                        .into_any()
                }
            }}
        </div>
    }
}
