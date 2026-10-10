#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() {
    use axum::Router;
    use leptos::logging::log;
    use leptos::prelude::*;
    use leptos_axum::{LeptosRoutes, generate_route_list};
    use speed_test_rs::app::*;
    use speed_test_rs::server;

    // `speed-test-rs healthcheck` is used by the Docker HEALTHCHECK so the runtime
    // image does not need curl.
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        std::process::exit(server::healthcheck::run());
    }

    // `get_configuration(None)` reads the [package.metadata.leptos] values baked in at
    // build time, overridden by LEPTOS_* environment variables at runtime
    // (e.g. LEPTOS_SITE_ADDR=0.0.0.0:8080 in the Docker image).
    let conf = get_configuration(None).unwrap();
    let leptos_options = conf.leptos_options;
    let addr = leptos_options.site_addr;
    let routes = generate_route_list(App);

    let app = Router::new()
        .merge(server::api::routes())
        .leptos_routes(&leptos_options, routes, {
            let leptos_options = leptos_options.clone();
            move || shell(leptos_options.clone())
        })
        .fallback(leptos_axum::file_and_error_handler(shell))
        .with_state(leptos_options);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    log!("listening on http://{}", &addr);
    axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap();
}

/// Resolves on Ctrl+C or SIGTERM so `docker stop` shuts the server down cleanly.
#[cfg(feature = "ssr")]
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.ok();
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut sig) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            sig.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[cfg(not(feature = "ssr"))]
pub fn main() {
    // no client-side main function
    // see lib.rs for hydration function instead
}
