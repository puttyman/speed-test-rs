//! Segmented control (docs/design/tokens.md): a soft track with a white
//! thumb that slides under the selected option (350 ms). A radio group with
//! roving focus: arrow keys move the selection.

use leptos::prelude::*;

/// Index after moving `delta` steps (wrapping) in `n` options.
pub fn step_index(current: usize, delta: i32, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let n = n as i64;
    ((current as i64 + i64::from(delta)).rem_euclid(n)) as usize
}

#[component]
pub fn Segmented(
    /// `(value, label)` pairs.
    options: Vec<(String, String)>,
    #[prop(into)] value: Signal<String>,
    on_change: Callback<String>,
    /// Accessible name of the group.
    label: &'static str,
    #[prop(optional, into)] disabled: Signal<bool>,
    /// Extra class, e.g. `segmented-sm`.
    #[prop(optional)]
    class: &'static str,
) -> impl IntoView {
    let n = options.len();
    let values: Vec<String> = options.iter().map(|(v, _)| v.clone()).collect();
    let values = StoredValue::new(values);
    let index = Memo::new(move |_| {
        let v = value.get();
        values.with_value(|vals| vals.iter().position(|x| *x == v).unwrap_or(0))
    });

    let on_key = move |ev: leptos::ev::KeyboardEvent| {
        let delta = match ev.key().as_str() {
            "ArrowRight" | "ArrowDown" => 1,
            "ArrowLeft" | "ArrowUp" => -1,
            _ => return,
        };
        ev.prevent_default();
        if disabled.get_untracked() {
            return;
        }
        let next = step_index(index.get_untracked(), delta, n);
        if let Some(v) = values.with_value(|vals| vals.get(next).cloned()) {
            on_change.run(v);
        }
        focus_child(&ev, next);
    };

    view! {
        <div
            class=format!("segmented {class}")
            role="radiogroup"
            aria-label=label
            aria-disabled=move || disabled.get().then_some("true")
            style=move || format!("--seg-n:{n};--seg-i:{}", index.get())
            on:keydown=on_key
        >
            <span class="segmented-thumb" aria-hidden="true"></span>
            {options
                .into_iter()
                .enumerate()
                .map(|(i, (v, text))| {
                    let checked = move || index.get() == i;
                    view! {
                        <button
                            type="button"
                            role="radio"
                            class="segmented-option"
                            aria-checked=move || if checked() { "true" } else { "false" }
                            tabindex=move || if checked() { "0" } else { "-1" }
                            disabled=move || disabled.get()
                            on:click=move |_| on_change.run(v.clone())
                        >
                            {text}
                        </button>
                    }
                })
                .collect_view()}
        </div>
    }
}

/// Moves focus to the `i`-th option of the group that received `ev`.
fn focus_child(ev: &leptos::ev::KeyboardEvent, i: usize) {
    use wasm_bindgen::JsCast;
    let Some(group) = ev
        .current_target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
    else {
        return;
    };
    // Child 0 is the thumb.
    if let Some(el) = group
        .children()
        .item(i as u32 + 1)
        .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
    {
        let _ = el.focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_wrap() {
        assert_eq!(step_index(0, 1, 3), 1);
        assert_eq!(step_index(2, 1, 3), 0);
        assert_eq!(step_index(0, -1, 3), 2);
        assert_eq!(step_index(0, 1, 0), 0);
    }
}
