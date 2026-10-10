//! Server / connection card (SPEC §7.4) from `GET /api/info` plus the test
//! settings.

use leptos::prelude::*;

use crate::model::units::PLACEHOLDER;
use crate::pages::api::ServerInfo;
use crate::ui_state::{PROFILES, TRANSPORTS};

/// "WebSocket · TLS", "HTTP".
pub fn connection_label(transport: &str, tls: bool) -> String {
    let name = TRANSPORTS
        .iter()
        .find(|(k, _)| *k == transport)
        .map_or(transport, |(_, v)| v);
    if tls {
        format!("{name} · TLS")
    } else {
        name.to_string()
    }
}

#[component]
pub fn ServerCard(
    #[prop(into)] info: Signal<Option<ServerInfo>>,
    #[prop(into)] transport: Signal<String>,
    #[prop(into)] profile: Signal<String>,
    #[prop(into)] streams: Signal<u8>,
) -> impl IntoView {
    let field = move |f: fn(&ServerInfo) -> Option<String>| {
        move || {
            info.get()
                .and_then(|i| f(&i))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| PLACEHOLDER.to_string())
        }
    };
    let connection = move || {
        let tls = info.get().is_some_and(|i| i.tls);
        connection_label(&transport.get(), tls)
    };
    let profile_label = move || {
        let p = profile.get();
        PROFILES
            .iter()
            .find(|(k, _)| *k == p)
            .map_or(p.clone(), |(_, v)| v.to_string())
    };
    view! {
        <section class="card server-card" aria-labelledby="server-title">
            <h2 id="server-title" class="card-title">"Server"</h2>
            <dl class="kv">
                <div>
                    <dt>"Name"</dt>
                    <dd class="server-name">{field(|i| Some(i.server_name.clone()))}</dd>
                </div>
                <div>
                    <dt>"Version"</dt>
                    <dd>{field(|i| Some(i.version.clone()))}</dd>
                </div>
                <div>
                    <dt>"Connection"</dt>
                    <dd>{connection}</dd>
                </div>
                <div>
                    <dt>"Your IP"</dt>
                    <dd class="mono">{field(|i| i.client_ip.clone())}</dd>
                </div>
                <div>
                    <dt>"Profile"</dt>
                    <dd>{profile_label}</dd>
                </div>
                <div>
                    <dt>"Streams"</dt>
                    <dd>{move || streams.get().to_string()}</dd>
                </div>
            </dl>
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(connection_label("websocket", true), "WebSocket · TLS");
        assert_eq!(connection_label("http", false), "HTTP");
        assert_eq!(connection_label("other", false), "other");
    }
}
