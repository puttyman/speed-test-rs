//! ⓘ button with a one-sentence explanation (SPEC §7.5), shown on hover and
//! keyboard focus.

use leptos::prelude::*;

use super::icons;

#[component]
pub fn InfoTip(
    /// What the tip explains, for the button's accessible name.
    topic: &'static str,
    text: &'static str,
) -> impl IntoView {
    let id = format!("tip-{}", topic.to_ascii_lowercase().replace(' ', "-"));
    view! {
        <span class="info-tip">
            <button
                type="button"
                class="info-btn"
                aria-label=format!("About {topic}")
                aria-describedby=id.clone()
            >
                {icons::info()}
            </button>
            <span class="info-bubble" role="tooltip" id=id>
                {text}
            </span>
        </span>
    }
}
