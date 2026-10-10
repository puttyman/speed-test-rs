//! Logging setup and the startup output (docs/SPEC.md §9.4).

use std::fmt::Write;
use std::io::IsTerminal;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use qrcode::QrCode;
use qrcode::render::unicode::Dense1x2;
use tracing_subscriber::EnvFilter;

use super::config::{Config, DEFAULT_LOG};

/// Initialises `tracing` from `--log` / `RUST_LOG` (already resolved into `config.log`).
pub fn init_logging(config: &Config) {
    let filter = EnvFilter::try_new(&config.log).unwrap_or_else(|e| {
        eprintln!(
            "invalid log filter `{}` ({e}); using `{DEFAULT_LOG}`",
            config.log
        );
        EnvFilter::new(DEFAULT_LOG)
    });
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .init();
}

/// What the server actually bound, for the startup output.
pub struct Bound<'a> {
    pub config: &'a Config,
    /// Local addresses of the HTTP listeners.
    pub http: Vec<SocketAddr>,
    /// Local addresses of the HTTPS listeners.
    pub https: Vec<SocketAddr>,
    /// Where the UI assets come from (e.g. "embedded").
    pub assets: String,
    /// The WebRTC endpoint (e.g. `udp/3478 on 192.168.1.10`); `None` if it failed to start.
    pub rtc: Option<String>,
}

impl Bound<'_> {
    /// The URL to open on this machine (`--open`).
    pub fn local_url(&self) -> Option<String> {
        local_url("http", &self.http).or_else(|| local_url("https", &self.https))
    }

    /// The URLs other devices on the LAN can use, best first.
    pub fn lan_urls(&self) -> Vec<String> {
        let interfaces = interface_ips();
        let mut urls = lan_urls("http", &self.http, &interfaces);
        urls.extend(lan_urls("https", &self.https, &interfaces));
        urls
    }

    /// Prints the startup banner (version, data dir, URLs and a QR code) to stdout.
    pub fn print_banner(&self) {
        print!("{}", self.banner());
    }

    fn banner(&self) -> String {
        let config = self.config;
        let mut out = String::new();
        let _ = writeln!(out, "\nspeed-test-rs {}", env!("CARGO_PKG_VERSION"));
        let history = if config.history {
            String::new()
        } else {
            " (history off)".to_owned()
        };
        let _ = writeln!(out, "  data dir   {}{history}", config.data_dir.display());
        let _ = writeln!(out, "  assets     {}", self.assets);
        let _ = writeln!(
            out,
            "  webrtc     {}",
            self.rtc
                .as_deref()
                .unwrap_or("unavailable (packet loss is not measured)")
        );

        let listening = join_addrs(&self.http);
        if config.listens_on_all_interfaces() {
            let port = self.http.first().map_or(0, SocketAddr::port);
            let _ = writeln!(
                out,
                "  listening on all interfaces ({listening}); use --listen 127.0.0.1:{port} to restrict"
            );
        } else {
            let _ = writeln!(out, "  listening on {listening}");
        }
        if !self.https.is_empty() {
            let _ = writeln!(out, "  https on {}", join_addrs(&self.https));
        }

        if let Some(url) = self.local_url() {
            let _ = writeln!(out, "  local      {url}");
        }
        let lan = self.lan_urls();
        for (i, url) in lan.iter().enumerate() {
            let label = if i == 0 { "network" } else { "" };
            let _ = writeln!(out, "  {label:<10} {url}");
        }
        if config.qr
            && let Some(first) = lan.first()
            && let Some(qr) = qr_code(first)
        {
            let _ = writeln!(out, "\n  Scan to open {first} on your phone:\n{qr}");
        }
        out
    }
}

fn join_addrs(addrs: &[SocketAddr]) -> String {
    addrs
        .iter()
        .map(SocketAddr::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn url(scheme: &str, ip: IpAddr, port: u16) -> String {
    match ip {
        IpAddr::V4(ip) => format!("{scheme}://{ip}:{port}"),
        IpAddr::V6(ip) => format!("{scheme}://[{ip}]:{port}"),
    }
}

fn local_url(scheme: &str, addrs: &[SocketAddr]) -> Option<String> {
    addrs.iter().find_map(|addr| {
        let ip = addr.ip();
        if ip.is_unspecified() || ip.is_loopback() {
            Some(match ip {
                IpAddr::V4(_) => format!("{scheme}://localhost:{}", addr.port()),
                IpAddr::V6(_) => url(scheme, IpAddr::V6(Ipv6Addr::LOCALHOST), addr.port()),
            })
        } else {
            None
        }
    })
}

fn interface_ips() -> Vec<IpAddr> {
    match if_addrs::get_if_addrs() {
        Ok(ifaces) => ifaces.into_iter().map(|iface| iface.ip()).collect(),
        Err(e) => {
            tracing::warn!("cannot list network interfaces: {e}");
            Vec::new()
        }
    }
}

/// Whether an address is worth showing as a LAN URL (not loopback, not link-local).
fn reachable(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified(),
        IpAddr::V6(ip) => {
            !ip.is_loopback() && !ip.is_unspecified() && (ip.segments()[0] & 0xffc0) != 0xfe80
        }
    }
}

/// Ranks addresses so the most likely home-LAN address comes first (it gets the QR code):
/// 192.168/16, 10/8, 172.16/12, other IPv4, then IPv6.
fn rank(ip: &IpAddr) -> u8 {
    match ip {
        IpAddr::V4(ip) => match ip.octets() {
            [192, 168, ..] => 0,
            [10, ..] => 1,
            [172, b, ..] if (16..32).contains(&b) => 2,
            _ => 3,
        },
        IpAddr::V6(_) => 4,
    }
}

fn lan_urls(scheme: &str, bound: &[SocketAddr], interfaces: &[IpAddr]) -> Vec<String> {
    let mut ips: Vec<(IpAddr, u16)> = Vec::new();
    for addr in bound {
        let candidates: Vec<IpAddr> = match addr.ip() {
            IpAddr::V4(Ipv4Addr::UNSPECIFIED) => interfaces
                .iter()
                .filter(|ip| ip.is_ipv4())
                .copied()
                .collect(),
            IpAddr::V6(ip) if ip.is_unspecified() => interfaces
                .iter()
                .filter(|ip| ip.is_ipv6())
                .copied()
                .collect(),
            ip => vec![ip],
        };
        for ip in candidates.into_iter().filter(reachable) {
            if !ips.contains(&(ip, addr.port())) {
                ips.push((ip, addr.port()));
            }
        }
    }
    ips.sort_by_key(|(ip, _)| rank(ip));
    ips.into_iter()
        .map(|(ip, port)| url(scheme, ip, port))
        .collect()
}

/// Renders `text` as a QR code with Unicode half blocks, two modules per character row.
/// Colours are inverted for the usual dark terminal background.
fn qr_code(text: &str) -> Option<String> {
    let code = QrCode::new(text.as_bytes()).ok()?;
    let rendered = code
        .render::<Dense1x2>()
        .dark_color(Dense1x2::Light)
        .light_color(Dense1x2::Dark)
        .quiet_zone(true)
        .build();
    Some(rendered.lines().map(|line| format!("  {line}\n")).collect())
}

/// Opens `url` in the default browser (`--open`).
pub fn open_browser(url: &str) {
    if let Err(e) = open::that_detached(url) {
        tracing::warn!("could not open a browser for {url}: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ips(list: &[&str]) -> Vec<IpAddr> {
        list.iter().map(|s| s.parse().unwrap()).collect()
    }

    #[test]
    fn urls_for_all_interfaces() {
        let interfaces = ips(&[
            "127.0.0.1",
            "172.17.0.1",
            "192.168.1.10",
            "169.254.3.4",
            "::1",
            "fe80::1",
            "fd00::10",
        ]);
        let bound = crate::server::config::dual_stack(8080);
        assert_eq!(
            lan_urls("http", &bound, &interfaces),
            vec![
                "http://192.168.1.10:8080",
                "http://172.17.0.1:8080",
                "http://[fd00::10]:8080"
            ]
        );
        assert_eq!(
            local_url("http", &bound).as_deref(),
            Some("http://localhost:8080")
        );
    }

    #[test]
    fn urls_for_specific_addresses() {
        let interfaces = ips(&["192.168.1.10", "10.0.0.2"]);
        let bound = vec!["127.0.0.1:9000".parse().unwrap()];
        assert!(lan_urls("http", &bound, &interfaces).is_empty());
        assert_eq!(
            local_url("http", &bound).as_deref(),
            Some("http://localhost:9000")
        );

        let bound = vec!["10.0.0.2:9000".parse().unwrap()];
        assert_eq!(
            lan_urls("https", &bound, &interfaces),
            vec!["https://10.0.0.2:9000"]
        );
        assert_eq!(local_url("http", &bound), None);
    }

    #[test]
    fn qr_renders() {
        let qr = qr_code("http://192.168.1.10:8080").unwrap();
        assert!(qr.lines().count() > 10);
    }
}
