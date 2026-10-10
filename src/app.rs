use leptos::prelude::*;
use leptos_meta::{MetaTags, Stylesheet, Title, provide_meta_context};
use leptos_router::{
    StaticSegment,
    components::{Route, Router, Routes},
};

use crate::model::profile::{Direction, Step};
use crate::model::scores::{Classification, Score, Scores};
use crate::model::units::{PLACEHOLDER, Unit, format_ms, format_percent, format_rate};

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

/// The measurement view: hero number, metric tiles and quality badges, driven
/// by [`TestController`]. A later wave replaces this with the full UI.
#[component]
fn HomePage() -> impl IntoView {
    let test = TestController::new();
    let status = test.status;
    let running = move || status.get() == RunStatus::Running;

    let hero = Signal::derive(move || match status.get() {
        RunStatus::Running => rate_parts(test.live_bps.get()),
        _ => rate_parts(test.download.get()),
    });
    let hero_title = move || match (status.get(), test.live_direction.get()) {
        (RunStatus::Running, Some(Direction::Download)) => "Download speed",
        (RunStatus::Running, Some(Direction::Upload)) => "Upload speed",
        (RunStatus::Done, _) => "Download speed",
        _ => "Your network speed",
    };
    let button_label = move || match status.get() {
        RunStatus::Idle => "Start test",
        RunStatus::Running => "Stop",
        RunStatus::Done | RunStatus::Failed => "Test again",
    };
    let badge = move |pick: fn(&Scores) -> Option<Score>| {
        move || {
            test.scores
                .get()
                .and_then(|s| pick(&s))
                .map_or(PLACEHOLDER, |s| class_label(s.class))
        }
    };

    view! {
        <section class="hero">
            <div class="hero-primary">
                <h1 class="hero-title">{hero_title}</h1>
                <div class="hero-value" aria-live="polite">
                    <span class="value">{move || hero.get().0}</span>
                    <span class="unit">{move || hero.get().1}</span>
                </div>
                <p class="phase-label" aria-live="polite">{move || test.phase.get()}</p>
                <div class="test-controls">
                    <button class="start-button" on:click=move |_| test.toggle()>
                        {button_label}
                    </button>
                    <div class="test-options">
                        <select
                            class="test-select"
                            aria-label="Transport"
                            prop:value=move || test.transport.get()
                            on:change=move |ev| test.transport.set(event_target_value(&ev))
                            disabled=running
                        >
                            <option value="websocket">"WebSocket"</option>
                            <option value="http">"HTTP"</option>
                        </select>
                        <select
                            class="test-select"
                            aria-label="Profile"
                            prop:value=move || test.profile.get()
                            on:change=move |ev| test.profile.set(event_target_value(&ev))
                            disabled=running
                        >
                            <option value="quick">"Quick"</option>
                            <option value="standard" selected=true>"Standard"</option>
                        </select>
                    </div>
                </div>
            </div>
            <div class="metric-grid">
                <Metric
                    label="Download"
                    kind="download"
                    value=Signal::derive(move || rate_parts(test.download.get()))
                />
                <Metric
                    label="Upload"
                    kind="upload"
                    value=Signal::derive(move || rate_parts(test.upload.get()))
                />
                <Metric
                    label="Latency"
                    kind="latency"
                    value=Signal::derive(move || ms_parts(test.latency.get()))
                />
                <Metric
                    label="Jitter"
                    kind="jitter"
                    value=Signal::derive(move || ms_parts(test.jitter.get()))
                />
                <Metric
                    label="Packet loss"
                    kind="loss"
                    value=Signal::derive(move || percent_parts(test.packet_loss.get()))
                />
            </div>
        </section>

        <section class="card-row">
            <div class="card">
                <h2>"Network quality score"</h2>
                <ul class="quality-list">
                    <li>
                        <span>"Video streaming"</span>
                        <span class="badge badge-streaming">{badge(|s| s.streaming)}</span>
                    </li>
                    <li>
                        <span>"Online gaming"</span>
                        <span class="badge badge-gaming">{badge(|s| s.gaming)}</span>
                    </li>
                    <li>
                        <span>"Video chatting"</span>
                        <span class="badge badge-rtc">{badge(|s| s.rtc)}</span>
                    </li>
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
fn Metric(
    label: &'static str,
    kind: &'static str,
    value: Signal<(String, &'static str)>,
) -> impl IntoView {
    view! {
        <div class=format!("metric metric-{kind}")>
            <div class="metric-label">{label}</div>
            <div class="metric-value">
                <span class="value">{move || value.get().0}</span>
                <span class="unit">{move || value.get().1}</span>
            </div>
        </div>
    }
}

fn rate_parts(bps: Option<f64>) -> (String, &'static str) {
    format_rate(bps.unwrap_or(f64::NAN), Unit::Mbps)
}

fn ms_parts(ms: Option<f64>) -> (String, &'static str) {
    format_ms(ms.unwrap_or(f64::NAN))
}

fn percent_parts(ratio: Option<f64>) -> (String, &'static str) {
    format_percent(ratio.unwrap_or(f64::NAN))
}

fn class_label(class: Classification) -> &'static str {
    match class {
        Classification::Bad => "Bad",
        Classification::Poor => "Poor",
        Classification::Average => "Average",
        Classification::Good => "Good",
        Classification::Great => "Great",
    }
}

#[cfg_attr(not(feature = "hydrate"), allow(dead_code))]
/// Size for phase labels: `100 kB`, `1 MB`, `1.5 MB`, `1 GB`.
fn format_bytes(bytes: u64) -> String {
    let b = bytes as f64;
    let (v, unit) = if b >= 1e9 {
        (b / 1e9, "GB")
    } else if b >= 1e6 {
        (b / 1e6, "MB")
    } else if b >= 1e3 {
        (b / 1e3, "kB")
    } else {
        (b, "B")
    };
    if (v - v.round()).abs() < 1e-9 {
        format!("{} {unit}", v.round() as u64)
    } else {
        format!("{v:.1} {unit}")
    }
}

#[cfg_attr(not(feature = "hydrate"), allow(dead_code))]
/// One-line description of a profile step, e.g. `Download · 10 MB × 6`.
fn step_label(step: &Step) -> String {
    let plural = |n: u32, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let secs = |ms: u32| format!("{} s", f64::from(ms) / 1000.0);
    match *step {
        Step::Latency { num_packets } => {
            format!("Latency · {}", plural(num_packets, "probe", "probes"))
        }
        Step::Download { bytes, count, .. } => {
            format!("Download · {} × {count}", format_bytes(bytes))
        }
        Step::Upload { bytes, count, .. } => format!("Upload · {} × {count}", format_bytes(bytes)),
        Step::PacketLoss { num_packets, .. } => {
            format!("Packet loss · {}", plural(num_packets, "packet", "packets"))
        }
        Step::TimedLatency { duration_ms } => format!("Latency · {}", secs(duration_ms)),
        Step::TimedDownload { duration_ms } => format!("Download · {}", secs(duration_ms)),
        Step::TimedUpload { duration_ms } => format!("Upload · {}", secs(duration_ms)),
        Step::Pause { duration_ms } => format!("Pause · {}", secs(duration_ms)),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "hydrate"), allow(dead_code))]
enum RunStatus {
    Idle,
    Running,
    Done,
    Failed,
}

/// Minimal glue between the page and the measurement engine: signals the
/// view reads, and (in the browser) a handle to the running test.
#[derive(Clone, Copy)]
struct TestController {
    status: RwSignal<RunStatus>,
    /// `"websocket"` or `"http"`.
    transport: RwSignal<String>,
    /// `"quick"` or `"standard"`.
    profile: RwSignal<String>,
    phase: RwSignal<String>,
    live_bps: RwSignal<Option<f64>>,
    live_direction: RwSignal<Option<Direction>>,
    download: RwSignal<Option<f64>>,
    upload: RwSignal<Option<f64>>,
    latency: RwSignal<Option<f64>>,
    jitter: RwSignal<Option<f64>>,
    packet_loss: RwSignal<Option<f64>>,
    scores: RwSignal<Option<Scores>>,
    #[cfg(feature = "hydrate")]
    handle: StoredValue<Option<crate::engine::EngineHandle>, LocalStorage>,
}

impl TestController {
    fn new() -> Self {
        Self {
            status: RwSignal::new(RunStatus::Idle),
            transport: RwSignal::new("websocket".to_string()),
            profile: RwSignal::new("standard".to_string()),
            phase: RwSignal::new(String::new()),
            live_bps: RwSignal::new(None),
            live_direction: RwSignal::new(None),
            download: RwSignal::new(None),
            upload: RwSignal::new(None),
            latency: RwSignal::new(None),
            jitter: RwSignal::new(None),
            packet_loss: RwSignal::new(None),
            scores: RwSignal::new(None),
            #[cfg(feature = "hydrate")]
            handle: StoredValue::new_local(None),
        }
    }

    /// Starts a test, or stops the running one.
    fn toggle(self) {
        #[cfg(feature = "hydrate")]
        {
            if self.status.get_untracked() == RunStatus::Running {
                self.handle.with_value(|h| {
                    if let Some(h) = h {
                        h.abort();
                    }
                });
            } else {
                self.start();
            }
        }
    }

    #[cfg(feature = "hydrate")]
    fn start(self) {
        use crate::engine::{self, EngineEvent, EngineOptions};
        use crate::model::profile::Profile;
        use crate::model::result::Transport;
        use crate::model::stats;
        use std::cell::RefCell;
        use std::rc::Rc;

        #[derive(Default)]
        struct Acc {
            down: Vec<f64>,
            up: Vec<f64>,
            latency: Vec<f64>,
        }

        let transport = match self.transport.get_untracked().as_str() {
            "http" => Transport::Http,
            _ => Transport::Websocket,
        };
        let profile = Profile::by_name(&self.profile.get_untracked()).unwrap_or_default();
        let opts = EngineOptions::new(transport, profile, engine::detect_device());

        for s in [
            self.download,
            self.upload,
            self.latency,
            self.jitter,
            self.packet_loss,
            self.live_bps,
        ] {
            s.set(None);
        }
        self.live_direction.set(None);
        self.scores.set(None);
        self.phase.set("Connecting…".into());
        self.status.set(RunStatus::Running);

        let acc = Rc::new(RefCell::new(Acc::default()));
        let percentile = opts.config.bandwidth_percentile;
        let this = self;
        let on_event = move |ev: EngineEvent| match ev {
            EngineEvent::Connected { .. } => {}
            EngineEvent::PhaseStarted { step, .. } => {
                this.phase.set(step_label(&step));
                if step.direction().is_none() {
                    this.live_direction.set(None);
                }
            }
            EngineEvent::Latency { ms } => {
                let mut a = acc.borrow_mut();
                a.latency.push(ms);
                this.latency.set(stats::median(&a.latency));
                this.jitter.set(stats::jitter(&a.latency));
            }
            EngineEvent::Bandwidth { direction, point } => {
                let mut a = acc.borrow_mut();
                let (v, sig) = match direction {
                    Direction::Download => (&mut a.down, this.download),
                    Direction::Upload => (&mut a.up, this.upload),
                };
                v.push(point.bps);
                sig.set(stats::percentile(v, percentile));
            }
            EngineEvent::Live { direction, bps } => {
                this.live_direction.set(Some(direction));
                this.live_bps.set(Some(bps));
            }
            EngineEvent::LoadedLatency { .. } | EngineEvent::PhaseFinished { .. } => {}
            EngineEvent::PacketLoss(counts) => this.packet_loss.set(counts.ratio()),
            EngineEvent::Finished(result) => {
                let s = result.summary;
                this.download.set(s.download_bps);
                this.upload.set(s.upload_bps);
                this.latency.set(s.latency_ms);
                this.jitter.set(s.jitter_ms);
                this.packet_loss.set(s.packet_loss_ratio);
                this.scores.set(Some(result.scores));
                this.live_bps.set(None);
                this.live_direction.set(None);
                this.phase.set(format!(
                    "Done in {:.1} s",
                    result.duration_ms as f64 / 1000.0
                ));
                this.status.set(RunStatus::Done);
            }
            EngineEvent::Error(msg) => {
                this.live_bps.set(None);
                this.phase.set(msg);
                this.status.set(RunStatus::Failed);
            }
            EngineEvent::Aborted => {
                this.live_bps.set(None);
                this.phase.set("Stopped".into());
                this.status.set(RunStatus::Failed);
            }
        };
        let handle = engine::start(opts, on_event);
        self.handle.set_value(Some(handle));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_labels() {
        assert_eq!(
            step_label(&Step::download(10_000_000, 6)),
            "Download · 10 MB × 6"
        );
        assert_eq!(step_label(&Step::upload(100_000, 8)), "Upload · 100 kB × 8");
        assert_eq!(
            step_label(&Step::download(1_000_000_000, 2)),
            "Download · 1 GB × 2"
        );
        assert_eq!(
            step_label(&Step::Latency { num_packets: 20 }),
            "Latency · 20 probes"
        );
        assert_eq!(
            step_label(&Step::Latency { num_packets: 1 }),
            "Latency · 1 probe"
        );
        assert_eq!(
            step_label(&Step::default_packet_loss()),
            "Packet loss · 1000 packets"
        );
        assert_eq!(
            step_label(&Step::TimedDownload {
                duration_ms: 10_000
            }),
            "Download · 10 s"
        );
        assert_eq!(
            step_label(&Step::Pause { duration_ms: 2500 }),
            "Pause · 2.5 s"
        );
    }

    #[test]
    fn bytes_labels() {
        assert_eq!(format_bytes(100_000), "100 kB");
        assert_eq!(format_bytes(250_000_000), "250 MB");
        assert_eq!(format_bytes(1_500_000), "1.5 MB");
        assert_eq!(format_bytes(12), "12 B");
    }

    #[test]
    fn display_parts() {
        assert_eq!(rate_parts(None).0, PLACEHOLDER);
        assert_eq!(rate_parts(Some(984e6)), ("984".into(), "Mbps"));
        assert_eq!(ms_parts(Some(2.514)).0, "2.51");
        assert_eq!(percent_parts(None).0, PLACEHOLDER);
        assert_eq!(class_label(Classification::Great), "Great");
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
