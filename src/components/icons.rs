//! Inline line icons (24 px grid, 1.75 px stroke, `currentColor`).

use leptos::prelude::*;

fn icon(path: &'static str) -> impl IntoView {
    view! {
        <svg
            class="icon"
            viewBox="0 0 24 24"
            width="20"
            height="20"
            fill="none"
            stroke="currentColor"
            stroke-width="1.75"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
            focusable="false"
        >
            <path d=path></path>
        </svg>
    }
}

pub fn sun() -> impl IntoView {
    icon(
        "M12 3v1.5M12 19.5V21M4.6 4.6l1.1 1.1M18.3 18.3l1.1 1.1M3 12h1.5M19.5 12H21M4.6 19.4l1.1-1.1M18.3 5.7l1.1-1.1M16 12a4 4 0 1 1-8 0 4 4 0 0 1 8 0Z",
    )
}

pub fn moon() -> impl IntoView {
    icon("M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5Z")
}

pub fn monitor() -> impl IntoView {
    icon("M3.5 5.5h17v11h-17zM9 20h6M12 16.5V20")
}

pub fn settings() -> impl IntoView {
    icon(
        "M4 7h9M17 7h3M4 17h3M11 17h9M15 9.5a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5ZM9 19.5a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5Z",
    )
}

pub fn close() -> impl IntoView {
    icon("M6 6l12 12M18 6 6 18")
}

pub fn pin() -> impl IntoView {
    icon(
        "M12 21s-6.5-6.2-6.5-11a6.5 6.5 0 0 1 13 0c0 4.8-6.5 11-6.5 11ZM12 12.2a2.2 2.2 0 1 0 0-4.4 2.2 2.2 0 0 0 0 4.4Z",
    )
}

pub fn pencil() -> impl IntoView {
    icon("M4 20h4L19 9l-4-4L4 16v4ZM13.5 6.5l4 4")
}

pub fn check() -> impl IntoView {
    icon("M5 12.5l4.5 4.5L19 7.5")
}

pub fn info() -> impl IntoView {
    icon("M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18ZM12 11v5.5M12 7.6v.1")
}
