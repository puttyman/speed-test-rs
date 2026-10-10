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

pub fn phone() -> impl IntoView {
    icon("M7.5 2.5h9a1 1 0 0 1 1 1v17a1 1 0 0 1-1 1h-9a1 1 0 0 1-1-1v-17a1 1 0 0 1 1-1ZM11 18.5h2")
}

pub fn tablet() -> impl IntoView {
    icon("M5.5 3h13a1 1 0 0 1 1 1v16a1 1 0 0 1-1 1h-13a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1ZM11 18h2")
}

pub fn laptop() -> impl IntoView {
    icon("M5 6a1 1 0 0 1 1-1h12a1 1 0 0 1 1 1v9H5V6ZM2.5 18.5h19l-1.5-3.5H4l-1.5 3.5Z")
}

pub fn desktop() -> impl IntoView {
    icon("M3 4.5h18v12H3zM9 20.5h6M12 16.5v4")
}

pub fn tv() -> impl IntoView {
    icon("M3 6.5h18v12H3zM8 3l4 3.5L16 3M8 21h8")
}

pub fn device() -> impl IntoView {
    icon("M4 5h11v9H4zM17 9h4v11h-4zM7 18h8M9.5 14v4")
}

pub fn users() -> impl IntoView {
    icon(
        "M9 11a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7ZM2.5 20a6.5 6.5 0 0 1 13 0M16 4.3a3.5 3.5 0 0 1 0 6.4M18.5 14.3a6.5 6.5 0 0 1 3 5.7",
    )
}

pub fn trash() -> impl IntoView {
    icon("M4 7h16M10 11v6M14 11v6M5.5 7l1 13h11l1-13M9 7V4h6v3")
}

pub fn chevron_down() -> impl IntoView {
    icon("M6 9l6 6 6-6")
}

pub fn arrow_down() -> impl IntoView {
    icon("M12 4v16M6 14l6 6 6-6")
}

pub fn arrow_up() -> impl IntoView {
    icon("M12 20V4M6 10l6-6 6 6")
}

pub fn warning() -> impl IntoView {
    icon("M12 3.5 2.5 20h19L12 3.5ZM12 10v4.5M12 17.4v.1")
}

pub fn share() -> impl IntoView {
    icon(
        "M10 14a4.5 4.5 0 0 0 6.4 0l3-3a4.5 4.5 0 0 0-6.4-6.4l-1 1M14 10a4.5 4.5 0 0 0-6.4 0l-3 3a4.5 4.5 0 0 0 6.4 6.4l1-1",
    )
}

pub fn export() -> impl IntoView {
    icon("M12 3v12M7 10l5 5 5-5M4 17v3h16v-3")
}

/// The icon of a device type (SPEC §7.11).
pub fn device_kind(kind: crate::model::protocol::DeviceKind) -> AnyView {
    use crate::model::protocol::DeviceKind;
    match kind {
        DeviceKind::Phone => phone().into_any(),
        DeviceKind::Tablet => tablet().into_any(),
        DeviceKind::Laptop => laptop().into_any(),
        DeviceKind::Desktop => desktop().into_any(),
        DeviceKind::Tv => tv().into_any(),
        DeviceKind::Other => device().into_any(),
    }
}
