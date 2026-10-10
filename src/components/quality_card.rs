//! Network quality (SPEC §5.9, §7.4): one status badge per use case. The
//! badge carries the class name as text, not just color.

use leptos::prelude::*;

use super::motion::reveal_style;
use crate::model::scores::{Classification, Score, Scores};
use crate::model::units::PLACEHOLDER;

/// Display name of a class.
pub fn class_label(class: Classification) -> &'static str {
    match class {
        Classification::Bad => "Bad",
        Classification::Poor => "Poor",
        Classification::Average => "Average",
        Classification::Good => "Good",
        Classification::Great => "Great",
    }
}

/// A status badge (`status-great` … `status-bad` tokens).
#[component]
pub fn StatusBadge(#[prop(into)] class: Signal<Option<Classification>>) -> impl IntoView {
    view! {
        <span class=move || {
            format!(
                "badge {}",
                class.get().map_or("badge-pending".to_string(), |c| format!("status-{}", c.as_str())),
            )
        }>{move || class.get().map_or(PLACEHOLDER, class_label)}</span>
    }
}

#[component]
pub fn QualityCard(#[prop(into)] scores: Signal<Option<Scores>>) -> impl IntoView {
    let row =
        |name: &'static str, hint: &'static str, pick: fn(&Scores) -> Option<Score>, i: usize| {
            let class =
                Signal::derive(move || scores.get().and_then(|s| pick(&s)).map(|s| s.class));
            view! {
                <li>
                    <div class="quality-name">
                        <span>{name}</span>
                        <span class="quality-hint">{hint}</span>
                    </div>
                    <span class="badge-flip reveal" style=reveal_style(i)>
                        <StatusBadge class=class/>
                    </span>
                </li>
            }
        };
    view! {
        <section class="card quality-card" aria-labelledby="quality-title">
            <h2 id="quality-title" class="card-title">"Network quality"</h2>
            <ul class="quality-list">
                {row("Video streaming", "Download, latency and loss", |s| s.streaming, 5)}
                {row("Online gaming", "Latency, loss and load", |s| s.gaming, 6)}
                {row("Video chatting", "Latency, jitter and loss", |s| s.rtc, 7)}
            </ul>
            {move || {
                scores
                    .get()
                    .filter(|s| s.streaming.is_none() && s.gaming.is_none() && s.rtc.is_none())
                    .map(|_| {
                        view! {
                            <p class="card-note">
                                "Scores need latency under load. The Standard profile measures it."
                            </p>
                        }
                    })
            }}
        </section>
    }
}
