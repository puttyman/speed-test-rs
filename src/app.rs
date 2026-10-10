use leptos::prelude::*;
use leptos_meta::{MetaTags, Stylesheet, Title, provide_meta_context};
use leptos_router::{
    StaticSegment,
    components::{Route, Router, Routes},
};

/// HTML document shell rendered by the server around the hydrated `App`.
pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
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

    view! {
        // id=leptos means cargo-leptos will hot-reload this stylesheet
        <Stylesheet id="leptos" href="/pkg/speed-test-rs.css"/>
        <Title text="Speed Test"/>

        <Router>
            <Header/>
            <main class="page">
                <Routes fallback=|| view! { <NotFound/> }>
                    <Route path=StaticSegment("") view=HomePage/>
                </Routes>
            </main>
        </Router>
    }
}

#[component]
fn Header() -> impl IntoView {
    view! {
        <header class="site-header">
            <div class="inner">
                <a class="brand" href="/">
                    <span class="brand-mark" aria-hidden="true"></span>
                    "speed-test-rs"
                </a>
                <nav class="site-nav">
                    <a href="/" aria-current="page">"Test"</a>
                </nav>
            </div>
        </header>
    }
}

/// The measurement view. Values are placeholders until the WebSocket engine
/// (Milestone 1 in docs/SPEC.md) is wired up.
#[component]
fn HomePage() -> impl IntoView {
    view! {
        <section class="hero">
            <div class="hero-primary">
                <h1 class="hero-title">"Your network speed"</h1>
                <div class="hero-value">
                    <span class="value">"—"</span>
                    <span class="unit">"Mbps"</span>
                </div>
                <button
                    class="start-button"
                    disabled=true
                    title="The measurement engine is not implemented yet. See docs/SPEC.md."
                >
                    "Start test"
                </button>
            </div>
            <div class="metric-grid">
                <Metric label="Download" unit="Mbps" kind="download"/>
                <Metric label="Upload" unit="Mbps" kind="upload"/>
                <Metric label="Latency" unit="ms" kind="latency"/>
                <Metric label="Jitter" unit="ms" kind="jitter"/>
                <Metric label="Packet loss" unit="%" kind="loss"/>
            </div>
        </section>

        <section class="card-row">
            <div class="card">
                <h2>"Network quality score"</h2>
                <ul class="quality-list">
                    <li><span>"Video streaming"</span><span class="badge">"—"</span></li>
                    <li><span>"Online gaming"</span><span class="badge">"—"</span></li>
                    <li><span>"Video chatting"</span><span class="badge">"—"</span></li>
                </ul>
            </div>
            <div class="card">
                <h2>"Server"</h2>
                <p class="muted">"Self-hosted on your local network"</p>
            </div>
        </section>
    }
}

#[component]
fn Metric(label: &'static str, unit: &'static str, kind: &'static str) -> impl IntoView {
    view! {
        <div class=format!("metric metric-{kind}")>
            <div class="metric-label">{label}</div>
            <div class="metric-value">
                <span class="value">"—"</span>
                <span class="unit">{unit}</span>
            </div>
        </div>
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
        <section class="card">
            <h1>"404"</h1>
            <p>"Page not found. " <a href="/">"Back to the speed test"</a></p>
        </section>
    }
}
