#[cfg(feature = "ssr")]
fn main() -> std::process::ExitCode {
    use clap::Parser;
    use speed_test_rs::server::config::{Cli, Command, Config, healthcheck_listen};
    use speed_test_rs::server::healthcheck;
    use std::process::ExitCode;

    match Cli::parse().into_command() {
        // `speed-test-rs healthcheck` is used by the Docker HEALTHCHECK so the runtime
        // image does not need curl.
        Command::Healthcheck(args) => match healthcheck_listen(&args) {
            Ok(listen) => ExitCode::from(healthcheck::run(&listen) as u8),
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::from(2)
            }
        },
        Command::Serve(args) => {
            let config = match Config::from_args(args) {
                Ok(config) => config,
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::from(2);
                }
            };
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("failed to start the Tokio runtime");
            match runtime.block_on(serve(config)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    tracing::error!("{e}");
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

#[cfg(feature = "ssr")]
async fn serve(config: speed_test_rs::server::Config) -> Result<(), String> {
    use speed_test_rs::server::{AppState, assets, listen, router, rtc, startup, storage};

    startup::init_logging(&config);

    if config.history
        && let Err(e) = std::fs::create_dir_all(&config.data_dir)
    {
        tracing::warn!(
            "cannot create the data directory {}: {e}",
            config.data_dir.display()
        );
    }

    let http = listen::bind_all(&config.listen).map_err(|e| e.to_string())?;
    let tls = match &config.tls {
        Some(tls) => {
            let rustls = listen::load_tls(&tls.cert, &tls.key)?;
            let listeners = listen::bind_all(&tls.listen).map_err(|e| e.to_string())?;
            Some((listeners, rustls))
        }
        None => None,
    };

    let mut state = AppState::new(config);
    state.storage = storage::Storage::from_config(&state.config)
        .await
        .map_err(|e| format!("cannot open the history database: {e}"))?;
    if assets::embedded_wasm_present(&state.leptos.output_name) == Some(false) {
        tracing::warn!(
            "the embedded site has no pkg/{}.wasm; the page will not hydrate",
            state.leptos.output_name
        );
    }

    // WebRTC packet-loss endpoint (SPEC §5.7). Without it the test reports "unavailable".
    let rtc = match rtc::RtcEcho::start(&state.config) {
        Ok(echo) => {
            let description = echo.describe().to_owned();
            state.measure.rtc = Some(std::sync::Arc::new(echo));
            Some(description)
        }
        Err(e) => {
            tracing::warn!("WebRTC packet loss test unavailable: {e}");
            None
        }
    };

    let local_addrs = |listeners: &[std::net::TcpListener]| {
        listeners
            .iter()
            .filter_map(|l| l.local_addr().ok())
            .collect::<Vec<_>>()
    };
    let bound = startup::Bound {
        config: &state.config,
        http: local_addrs(&http),
        https: tls
            .as_ref()
            .map(|(listeners, _)| local_addrs(listeners))
            .unwrap_or_default(),
        assets: state.assets.describe(),
        rtc,
    };
    bound.print_banner();
    // The sockets are already listening, so the browser can connect right away.
    if state.config.open
        && let Some(url) = bound.local_url()
    {
        startup::open_browser(&url);
    }
    tracing::info!("ready");

    let app = router(state);
    listen::serve(app, http, tls, listen::shutdown_signal())
        .await
        .map_err(|e| e.to_string())
}

#[cfg(not(feature = "ssr"))]
pub fn main() {
    // no client-side main function
    // see lib.rs for hydration function instead
}
