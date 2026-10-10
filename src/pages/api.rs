//! Small JSON client for the REST API (SPEC §6.2), used by the pages.
//!
//! Browser only: on the server every call fails with "not in a browser", so
//! callers spawn them from effects or event handlers.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::model::history::{LabelSummary, RunSummary};
use crate::model::result::RunResult;

/// `GET /api/info` (the fields the UI uses).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ServerInfo {
    pub server_name: String,
    pub version: String,
    pub client_ip: Option<String>,
    pub udp_port: u16,
    pub tls: bool,
    pub presence: bool,
    /// Defaults to `true` so an older server still shows the save UI.
    #[serde(default = "yes")]
    pub history: bool,
    pub limits: InfoLimits,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(default)]
pub struct InfoLimits {
    pub max_streams: u8,
}

impl Default for InfoLimits {
    fn default() -> Self {
        Self { max_streams: 8 }
    }
}

/// Response of `POST /api/runs`.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct Saved {
    pub id: Uuid,
    /// `false` when history is off.
    #[serde(default = "yes")]
    pub stored: bool,
}

#[derive(Serialize)]
struct LabelPatch<'a> {
    label: Option<&'a str>,
}

pub async fn server_info() -> Result<ServerInfo, String> {
    let text = imp::request("GET", "/api/info", None).await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

pub async fn labels(device_id: &str) -> Result<Vec<LabelSummary>, String> {
    let url = format!("/api/labels?device={}", encode(device_id));
    let text = imp::request("GET", &url, None).await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// `POST /api/runs?session=<engine session>`.
pub async fn save_run(run: &RunResult, session: Uuid) -> Result<Saved, String> {
    let body = serde_json::to_string(run).map_err(|e| e.to_string())?;
    let url = format!("/api/runs?session={session}");
    let text = imp::request("POST", &url, Some(body)).await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// `PATCH /api/runs/{id}` with a new label (`None` or empty clears it).
pub async fn rename_run(id: Uuid, label: Option<&str>) -> Result<RunSummary, String> {
    let body = serde_json::to_string(&LabelPatch { label }).map_err(|e| e.to_string())?;
    let text = imp::request("PATCH", &format!("/api/runs/{id}"), Some(body)).await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Percent-encodes a query value (RFC 3986 unreserved characters pass).
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(feature = "hydrate")]
mod imp {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;

    fn err(v: wasm_bindgen::JsValue) -> String {
        v.as_string()
            .or_else(|| {
                v.dyn_ref::<js_sys::Error>()
                    .map(|e| String::from(e.message()))
            })
            .unwrap_or_else(|| "network error".into())
    }

    /// Sends a request and returns the body text of a 2xx response.
    pub async fn request(method: &str, url: &str, body: Option<String>) -> Result<String, String> {
        let window = web_sys::window().ok_or("no window")?;
        let init = web_sys::RequestInit::new();
        init.set_method(method);
        init.set_credentials(web_sys::RequestCredentials::SameOrigin);
        let headers = web_sys::Headers::new().map_err(err)?;
        headers.set("Accept", "application/json").map_err(err)?;
        if let Some(body) = &body {
            headers
                .set("Content-Type", "application/json")
                .map_err(err)?;
            init.set_body(&body.into());
        }
        init.set_headers(&headers);
        let req = web_sys::Request::new_with_str_and_init(url, &init).map_err(err)?;
        let resp: web_sys::Response = JsFuture::from(window.fetch_with_request(&req))
            .await
            .map_err(err)?
            .dyn_into()
            .map_err(err)?;
        let text = JsFuture::from(resp.text().map_err(err)?)
            .await
            .map_err(err)?
            .as_string()
            .unwrap_or_default();
        if resp.ok() {
            Ok(text)
        } else {
            Err(format!("HTTP {}: {}", resp.status(), text))
        }
    }
}

#[cfg(not(feature = "hydrate"))]
mod imp {
    pub async fn request(
        _method: &str,
        _url: &str,
        _body: Option<String>,
    ) -> Result<String, String> {
        Err("not in a browser".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_query_values() {
        assert_eq!(encode("abc-123"), "abc-123");
        assert_eq!(encode("a b&c"), "a%20b%26c");
        assert_eq!(encode("·"), "%C2%B7");
    }

    #[test]
    fn info_defaults() {
        let info: ServerInfo = serde_json::from_str(r#"{"server_name":"nas"}"#).unwrap();
        assert!(info.history);
        assert_eq!(info.limits.max_streams, 8);
        let info: ServerInfo =
            serde_json::from_str(r#"{"history":false,"limits":{"max_streams":4}}"#).unwrap();
        assert!(!info.history);
        assert_eq!(info.limits.max_streams, 4);
    }
}
