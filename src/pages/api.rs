//! Small JSON client for the REST API (SPEC §6.2), used by the pages.
//!
//! Browser only: on the server every call fails with "not in a browser", so
//! callers spawn them from effects or event handlers.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::model::history::{CompareResult, GroupRecord, LabelSummary, RunPage, RunSummary};
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

/// Filters of `GET /api/runs` and the export (SPEC §6.2, §7.7).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunsFilter {
    pub label: Option<String>,
    /// Device ID ("this device").
    pub device: Option<String>,
    /// `YYYY-MM-DD` (UTC day) or RFC 3339.
    pub from: Option<String>,
    pub to: Option<String>,
    pub exclude_overlapped: bool,
}

impl RunsFilter {
    /// `key=value` pairs, percent-encoded and joined with `&` (empty values skipped).
    pub fn query(&self) -> String {
        let mut parts = Vec::new();
        let mut push = |k: &str, v: &Option<String>| {
            if let Some(v) = v.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
                parts.push(format!("{k}={}", encode(v)));
            }
        };
        push("label", &self.label);
        push("device", &self.device);
        push("from", &self.from);
        push("to", &self.to);
        if self.exclude_overlapped {
            parts.push("exclude_overlapped=true".into());
        }
        parts.join("&")
    }

    /// `/api/runs/export?format=…&<filters>`.
    pub fn export_url(&self, format: &str) -> String {
        let q = self.query();
        if q.is_empty() {
            format!("/api/runs/export?format={format}")
        } else {
            format!("/api/runs/export?format={format}&{q}")
        }
    }
}

/// `GET /api/runs?<filter>&limit=&cursor=`.
pub async fn list_runs(
    filter: &RunsFilter,
    limit: u32,
    cursor: Option<&str>,
) -> Result<RunPage, String> {
    let mut url = format!("/api/runs?limit={limit}");
    let q = filter.query();
    if !q.is_empty() {
        url.push('&');
        url.push_str(&q);
    }
    if let Some(c) = cursor {
        url.push_str(&format!("&cursor={}", encode(c)));
    }
    let text = imp::request("GET", &url, None).await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// `GET /api/labels` for all devices (`None`) or one.
pub async fn all_labels(device_id: Option<&str>) -> Result<Vec<LabelSummary>, String> {
    let url = match device_id {
        Some(d) => format!("/api/labels?device={}", encode(d)),
        None => "/api/labels".to_string(),
    };
    let text = imp::request("GET", &url, None).await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// `GET /api/runs/{id}`: the full run with its points.
pub async fn get_run(id: Uuid) -> Result<RunResult, String> {
    let text = imp::request("GET", &format!("/api/runs/{id}"), None).await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// `DELETE /api/runs/{id}`.
pub async fn delete_run(id: Uuid) -> Result<(), String> {
    imp::request("DELETE", &format!("/api/runs/{id}"), None)
        .await
        .map(|_| ())
}

/// What to compare: labels or run IDs (SPEC §7.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompareTarget {
    Labels(Vec<String>),
    Ids(Vec<Uuid>),
}

impl CompareTarget {
    /// The `/compare?…` page URL (also the API query).
    pub fn query(&self) -> String {
        match self {
            CompareTarget::Labels(l) => format!(
                "labels={}",
                l.iter().map(|l| encode(l)).collect::<Vec<_>>().join(",")
            ),
            CompareTarget::Ids(ids) => format!(
                "ids={}",
                ids.iter()
                    .map(Uuid::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        }
    }

    pub fn len(&self) -> usize {
        match self {
            CompareTarget::Labels(l) => l.len(),
            CompareTarget::Ids(i) => i.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// `GET /api/compare?labels=…|ids=…`.
pub async fn compare(
    target: &CompareTarget,
    exclude_overlapped: bool,
) -> Result<CompareResult, String> {
    let mut url = format!("/api/compare?{}", target.query());
    if exclude_overlapped {
        url.push_str("&exclude_overlapped=true");
    }
    let text = imp::request("GET", &url, None).await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// `GET /api/groups/{id}`.
pub async fn get_group(id: Uuid) -> Result<GroupRecord, String> {
    let text = imp::request("GET", &format!("/api/groups/{id}"), None).await?;
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
    fn filter_queries() {
        assert_eq!(RunsFilter::default().query(), "");
        let f = RunsFilter {
            label: Some("Desk · 5 GHz".into()),
            device: Some("d1".into()),
            from: Some("2026-10-01".into()),
            to: Some(" ".into()),
            exclude_overlapped: true,
        };
        assert_eq!(
            f.query(),
            "label=Desk%20%C2%B7%205%20GHz&device=d1&from=2026-10-01&exclude_overlapped=true"
        );
        assert!(
            f.export_url("csv")
                .starts_with("/api/runs/export?format=csv&label=")
        );
        assert_eq!(
            RunsFilter::default().export_url("json"),
            "/api/runs/export?format=json"
        );
        let t = CompareTarget::Labels(vec!["a b".into(), "c,d".into()]);
        assert_eq!(t.query(), "labels=a%20b,c%2Cd");
        assert_eq!(t.len(), 2);
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
