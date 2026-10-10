//! `/history` (SPEC §7.7). Placeholder until the history wave lands.

use leptos::prelude::*;
use leptos_meta::Title;

#[component]
pub fn HistoryPage() -> impl IntoView {
    view! {
        <Title text="History · Speed Test"/>
        <section class="placeholder-page">
            <h1 class="page-title">"History"</h1>
            <p class="muted">"History is coming soon."</p>
            <a class="pill-button" href="/">
                "Back to the test"
            </a>
        </section>
    }
}
