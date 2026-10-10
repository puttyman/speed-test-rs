//! Label field (SPEC §7.7): shows the label the next run will be saved
//! with. Tap to edit; recent labels from `GET /api/labels?device=` appear as
//! one-tap chips, with free text for a new one.

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;

use super::icons;
use crate::pages::api;

/// Max chips shown.
const MAX_CHIPS: usize = 8;
pub const MAX_LABEL_CHARS: usize = 100;

/// Trims and caps a typed label; `None` when empty.
pub fn clean_label(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.chars().take(MAX_LABEL_CHARS).collect())
    }
}

#[component]
pub fn LabelPicker(
    label: RwSignal<String>,
    #[prop(into)] device_id: Signal<String>,
    #[prop(optional, into)] disabled: Signal<bool>,
) -> impl IntoView {
    let editing = RwSignal::new(false);
    let draft = RwSignal::new(String::new());
    let chips = RwSignal::new(Vec::<String>::new());
    let input = NodeRef::<html::Input>::new();

    let open = move || {
        if disabled.get_untracked() {
            return;
        }
        draft.set(label.get_untracked());
        editing.set(true);
        let device = device_id.get_untracked();
        spawn_local(async move {
            if let Ok(list) = api::labels(&device).await {
                chips.set(
                    list.into_iter()
                        .map(|l| l.label)
                        .filter(|l| !l.is_empty())
                        .take(MAX_CHIPS)
                        .collect(),
                );
            }
        });
    };
    let commit = move |value: String| {
        if let Some(v) = clean_label(&value) {
            label.set(v);
        }
        editing.set(false);
    };

    // Focus and select the text when the editor opens.
    Effect::new(move |_| {
        if editing.get()
            && let Some(el) = input.get()
        {
            let _ = el.focus();
            el.select();
        }
    });

    view! {
        <div class="label-picker" class:is-editing=move || editing.get()>
            <Show
                when=move || editing.get()
                fallback=move || {
                    view! {
                        <button
                            type="button"
                            class="label-button"
                            disabled=move || disabled.get()
                            aria-label=move || format!("Label: {}. Edit label", label.get())
                            on:click=move |_| open()
                        >
                            <span class="label-pin">{icons::pin()}</span>
                            <span class="label-text">{move || label.get()}</span>
                            <span class="label-edit">{icons::pencil()}</span>
                        </button>
                    }
                }
            >
                <form
                    class="label-editor"
                    on:submit=move |ev| {
                        ev.prevent_default();
                        commit(draft.get_untracked());
                    }
                >
                    <label class="visually-hidden" for="label-input">
                        "Label for this run"
                    </label>
                    <div class="label-input-row">
                        <input
                            id="label-input"
                            class="text-input"
                            type="text"
                            maxlength=MAX_LABEL_CHARS.to_string()
                            placeholder="e.g. Bedroom, Office AP – 5 GHz"
                            autocomplete="off"
                            node_ref=input
                            prop:value=move || draft.get()
                            on:input=move |ev| draft.set(event_target_value(&ev))
                            on:keydown=move |ev| {
                                if ev.key() == "Escape" {
                                    ev.prevent_default();
                                    editing.set(false);
                                }
                            }
                        />
                        <button type="submit" class="pill-button pill-sm">
                            "Done"
                        </button>
                    </div>
                    <Show when=move || !chips.with(|c| c.is_empty())>
                        <ul class="chips" aria-label="Recent labels">
                            {move || {
                                chips
                                    .get()
                                    .into_iter()
                                    .map(|c| {
                                        let c2 = c.clone();
                                        let text = c.clone();
                                        view! {
                                            <li>
                                                <button
                                                    type="button"
                                                    class="chip"
                                                    class:is-selected=move || draft.get() == c2
                                                    on:click=move |_| commit(c.clone())
                                                >
                                                    {text}
                                                </button>
                                            </li>
                                        }
                                    })
                                    .collect_view()
                            }}
                        </ul>
                    </Show>
                </form>
            </Show>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_labels() {
        assert_eq!(clean_label("  Bedroom "), Some("Bedroom".into()));
        assert_eq!(clean_label("   "), None);
        assert_eq!(clean_label(&"x".repeat(150)).unwrap().len(), 100);
    }
}
