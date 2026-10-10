//! UI assets: `/pkg/*` (JS, WASM, CSS) and the `public/` files (fonts, favicon, ...).
//!
//! With the `embed-assets` feature the cargo-leptos site output is compiled into the binary
//! (`rust-embed`), together with Brotli and gzip variants made by `build.rs`. Otherwise, or
//! when `--site-root` is given, files are read from disk. Requests that match no asset fall
//! through to the Leptos 404 page (this replaces `leptos_axum::file_and_error_handler`).
//! See docs/SPEC.md §10.1.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::header::{
    ACCEPT_ENCODING, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_TYPE, ETAG, IF_NONE_MATCH, VARY,
};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use leptos::prelude::provide_context;

use super::config::Config;
use super::state::AppState;
use crate::app::shell;

/// Cache policy for content-hashed file names.
pub const CACHE_IMMUTABLE: &str = "public, max-age=31536000, immutable";
/// Cache policy for everything else: always revalidate (cheap thanks to the ETag).
pub const CACHE_NO_CACHE: &str = "no-cache";

/// Where UI assets come from.
#[derive(Clone, Debug)]
pub enum Assets {
    /// Compiled into the binary (`embed-assets` feature).
    #[cfg(feature = "embed-assets")]
    Embedded,
    /// Read from this directory (the cargo-leptos site root).
    Disk(Arc<Path>),
}

impl Assets {
    pub fn from_config(config: &Config) -> Self {
        match &config.site_root {
            Some(root) => Self::Disk(root.as_path().into()),
            #[cfg(feature = "embed-assets")]
            None => Self::Embedded,
            #[cfg(not(feature = "embed-assets"))]
            None => Self::Disk(Path::new("target/site").into()),
        }
    }

    /// Human-readable description for the startup output.
    pub fn describe(&self) -> String {
        match self {
            #[cfg(feature = "embed-assets")]
            Self::Embedded => "embedded".to_owned(),
            Self::Disk(root) => format!("from disk ({})", root.display()),
        }
    }

    /// Serves `path` (the request path) if it names an asset; `None` otherwise.
    pub async fn response(&self, path: &str, headers: &HeaderMap) -> Option<Response> {
        let rel = sanitize(path)?;
        let file = match self {
            #[cfg(feature = "embed-assets")]
            Self::Embedded => embedded::lookup(&rel, &accepted_encodings(headers))?,
            Self::Disk(root) => disk_lookup(root, &rel).await?,
        };
        Some(file.into_response(&rel, headers))
    }
}

/// Axum fallback: serve an asset, or render the Leptos 404 page.
pub async fn fallback(State(state): State<AppState>, req: Request) -> Response {
    if matches!(*req.method(), Method::GET | Method::HEAD)
        && let Some(res) = state.assets.response(req.uri().path(), req.headers()).await
    {
        return res;
    }
    not_found(state, req).await
}

/// Renders the app shell for an unknown path; the router's fallback route sets 404.
async fn not_found(state: AppState, req: Request) -> Response {
    let options = state.leptos.clone();
    let handler = leptos_axum::render_app_to_stream_with_context(
        move || provide_context(state.clone()),
        move || shell(options.clone()),
    );
    let mut res = handler(req).await;
    if res.status() == StatusCode::OK {
        *res.status_mut() = StatusCode::NOT_FOUND;
    }
    res
}

/// A located asset, ready to be turned into a response.
struct Found {
    body: Cow<'static, [u8]>,
    etag: String,
    /// `Content-Encoding` of `body`, if it is a pre-compressed variant.
    encoding: Option<&'static str>,
    /// Whether compressed variants exist (so the response depends on `Accept-Encoding`).
    vary: bool,
}

impl Found {
    fn into_response(self, rel: &str, req_headers: &HeaderMap) -> Response {
        let mut headers = HeaderMap::new();
        if let Ok(etag) = HeaderValue::from_str(&self.etag) {
            headers.insert(ETAG, etag);
        }
        headers.insert(CACHE_CONTROL, HeaderValue::from_static(cache_control(rel)));
        if self.vary {
            headers.insert(VARY, HeaderValue::from_static("accept-encoding"));
        }

        if etag_matches(req_headers, &self.etag) {
            return (StatusCode::NOT_MODIFIED, headers).into_response();
        }

        headers.insert(CONTENT_TYPE, content_type(rel));
        if let Some(encoding) = self.encoding {
            headers.insert(CONTENT_ENCODING, HeaderValue::from_static(encoding));
        }
        (headers, Body::from(self.body)).into_response()
    }
}

/// Turns a request path into a relative asset path, rejecting anything that could escape
/// the site root (`..`, `.`, empty segments, backslashes, drive letters).
fn sanitize(path: &str) -> Option<String> {
    let rel = path.strip_prefix('/')?;
    if rel.is_empty() {
        return None;
    }
    let ok = rel.split('/').all(|seg| {
        !seg.is_empty() && seg != "." && seg != ".." && !seg.contains(['\\', ':', '\0'])
    });
    ok.then(|| rel.to_owned())
}

/// `Content-Type` from the file extension. `.wasm` must be `application/wasm` so browsers
/// can stream-compile it.
pub fn content_type(path: &str) -> HeaderValue {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let value = match ext.as_str() {
        "wasm" => Cow::Borrowed("application/wasm"),
        "webmanifest" => Cow::Borrowed("application/manifest+json"),
        "js" | "mjs" => Cow::Borrowed("text/javascript; charset=utf-8"),
        "woff2" => Cow::Borrowed("font/woff2"),
        _ => {
            let mime = mime_guess::from_ext(&ext).first_or_octet_stream();
            if mime.type_() == mime_guess::mime::TEXT {
                Cow::Owned(format!("{mime}; charset=utf-8"))
            } else {
                Cow::Owned(mime.to_string())
            }
        }
    };
    HeaderValue::from_str(&value).unwrap_or(HeaderValue::from_static("application/octet-stream"))
}

/// `Cache-Control` for an asset path: immutable for content-hashed names, `no-cache`
/// otherwise (and always for the service worker, which must be revalidated).
pub fn cache_control(path: &str) -> &'static str {
    let name = path.rsplit('/').next().unwrap_or(path);
    if name != "sw.js" && is_hashed_name(name) {
        CACHE_IMMUTABLE
    } else {
        CACHE_NO_CACHE
    }
}

/// Whether a file name carries a content hash, as cargo-leptos writes them with
/// `hash-files = true` (`<name>.<hash>.<ext>`, the hash being at least 10 alphanumeric
/// characters with both letters and digits).
fn is_hashed_name(name: &str) -> bool {
    let mut parts = name.rsplit('.');
    let (Some(_ext), Some(hash), Some(_stem)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    hash.len() >= 10
        && hash
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && hash.chars().any(|c| c.is_ascii_digit())
        && hash.chars().any(|c| c.is_ascii_alphabetic())
}

/// Weak comparison of `If-None-Match` against our ETag (RFC 9110 §13.1.2).
fn etag_matches(headers: &HeaderMap, etag: &str) -> bool {
    let strip = |tag: &str| tag.trim().trim_start_matches("W/").to_owned();
    let ours = strip(etag);
    headers
        .get_all(IF_NONE_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|tag| tag.trim() == "*" || strip(tag) == ours)
}

/// Content codings the client accepts, in our order of preference.
#[cfg_attr(not(feature = "embed-assets"), allow(dead_code))]
fn accepted_encodings(headers: &HeaderMap) -> Vec<&'static str> {
    let mut accepted = Vec::new();
    for value in headers
        .get_all(ACCEPT_ENCODING)
        .iter()
        .filter_map(|v| v.to_str().ok())
    {
        for item in value.split(',') {
            let mut parts = item.split(';');
            let coding = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
            let q = parts
                .find_map(|p| p.trim().strip_prefix("q=").map(str::to_owned))
                .and_then(|q| q.trim().parse::<f32>().ok())
                .unwrap_or(1.0);
            if q <= 0.0 {
                continue;
            }
            match coding.as_str() {
                "br" => accepted.push("br"),
                "gzip" | "x-gzip" => accepted.push("gzip"),
                _ => {}
            }
        }
    }
    accepted.sort_by_key(|c| if *c == "br" { 0 } else { 1 });
    accepted.dedup();
    accepted
}

async fn disk_lookup(root: &Path, rel: &str) -> Option<Found> {
    let path: PathBuf = root.join(rel);
    let meta = tokio::fs::metadata(&path).await.ok()?;
    if !meta.is_file() {
        return None;
    }
    let body = tokio::fs::read(&path).await.ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    Some(Found {
        etag: format!("W/\"{:x}-{:x}\"", body.len(), mtime),
        body: Cow::Owned(body),
        encoding: None,
        vary: false,
    })
}

#[cfg(feature = "embed-assets")]
mod embedded {
    use super::Found;
    use rust_embed::RustEmbed;
    use std::fmt::Write;

    /// The cargo-leptos site output (`target/site`), located by `build.rs`.
    #[derive(RustEmbed)]
    #[folder = "$SPEEDTEST_SITE_DIR"]
    struct Site;

    /// Brotli variants of the compressible files, made by `build.rs`.
    #[derive(RustEmbed)]
    #[folder = "$SPEEDTEST_SITE_BR_DIR"]
    struct SiteBr;

    /// gzip variants of the compressible files, made by `build.rs`.
    #[derive(RustEmbed)]
    #[folder = "$SPEEDTEST_SITE_GZ_DIR"]
    struct SiteGz;

    pub(super) fn lookup(rel: &str, accepted: &[&'static str]) -> Option<Found> {
        let file = Site::get(rel)?;
        let mut etag = String::with_capacity(36);
        etag.push('"');
        for byte in &file.metadata.sha256_hash()[..16] {
            let _ = write!(etag, "{byte:02x}");
        }

        let br = SiteBr::get(rel);
        let gz = SiteGz::get(rel);
        let vary = br.is_some() || gz.is_some();
        let variant = accepted.iter().find_map(|coding| match *coding {
            "br" => br.as_ref().map(|f| (f.data.clone(), "br")),
            "gzip" => gz.as_ref().map(|f| (f.data.clone(), "gzip")),
            _ => None,
        });

        Some(match variant {
            Some((body, coding)) => {
                etag.push('-');
                etag.push_str(coding);
                etag.push('"');
                Found {
                    body,
                    etag,
                    encoding: Some(coding),
                    vary,
                }
            }
            None => {
                etag.push('"');
                Found {
                    body: file.data,
                    etag,
                    encoding: None,
                    vary,
                }
            }
        })
    }

    /// Whether the embedded site contains `rel` (used by tests and the startup check).
    pub fn contains(rel: &str) -> bool {
        Site::get(rel).is_some()
    }
}

/// Whether the embedded site has the hydration bundle; `None` without `embed-assets`.
pub fn embedded_wasm_present(output_name: &str) -> Option<bool> {
    #[cfg(feature = "embed-assets")]
    {
        Some(embedded::contains(&format!("pkg/{output_name}.wasm")))
    }
    #[cfg(not(feature = "embed-assets"))]
    {
        let _ = output_name;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_types() {
        assert_eq!(content_type("pkg/app.wasm"), "application/wasm");
        assert_eq!(content_type("pkg/app.css"), "text/css; charset=utf-8");
        assert_eq!(content_type("pkg/app.js"), "text/javascript; charset=utf-8");
        assert_eq!(content_type("fonts/inter.woff2"), "font/woff2");
        assert_eq!(
            content_type("manifest.webmanifest"),
            "application/manifest+json"
        );
        assert_eq!(content_type("favicon.ico"), "image/x-icon");
        assert_eq!(content_type("blob"), "application/octet-stream");
    }

    #[test]
    fn cache_policy() {
        assert_eq!(cache_control("pkg/speed-test-rs.wasm"), CACHE_NO_CACHE);
        assert_eq!(
            cache_control("pkg/speed-test-rs.3f9a8c1d2e7b4a60.wasm"),
            CACHE_IMMUTABLE
        );
        assert_eq!(cache_control("fonts/inter-latin-var.woff2"), CACHE_NO_CACHE);
        assert_eq!(
            cache_control("fonts/ibm-plex-mono-latin-500.woff2"),
            CACHE_NO_CACHE
        );
        assert_eq!(cache_control("sw.js"), CACHE_NO_CACHE);
        assert_eq!(cache_control("manifest.webmanifest"), CACHE_NO_CACHE);
    }

    #[test]
    fn sanitizes_paths() {
        assert_eq!(sanitize("/pkg/a.js").as_deref(), Some("pkg/a.js"));
        assert_eq!(sanitize("/"), None);
        assert_eq!(sanitize("/../etc/passwd"), None);
        assert_eq!(sanitize("/pkg/../../x"), None);
        assert_eq!(sanitize("/pkg//x"), None);
        assert_eq!(sanitize("/pkg/./x"), None);
        assert_eq!(sanitize("/c:/x"), None);
        assert_eq!(sanitize("/a\\b"), None);
    }

    #[test]
    fn encodings() {
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT_ENCODING,
            HeaderValue::from_static("gzip, deflate, br;q=0.9"),
        );
        assert_eq!(accepted_encodings(&headers), vec!["br", "gzip"]);
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip, br;q=0"));
        assert_eq!(accepted_encodings(&headers), vec!["gzip"]);
        assert!(accepted_encodings(&HeaderMap::new()).is_empty());
    }

    #[test]
    fn if_none_match() {
        let mut headers = HeaderMap::new();
        headers.insert(IF_NONE_MATCH, HeaderValue::from_static("\"a\", W/\"b\""));
        assert!(etag_matches(&headers, "\"b\""));
        assert!(etag_matches(&headers, "W/\"a\""));
        assert!(!etag_matches(&headers, "\"c\""));
        headers.insert(IF_NONE_MATCH, HeaderValue::from_static("*"));
        assert!(etag_matches(&headers, "\"c\""));
    }
}
