//! The document shell, router and header. Pages live in [`crate::pages`],
//! reusable pieces in [`crate::components`].

use leptos::prelude::*;
use leptos_meta::{MetaTags, Stylesheet, Title, provide_meta_context};
use leptos_router::{
    StaticSegment,
    components::{A, Route, Router, Routes},
};

use crate::components::pwa_toast::PwaToast;
use crate::components::settings_drawer::{SettingsButton, SettingsDrawer};
use crate::components::theme_toggle::ThemeToggle;
use crate::components::units_toggle::UnitsToggle;
use crate::pages::history::HistoryPage;
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
                <Routes fallback=|| view! { <NotFound/> }>
                    <Route path=StaticSegment("") view=TestPage/>
                    <Route path=StaticSegment("history") view=HistoryPage/>
                </Routes>
            </main>
            <SettingsDrawer locked=ui.running max_streams=ui.max_streams/>
            <PwaToast/>
        </Router>
    }
}

#[component]
fn Header() -> impl IntoView {
    view! {
        <header class="site-header">
            <div class="inner">
                <a class="brand" href="/" aria-label="speed-test-rs home">
                    <span class="brand-mark" aria-hidden="true"></span>
                    <span class="brand-name">"speed-test-rs"</span>
                </a>
                <nav class="site-nav" aria-label="Main">
                    <A href="/" exact=true>
                        "Test"
                    </A>
                    <A href="/history">"History"</A>
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
