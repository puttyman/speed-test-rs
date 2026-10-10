//! The Axum router: API routes, Leptos SSR routes, and the asset/404 fallback.

use axum::Router;
use axum::http::HeaderValue;
use axum::http::header::CACHE_CONTROL;
use axum::middleware;
use axum::response::Response;
use leptos_axum::{LeptosRoutes, generate_route_list};

use super::state::AppState;
use super::{api, assets, http_measure};
use crate::app::{App, shell};

/// Builds the application router.
///
/// Other modules add their routes by merging a `Router<AppState>` (or a state-generic
/// `Router<S>`, like [`api::routes`]) here, before the Leptos routes.
pub fn router(state: AppState) -> Router {
    let routes = generate_route_list(App);
    Router::new()
        .merge(api::routes())
        .merge(http_measure::routes())
        .leptos_routes(&state, routes, {
            let options = state.leptos.clone();
            move || shell(options.clone())
        })
        .fallback(assets::fallback)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            http_measure::require_token,
        ))
        .layer(middleware::map_response(default_no_cache))
        .with_state(state)
}

/// HTML (and anything else that doesn't set its own policy) is served with `no-cache`.
async fn default_no_cache(mut res: Response) -> Response {
    res.headers_mut()
        .entry(CACHE_CONTROL)
        .or_insert(HeaderValue::from_static(assets::CACHE_NO_CACHE));
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::assets::{Assets, CACHE_IMMUTABLE, CACHE_NO_CACHE};
    use crate::server::config::{Config, ServeArgs};
    use axum::body::Body;
    use axum::http::header::{CONTENT_TYPE, ETAG, IF_NONE_MATCH};
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use std::path::{Path, PathBuf};
    use tower::ServiceExt;

    /// A throwaway site root with a few files, removed on drop.
    struct TempSite(PathBuf);

    impl TempSite {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "speed-test-rs-assets-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            for (path, data) in [
                ("pkg/speed-test-rs.wasm", &b"\0asm\x01\0\0\0"[..]),
                ("pkg/speed-test-rs.css", b"body{}"),
                ("pkg/speed-test-rs.js", b"export {}"),
                ("pkg/speed-test-rs.0123456789abcdef.js", b"export {}"),
                ("fonts/inter-latin-var.woff2", b"wOF2"),
                ("sw.js", b"self"),
                ("manifest.webmanifest", b"{}"),
            ] {
                let file = dir.join(path);
                std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                std::fs::write(file, data).unwrap();
            }
            Self(dir)
        }
    }

    impl Drop for TempSite {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn state_for(site: &Path) -> AppState {
        let args = ServeArgs {
            listen: vec!["127.0.0.1:0".into()],
            site_root: Some(site.to_path_buf()),
            ..ServeArgs::default()
        };
        let config = Config::resolve(args, |_| None).unwrap();
        let state = AppState::new(config);
        assert!(matches!(state.assets, Assets::Disk(_)));
        state
    }

    async fn get(app: &Router, uri: &str, headers: &[(&str, &str)]) -> Response {
        let mut req = Request::get(uri);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        app.clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    fn header(res: &Response, name: impl axum::http::header::AsHeaderName) -> &str {
        res.headers()
            .get(name)
            .map(|v| v.to_str().unwrap())
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn serves_assets_with_types_and_cache_headers() {
        let site = TempSite::new("types");
        let app = router(state_for(&site.0));

        let res = get(&app, "/pkg/speed-test-rs.wasm", &[]).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(header(&res, CONTENT_TYPE), "application/wasm");
        assert_eq!(header(&res, CACHE_CONTROL), CACHE_NO_CACHE);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"\0asm\x01\0\0\0");

        let res = get(&app, "/pkg/speed-test-rs.css", &[]).await;
        assert_eq!(header(&res, CONTENT_TYPE), "text/css; charset=utf-8");

        let res = get(&app, "/fonts/inter-latin-var.woff2", &[]).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(header(&res, CONTENT_TYPE), "font/woff2");

        let res = get(&app, "/pkg/speed-test-rs.0123456789abcdef.js", &[]).await;
        assert_eq!(header(&res, CACHE_CONTROL), CACHE_IMMUTABLE);

        let res = get(&app, "/sw.js", &[]).await;
        assert_eq!(header(&res, CACHE_CONTROL), CACHE_NO_CACHE);
        assert_eq!(header(&res, CONTENT_TYPE), "text/javascript; charset=utf-8");

        let res = get(&app, "/manifest.webmanifest", &[]).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(header(&res, CONTENT_TYPE), "application/manifest+json");
    }

    #[tokio::test]
    async fn etag_and_304() {
        let site = TempSite::new("etag");
        let app = router(state_for(&site.0));

        let res = get(&app, "/pkg/speed-test-rs.css", &[]).await;
        let etag = header(&res, ETAG).to_owned();
        assert!(!etag.is_empty());

        let res = get(
            &app,
            "/pkg/speed-test-rs.css",
            &[(IF_NONE_MATCH.as_str(), &etag)],
        )
        .await;
        assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(header(&res, ETAG), etag);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        assert!(body.is_empty());

        let res = get(
            &app,
            "/pkg/speed-test-rs.css",
            &[(IF_NONE_MATCH.as_str(), "\"x\"")],
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn unknown_paths_fall_through_to_leptos_404() {
        let site = TempSite::new("404");
        let app = router(state_for(&site.0));

        for uri in ["/pkg/missing.wasm", "/no/such/page", "/../Cargo.toml"] {
            let res = get(&app, uri, &[]).await;
            assert_eq!(res.status(), StatusCode::NOT_FOUND, "{uri}");
            assert!(header(&res, CONTENT_TYPE).starts_with("text/html"), "{uri}");
            let body = res.into_body().collect().await.unwrap().to_bytes();
            let html = String::from_utf8_lossy(&body);
            assert!(html.contains("Page not found"), "{uri}: {html}");
        }

        // A directory is not an asset.
        let res = get(&app, "/pkg", &[]).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn html_and_api_are_not_cached() {
        let site = TempSite::new("html");
        let app = router(state_for(&site.0));

        let res = get(&app, "/", &[]).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(header(&res, CACHE_CONTROL), CACHE_NO_CACHE);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&body).contains("/pkg/speed-test-rs.css"));

        let res = get(&app, "/api/health", &[]).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(header(&res, CACHE_CONTROL), CACHE_NO_CACHE);
    }

    #[cfg(feature = "embed-assets")]
    #[tokio::test]
    async fn embedded_assets() {
        use axum::http::header::CONTENT_ENCODING;

        let args = ServeArgs {
            listen: vec!["127.0.0.1:0".into()],
            ..ServeArgs::default()
        };
        let state = AppState::new(Config::resolve(args, |_| None).unwrap());
        assert!(matches!(state.assets, Assets::Embedded));
        let app = router(state);

        let res = get(&app, "/pkg/speed-test-rs.wasm", &[]).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(header(&res, CONTENT_TYPE), "application/wasm");
        assert!(header(&res, CONTENT_ENCODING).is_empty());
        let etag = header(&res, ETAG).to_owned();

        let res = get(
            &app,
            "/pkg/speed-test-rs.wasm",
            &[("accept-encoding", "gzip, br")],
        )
        .await;
        assert_eq!(header(&res, CONTENT_ENCODING), "br");
        assert_eq!(header(&res, "vary"), "accept-encoding");
        assert_ne!(header(&res, ETAG), etag);

        let res = get(
            &app,
            "/pkg/speed-test-rs.wasm",
            &[(IF_NONE_MATCH.as_str(), &etag)],
        )
        .await;
        assert_eq!(res.status(), StatusCode::NOT_MODIFIED);

        let res = get(
            &app,
            "/fonts/inter-latin-var.woff2",
            &[("accept-encoding", "br")],
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        assert!(header(&res, CONTENT_ENCODING).is_empty());

        let res = get(&app, "/pkg/nope.js", &[]).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }
}
