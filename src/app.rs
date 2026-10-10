//! The document shell, router and header. Pages live in [`crate::pages`],
//! reusable pieces in [`crate::components`].

use leptos::prelude::*;
use leptos_meta::{MetaTags, Stylesheet, Title, provide_meta_context};
use leptos_router::{
    ParamSegment, StaticSegment,
    components::{Route, Router, Routes},
    hooks::use_location,
};

use crate::components::devices_panel::InviteToast;
use crate::components::presence::provide_presence;
use crate::components::pwa_toast::PwaToast;
use crate::components::settings_drawer::{SettingsButton, SettingsDrawer};
use crate::components::theme_toggle::ThemeToggle;
use crate::components::units_toggle::UnitsToggle;
use crate::pages::compare::ComparePage;
use crate::pages::history::HistoryPage;
use crate::pages::run::RunPage;
use crate::pages::test::TestPage;
use crate::ui_state::{THEME_BOOT_SCRIPT, provide_ui_state};

/// HTML document shell rendered by the server around the hydrated `App`.
pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover"/>
                // Apply the stored theme before first paint (no flash on reload).
                <script inner_html=THEME_BOOT_SCRIPT></script>
                // PWA (SPEC §7.9): manifest, icons, iOS standalone metas, SW registration
                <link rel="manifest" href="/manifest.webmanifest"/>
                <meta
                    name="theme-color"
                    content="#ffffff"
                    media="(prefers-color-scheme: light)"
                />
                <meta name="theme-color" content="#171d26" media="(prefers-color-scheme: dark)"/>
                <link rel="icon" href="/favicon.ico" sizes="32x32"/>
                <link rel="icon" href="/favicon.svg" type="image/svg+xml"/>
                <link rel="apple-touch-icon" href="/icons/apple-touch-icon.png"/>
                <link
                    rel="preload"
                    href="/fonts/inter-latin-var.woff2"
                    r#as="font"
                    r#type="font/woff2"
                    crossorigin="anonymous"
                />
                <meta name="apple-mobile-web-app-capable" content="yes"/>
                <meta name="mobile-web-app-capable" content="yes"/>
                <meta name="apple-mobile-web-app-status-bar-style" content="default"/>
                <meta name="apple-mobile-web-app-title" content="Speed test"/>
                <script src="/pwa.js" defer></script>
                <AutoReload options=options.clone()/>
                <HydrationScripts options/>
                <MetaTags/>
            </head>
            <body>
                <App/>
            </body>
        </html>
    }
}

#[component]
pub fn App() -> impl IntoView {
    // Provides context that manages stylesheets, titles, meta tags, etc.
    provide_meta_context();
    let ui = provide_ui_state();
    // One presence connection per page load, shared by every page (SPEC §5.11).
    provide_presence(ui);

    view! {
        // id=leptos means cargo-leptos will hot-reload this stylesheet
        <Stylesheet id="leptos" href="/pkg/speed-test-rs.css"/>
        <Title text="Speed Test"/>

        <Router>
            <a class="skip-link" href="#main">
                "Skip to content"
            </a>
            <Header/>
            <main id="main" class="page">
                // View transitions between pages (SPEC §7.3); the router
                // falls back to a plain swap where they're unsupported, and
                // CSS turns the animation off for reduced motion.
                <Routes transition=true fallback=|| view! { <NotFound/> }>
                    <Route path=StaticSegment("") view=TestPage/>
                    <Route path=StaticSegment("history") view=HistoryPage/>
                    <Route path=StaticSegment("compare") view=ComparePage/>
                    <Route path=(StaticSegment("runs"), ParamSegment("id")) view=RunPage/>
                </Routes>
            </main>
            <SettingsDrawer locked=ui.running max_streams=ui.max_streams/>
            <InviteToast/>
            <PwaToast/>
        </Router>
    }
}

/// The nav section a path belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Test,
    History,
    Other,
}

/// History, compare and run pages all live under "History".
pub fn section_of(path: &str) -> Section {
    let first = path.trim_start_matches('/').split('/').next().unwrap_or("");
    match first {
        "" => Section::Test,
        "history" | "compare" | "runs" => Section::History,
        _ => Section::Other,
    }
}

#[component]
fn Header() -> impl IntoView {
    let location = use_location();
    let section = move || location.pathname.with(|p| section_of(p));
    view! {
        <header class="site-header">
            <div class="inner">
                <a class="brand" href="/" aria-label="speed-test-rs home">
                    <span class="brand-mark" aria-hidden="true"></span>
                    <span class="brand-name">"speed-test-rs"</span>
                </a>
                <nav class="site-nav" aria-label="Main">
                    <a href="/" aria-current=move || (section() == Section::Test).then_some("page")>
                        "Test"
                    </a>
                    <a
                        href="/history"
                        aria-current=move || (section() == Section::History).then_some("page")
                    >
                        "History"
                    </a>
                </nav>
                <div class="header-tools">
                    <UnitsToggle class="segmented-sm header-units"/>
                    <ThemeToggle/>
                    <SettingsButton/>
                </div>
            </div>
        </header>
    }
}

#[component]
fn NotFound() -> impl IntoView {
    // Set the HTTP status to 404 when rendering on the server
    #[cfg(feature = "ssr")]
    {
        if let Some(resp) = use_context::<leptos_axum::ResponseOptions>() {
            resp.set_status(axum::http::StatusCode::NOT_FOUND);
        }
    }

    view! {
        <section class="placeholder-page">
            <h1 class="page-title">"404"</h1>
            <p class="muted">"Page not found."</p>
            <a class="pill-button" href="/">
                "Back to the speed test"
            </a>
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nav_sections() {
        assert_eq!(section_of("/"), Section::Test);
        assert_eq!(section_of("/history"), Section::History);
        assert_eq!(section_of("/compare"), Section::History);
        assert_eq!(section_of("/runs/abc"), Section::History);
        assert_eq!(section_of("/nope"), Section::Other);
    }
}
