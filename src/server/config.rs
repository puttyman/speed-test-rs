//! Command line and environment configuration (docs/SPEC.md §9).
//!
//! Every option can be given as a flag or an environment variable, with the precedence
//! **flag > env var > default**. Parsing is done by `clap` into [`ServeArgs`] (flags only);
//! [`Config::resolve`] then layers the environment and the defaults on top and validates the
//! result. The environment is passed in as a lookup function so the resolution is testable
//! without touching the process environment.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::str::FromStr;

use clap::{Args, Parser, Subcommand};

/// Default HTTP port.
pub const DEFAULT_PORT: u16 = 8080;
/// Default HTTPS port (only used when a certificate is configured).
pub const DEFAULT_TLS_PORT: u16 = 8443;
/// Default WebRTC UDP port.
pub const DEFAULT_UDP_PORT: u16 = 3478;
/// Default number of concurrent test sessions.
pub const DEFAULT_MAX_SESSIONS: usize = 16;
/// Default per-request byte cap.
pub const DEFAULT_MAX_BYTES: u64 = 2_000_000_000;
/// Default log filter.
pub const DEFAULT_LOG: &str = "info";
/// Directory name used under the platform data directory.
pub const APP_DIR_NAME: &str = "speed-test-rs";
/// SQLite file name inside the data directory.
pub const DB_FILE_NAME: &str = "speedtest.db";

/// Self-hosted LAN speed test. Serves the UI and the measurement endpoints.
#[derive(Debug, Parser)]
#[command(
    name = "speed-test-rs",
    version,
    about,
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Options for the default `serve` command.
    #[command(flatten)]
    pub serve: ServeArgs,
}

impl Cli {
    /// The command to run; `serve` when none was given.
    pub fn into_command(self) -> Command {
        self.command.unwrap_or(Command::Serve(self.serve))
    }
}

// Parsed once at startup, so the size difference between the variants doesn't matter.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the server and serve the UI (the default command).
    Serve(ServeArgs),
    /// Exit 0 if the local server answers /api/health (used by the Docker HEALTHCHECK).
    Healthcheck(HealthcheckArgs),
}

/// Flags of the `serve` command. Every field is optional here; the environment and the
/// defaults are applied by [`Config::resolve`].
#[derive(Debug, Default, Clone, Args)]
pub struct ServeArgs {
    /// HTTP/WebSocket listen address(es); repeat or comma-separate. A bare port listens on
    /// all interfaces. [env: SPEEDTEST_LISTEN, LEPTOS_SITE_ADDR] [default: 0.0.0.0:8080,[::]:8080]
    #[arg(short, long, value_name = "ADDR", value_delimiter = ',')]
    pub listen: Vec<String>,

    /// WebRTC UDP port. [env: SPEEDTEST_UDP_PORT] [default: 3478]
    #[arg(long, value_name = "PORT")]
    pub udp_port: Option<u16>,

    /// IP advertised in ICE candidates (set it under Docker bridge networking).
    /// [env: SPEEDTEST_PUBLIC_IP] [default: auto]
    #[arg(long, value_name = "IP")]
    pub public_ip: Option<IpAddr>,

    /// Name shown in the UI and the history. [env: SPEEDTEST_SERVER_NAME] [default: hostname]
    #[arg(long, value_name = "NAME")]
    pub server_name: Option<String>,

    /// Directory for the database. [env: SPEEDTEST_DATA_DIR] [default: platform data dir]
    #[arg(long, value_name = "DIR")]
    pub data_dir: Option<PathBuf>,

    /// SQLite file (overrides --data-dir). [env: SPEEDTEST_DB_PATH] [default: <data-dir>/speedtest.db]
    #[arg(long, value_name = "FILE")]
    pub db_path: Option<PathBuf>,

    /// Turn run persistence off. [env: SPEEDTEST_HISTORY=false]
    #[arg(long)]
    pub no_history: bool,

    /// Prune runs older than this many days. [env: SPEEDTEST_RETENTION_DAYS]
    #[arg(long, value_name = "N")]
    pub retention_days: Option<u32>,

    /// Maximum concurrent test sessions. [env: SPEEDTEST_MAX_SESSIONS] [default: 16]
    #[arg(long, value_name = "N")]
    pub max_sessions: Option<usize>,

    /// Per-request byte cap. [env: SPEEDTEST_MAX_BYTES] [default: 2000000000]
    #[arg(long, value_name = "N")]
    pub max_bytes: Option<u64>,

    /// Turn off the live devices panel and group tests. [env: SPEEDTEST_PRESENCE=false]
    #[arg(long)]
    pub no_presence: bool,

    /// Server link speed in Mbps, for the bottleneck warning.
    /// [env: SPEEDTEST_SERVER_LINK_MBPS] [default: auto (Linux)]
    #[arg(long, value_name = "N")]
    pub server_link_mbps: Option<u32>,

    /// Open the UI in the default browser after startup.
    #[arg(long)]
    pub open: bool,

    /// Don't print a QR code of the LAN URL at startup.
    #[arg(long)]
    pub no_qr: bool,

    /// Serve UI assets from this directory instead of the embedded copy.
    /// [env: LEPTOS_SITE_ROOT]
    #[arg(long, value_name = "DIR")]
    pub site_root: Option<PathBuf>,

    /// Log filter (tracing-subscriber EnvFilter syntax). [env: RUST_LOG] [default: info]
    #[arg(long, value_name = "FILTER")]
    pub log: Option<String>,

    /// HTTPS/WSS listen address(es); repeat or comma-separate. [env: SPEEDTEST_TLS_LISTEN]
    /// [default: 0.0.0.0:8443,[::]:8443 when a certificate is given]
    #[arg(long, value_name = "ADDR", value_delimiter = ',')]
    pub tls_listen: Vec<String>,

    /// PEM certificate (chain) for the HTTPS listener. [env: SPEEDTEST_TLS_CERT]
    #[arg(long, value_name = "FILE")]
    pub tls_cert: Option<PathBuf>,

    /// PEM private key for the HTTPS listener. [env: SPEEDTEST_TLS_KEY]
    #[arg(long, value_name = "FILE")]
    pub tls_key: Option<PathBuf>,
}

/// Flags of the `healthcheck` command.
#[derive(Debug, Default, Clone, Args)]
pub struct HealthcheckArgs {
    /// Listen address(es) of the server to check (as given to `serve`).
    /// [env: SPEEDTEST_LISTEN, LEPTOS_SITE_ADDR] [default: 0.0.0.0:8080,[::]:8080]
    #[arg(short, long, value_name = "ADDR", value_delimiter = ',')]
    pub listen: Vec<String>,
}

/// A value that must not be printed (e.g. in `Debug` output or logs).
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The secret value. Only call this where the value is actually compared or used.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

/// The optional HTTPS listener.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsConfig {
    pub listen: Vec<SocketAddr>,
    pub cert: PathBuf,
    pub key: PathBuf,
}

/// The validated server configuration, shared through [`crate::server::state::AppState`].
#[derive(Debug, Clone)]
pub struct Config {
    /// HTTP/WebSocket listen addresses (never empty).
    pub listen: Vec<SocketAddr>,
    /// WebRTC UDP port.
    pub udp_port: u16,
    /// IP to advertise in ICE candidates; `None` = auto-detect.
    pub public_ip: Option<IpAddr>,
    /// Shown in the UI and stored with each run.
    pub server_name: String,
    /// Directory for the database (and anything else persisted).
    pub data_dir: PathBuf,
    /// SQLite database file.
    pub db_path: PathBuf,
    /// Whether runs are persisted.
    pub history: bool,
    /// Auto-prune runs older than this many days.
    pub retention_days: Option<u32>,
    /// Maximum concurrent test sessions.
    pub max_sessions: usize,
    /// Per-request byte cap.
    pub max_bytes: u64,
    /// `SPEEDTEST_AUTH_TOKEN` (env only).
    pub auth_token: Option<Secret>,
    /// Whether the live devices panel and group tests are on.
    pub presence: bool,
    /// Server link speed in Mbps; `None` = auto-detect.
    pub server_link_mbps: Option<u32>,
    /// Open the UI in the default browser after startup.
    pub open: bool,
    /// Print a QR code of the first LAN URL at startup.
    pub qr: bool,
    /// Serve UI assets from this directory instead of the embedded copy. Always `Some` when
    /// the binary was built without `embed-assets` (defaults to `target/site`).
    pub site_root: Option<PathBuf>,
    /// tracing-subscriber filter.
    pub log: String,
    /// The HTTPS listener, when a certificate and key are configured.
    pub tls: Option<TlsConfig>,
}

/// A configuration error, reported to the user before the server starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError(String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

fn err(message: impl Into<String>) -> ConfigError {
    ConfigError(message.into())
}

/// Reads environment variables from the process; empty values count as unset.
pub fn process_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

impl Config {
    /// Resolves the configuration from the parsed flags and the process environment.
    pub fn from_args(args: ServeArgs) -> Result<Self, ConfigError> {
        Self::resolve(args, process_env)
    }

    /// Resolves the configuration: flag > env var (looked up through `env`) > default.
    pub fn resolve(
        args: ServeArgs,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let env = |key: &str| env(key).filter(|v| !v.trim().is_empty());

        let listen = resolve_listen(&args.listen, &env)?;
        let tls = resolve_tls(&args, &env)?;

        let udp_port =
            flag_or_env(args.udp_port, &env, "SPEEDTEST_UDP_PORT")?.unwrap_or(DEFAULT_UDP_PORT);
        let public_ip = flag_or_env(args.public_ip, &env, "SPEEDTEST_PUBLIC_IP")?;
        let server_name = args
            .server_name
            .or_else(|| env("SPEEDTEST_SERVER_NAME"))
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(default_server_name);

        let data_dir = args
            .data_dir
            .or_else(|| env("SPEEDTEST_DATA_DIR").map(PathBuf::from))
            .unwrap_or_else(default_data_dir);
        let db_path = args
            .db_path
            .or_else(|| env("SPEEDTEST_DB_PATH").map(PathBuf::from))
            .unwrap_or_else(|| data_dir.join(DB_FILE_NAME));

        let history = !args.no_history && env_bool(&env, "SPEEDTEST_HISTORY")?.unwrap_or(true);
        let presence = !args.no_presence && env_bool(&env, "SPEEDTEST_PRESENCE")?.unwrap_or(true);

        let retention_days = flag_or_env(args.retention_days, &env, "SPEEDTEST_RETENTION_DAYS")?;
        let max_sessions = flag_or_env(args.max_sessions, &env, "SPEEDTEST_MAX_SESSIONS")?
            .unwrap_or(DEFAULT_MAX_SESSIONS);
        if max_sessions == 0 {
            return Err(err("--max-sessions must be at least 1"));
        }
        let max_bytes =
            flag_or_env(args.max_bytes, &env, "SPEEDTEST_MAX_BYTES")?.unwrap_or(DEFAULT_MAX_BYTES);
        let server_link_mbps =
            flag_or_env(args.server_link_mbps, &env, "SPEEDTEST_SERVER_LINK_MBPS")?;
        let auth_token = env("SPEEDTEST_AUTH_TOKEN").map(Secret::new);

        let site_root = args
            .site_root
            .or_else(|| env("LEPTOS_SITE_ROOT").map(PathBuf::from))
            .or_else(default_site_root);

        let log = args
            .log
            .or_else(|| env("RUST_LOG"))
            .unwrap_or_else(|| DEFAULT_LOG.to_owned());

        Ok(Config {
            listen,
            udp_port,
            public_ip,
            server_name,
            data_dir,
            db_path,
            history,
            retention_days,
            max_sessions,
            max_bytes,
            auth_token,
            presence,
            server_link_mbps,
            open: args.open,
            qr: !args.no_qr,
            site_root,
            log,
            tls,
        })
    }

    /// Whether any HTTP listener is bound to an unspecified (all-interfaces) address.
    pub fn listens_on_all_interfaces(&self) -> bool {
        self.listen.iter().any(|addr| addr.ip().is_unspecified())
    }
}

/// Resolves the listen addresses for the `healthcheck` command the same way `serve` does.
pub fn healthcheck_listen(args: &HealthcheckArgs) -> Result<Vec<SocketAddr>, ConfigError> {
    resolve_listen(&args.listen, &process_env)
}

fn resolve_listen(
    flags: &[String],
    env: &impl Fn(&str) -> Option<String>,
) -> Result<Vec<SocketAddr>, ConfigError> {
    if !flags.is_empty() {
        return parse_addr_list(flags.iter().map(String::as_str), "--listen");
    }
    for key in ["SPEEDTEST_LISTEN", "LEPTOS_SITE_ADDR"] {
        if let Some(value) = env(key) {
            return parse_addr_list(split_list(&value), key);
        }
    }
    Ok(dual_stack(DEFAULT_PORT))
}

fn resolve_tls(
    args: &ServeArgs,
    env: &impl Fn(&str) -> Option<String>,
) -> Result<Option<TlsConfig>, ConfigError> {
    let cert = args
        .tls_cert
        .clone()
        .or_else(|| env("SPEEDTEST_TLS_CERT").map(PathBuf::from));
    let key = args
        .tls_key
        .clone()
        .or_else(|| env("SPEEDTEST_TLS_KEY").map(PathBuf::from));
    let listen = if !args.tls_listen.is_empty() {
        Some(parse_addr_list(
            args.tls_listen.iter().map(String::as_str),
            "--tls-listen",
        )?)
    } else if let Some(value) = env("SPEEDTEST_TLS_LISTEN") {
        Some(parse_addr_list(split_list(&value), "SPEEDTEST_TLS_LISTEN")?)
    } else {
        None
    };

    match (cert, key) {
        (Some(cert), Some(key)) => Ok(Some(TlsConfig {
            listen: listen.unwrap_or_else(|| dual_stack(DEFAULT_TLS_PORT)),
            cert,
            key,
        })),
        (None, None) if listen.is_none() => Ok(None),
        (None, None) => Err(err(
            "--tls-listen needs a certificate and key (--tls-cert and --tls-key)",
        )),
        (Some(_), None) => Err(err("--tls-cert is set but --tls-key is missing")),
        (None, Some(_)) => Err(err("--tls-key is set but --tls-cert is missing")),
    }
}

/// `0.0.0.0:<port>` and `[::]:<port>`.
pub fn dual_stack(port: u16) -> Vec<SocketAddr> {
    vec![
        SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)),
        SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)),
    ]
}

fn split_list(value: &str) -> impl Iterator<Item = &str> {
    value
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
}

/// Parses socket addresses. A bare port (`9000`) expands to both unspecified addresses.
fn parse_addr_list<'a>(
    items: impl Iterator<Item = &'a str>,
    source: &str,
) -> Result<Vec<SocketAddr>, ConfigError> {
    let mut addrs = Vec::new();
    for item in items.map(str::trim).filter(|s| !s.is_empty()) {
        let parsed = if let Ok(addr) = item.parse::<SocketAddr>() {
            vec![addr]
        } else if let Ok(port) = item.parse::<u16>() {
            dual_stack(port)
        } else {
            return Err(err(format!(
                "{source}: invalid address `{item}` (expected e.g. 0.0.0.0:8080, [::]:8080, \
                 127.0.0.1:8080 or a bare port)"
            )));
        };
        for addr in parsed {
            if !addrs.contains(&addr) {
                addrs.push(addr);
            }
        }
    }
    if addrs.is_empty() {
        return Err(err(format!("{source}: no address given")));
    }
    Ok(addrs)
}

fn flag_or_env<T>(
    flag: Option<T>,
    env: &impl Fn(&str) -> Option<String>,
    key: &str,
) -> Result<Option<T>, ConfigError>
where
    T: FromStr,
    T::Err: fmt::Display,
{
    if flag.is_some() {
        return Ok(flag);
    }
    env(key)
        .map(|value| {
            value
                .trim()
                .parse()
                .map_err(|e| err(format!("{key}: invalid value `{value}`: {e}")))
        })
        .transpose()
}

fn env_bool(env: &impl Fn(&str) -> Option<String>, key: &str) -> Result<Option<bool>, ConfigError> {
    env(key)
        .map(|value| match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            _ => Err(err(format!(
                "{key}: invalid value `{value}` (expected true or false)"
            ))),
        })
        .transpose()
}

/// The platform data directory (docs/SPEC.md §9.3): `$XDG_DATA_HOME/speed-test-rs` on Linux,
/// `~/Library/Application Support/speed-test-rs` on macOS, `%APPDATA%\speed-test-rs` on
/// Windows. Falls back to `./speed-test-rs-data` when there is no home directory.
pub fn default_data_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|dirs| dirs.data_dir().join(APP_DIR_NAME))
        .unwrap_or_else(|| PathBuf::from("speed-test-rs-data"))
}

fn default_server_name() -> String {
    gethostname::gethostname()
        .into_string()
        .ok()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| APP_DIR_NAME.to_owned())
}

/// Without embedded assets the UI is always served from disk, from the cargo-leptos
/// output by default.
fn default_site_root() -> Option<PathBuf> {
    if cfg!(feature = "embed-assets") {
        None
    } else {
        Some(PathBuf::from("target/site"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    fn parse(args: &[&str]) -> ServeArgs {
        let cli = Cli::try_parse_from(std::iter::once("speed-test-rs").chain(args.iter().copied()))
            .expect("valid args");
        match cli.into_command() {
            Command::Serve(args) => args,
            Command::Healthcheck(_) => panic!("expected serve"),
        }
    }

    fn resolve(args: &[&str], env: &[(&str, &str)]) -> Config {
        Config::resolve(parse(args), env_from(env)).expect("valid config")
    }

    #[test]
    fn defaults() {
        let config = resolve(&[], &[]);
        assert_eq!(
            config.listen,
            vec![
                "0.0.0.0:8080".parse::<SocketAddr>().unwrap(),
                "[::]:8080".parse().unwrap()
            ]
        );
        assert!(config.listens_on_all_interfaces());
        assert_eq!(config.udp_port, DEFAULT_UDP_PORT);
        assert_eq!(config.max_sessions, 16);
        assert_eq!(config.max_bytes, 2_000_000_000);
        assert!(config.history);
        assert!(config.presence);
        assert!(config.qr);
        assert!(!config.open);
        assert!(config.auth_token.is_none());
        assert!(config.tls.is_none());
        assert_eq!(config.log, "info");
        assert!(!config.server_name.is_empty());
        assert!(config.data_dir.ends_with(APP_DIR_NAME));
        assert_eq!(config.db_path, config.data_dir.join(DB_FILE_NAME));
    }

    #[test]
    fn flag_beats_env_beats_default() {
        let env = [
            ("SPEEDTEST_LISTEN", "127.0.0.1:9001"),
            ("SPEEDTEST_UDP_PORT", "4000"),
            ("SPEEDTEST_MAX_SESSIONS", "4"),
            ("SPEEDTEST_SERVER_NAME", "from-env"),
            ("RUST_LOG", "debug"),
        ];
        let config = resolve(&[], &env);
        assert_eq!(config.listen, vec!["127.0.0.1:9001".parse().unwrap()]);
        assert!(!config.listens_on_all_interfaces());
        assert_eq!(config.udp_port, 4000);
        assert_eq!(config.max_sessions, 4);
        assert_eq!(config.server_name, "from-env");
        assert_eq!(config.log, "debug");

        let config = resolve(
            &[
                "--listen",
                "127.0.0.1:9002",
                "--udp-port",
                "5000",
                "--max-sessions",
                "2",
                "--server-name",
                "from-flag",
                "--log",
                "warn",
            ],
            &env,
        );
        assert_eq!(config.listen, vec!["127.0.0.1:9002".parse().unwrap()]);
        assert_eq!(config.udp_port, 5000);
        assert_eq!(config.max_sessions, 2);
        assert_eq!(config.server_name, "from-flag");
        assert_eq!(config.log, "warn");
    }

    #[test]
    fn serve_subcommand_is_equivalent() {
        let config = resolve(&["serve", "--listen", "127.0.0.1:9003"], &[]);
        assert_eq!(config.listen, vec!["127.0.0.1:9003".parse().unwrap()]);
    }

    #[test]
    fn listen_lists() {
        let config = resolve(&["-l", "127.0.0.1:1", "-l", "[::1]:2,127.0.0.1:1"], &[]);
        assert_eq!(
            config.listen,
            vec!["127.0.0.1:1".parse().unwrap(), "[::1]:2".parse().unwrap()]
        );

        let config = resolve(&["--listen", "9000"], &[]);
        assert_eq!(config.listen, dual_stack(9000));

        let config = resolve(&[], &[("SPEEDTEST_LISTEN", "127.0.0.1:1, [::1]:2")]);
        assert_eq!(config.listen.len(), 2);

        // LEPTOS_SITE_ADDR (set by `cargo leptos watch` and the Docker image) is an alias,
        // used only when SPEEDTEST_LISTEN is unset.
        let config = resolve(&[], &[("LEPTOS_SITE_ADDR", "127.0.0.1:3000")]);
        assert_eq!(config.listen, vec!["127.0.0.1:3000".parse().unwrap()]);
        let config = resolve(
            &[],
            &[
                ("LEPTOS_SITE_ADDR", "127.0.0.1:3000"),
                ("SPEEDTEST_LISTEN", "127.0.0.1:4000"),
            ],
        );
        assert_eq!(config.listen, vec!["127.0.0.1:4000".parse().unwrap()]);

        assert!(Config::resolve(parse(&["--listen", "nope"]), env_from(&[])).is_err());
    }

    #[test]
    fn data_dir_and_db_path() {
        let config = resolve(&[], &[("SPEEDTEST_DATA_DIR", "/data")]);
        assert_eq!(config.data_dir, PathBuf::from("/data"));
        assert_eq!(config.db_path, PathBuf::from("/data/speedtest.db"));

        let config = resolve(&["--data-dir", "/flag"], &[("SPEEDTEST_DATA_DIR", "/data")]);
        assert_eq!(config.data_dir, PathBuf::from("/flag"));

        let config = resolve(
            &["--data-dir", "/flag"],
            &[("SPEEDTEST_DB_PATH", "/elsewhere/x.db")],
        );
        assert_eq!(config.db_path, PathBuf::from("/elsewhere/x.db"));

        // Empty env values count as unset.
        let config = resolve(&[], &[("SPEEDTEST_DATA_DIR", "")]);
        assert_eq!(config.data_dir, default_data_dir());
    }

    #[test]
    fn inverted_switches() {
        let config = resolve(&[], &[("SPEEDTEST_HISTORY", "false")]);
        assert!(!config.history);
        let config = resolve(&["--no-history"], &[("SPEEDTEST_HISTORY", "true")]);
        assert!(!config.history);
        let config = resolve(&[], &[("SPEEDTEST_PRESENCE", "0")]);
        assert!(!config.presence);
        let config = resolve(&["--no-presence", "--no-qr", "--open"], &[]);
        assert!(!config.presence);
        assert!(!config.qr);
        assert!(config.open);
        assert!(Config::resolve(parse(&[]), env_from(&[("SPEEDTEST_HISTORY", "maybe")])).is_err());
    }

    #[test]
    fn numbers_and_secrets() {
        let config = resolve(
            &["--retention-days", "30"],
            &[
                ("SPEEDTEST_MAX_BYTES", "1000"),
                ("SPEEDTEST_SERVER_LINK_MBPS", "2500"),
                ("SPEEDTEST_PUBLIC_IP", "192.168.1.10"),
                ("SPEEDTEST_AUTH_TOKEN", "hunter2"),
            ],
        );
        assert_eq!(config.retention_days, Some(30));
        assert_eq!(config.max_bytes, 1000);
        assert_eq!(config.server_link_mbps, Some(2500));
        assert_eq!(config.public_ip, Some("192.168.1.10".parse().unwrap()));
        assert_eq!(config.auth_token.as_ref().unwrap().expose(), "hunter2");
        assert!(!format!("{config:?}").contains("hunter2"));

        let bad = Config::resolve(parse(&[]), env_from(&[("SPEEDTEST_UDP_PORT", "70000")]));
        assert!(bad.unwrap_err().to_string().contains("SPEEDTEST_UDP_PORT"));
        assert!(Config::resolve(parse(&["--max-sessions", "0"]), env_from(&[])).is_err());
    }

    #[test]
    fn site_root() {
        let config = resolve(&[], &[("LEPTOS_SITE_ROOT", "site")]);
        assert_eq!(config.site_root, Some(PathBuf::from("site")));
        let config = resolve(
            &["--site-root", "/srv/site"],
            &[("LEPTOS_SITE_ROOT", "site")],
        );
        assert_eq!(config.site_root, Some(PathBuf::from("/srv/site")));
        let config = resolve(&[], &[]);
        assert_eq!(config.site_root.is_none(), cfg!(feature = "embed-assets"));
    }

    #[test]
    fn tls() {
        let config = resolve(&["--tls-cert", "c.pem", "--tls-key", "k.pem"], &[]);
        let tls = config.tls.unwrap();
        assert_eq!(tls.listen, dual_stack(DEFAULT_TLS_PORT));
        assert_eq!(tls.cert, PathBuf::from("c.pem"));

        let config = resolve(
            &["--tls-listen", "127.0.0.1:9443"],
            &[
                ("SPEEDTEST_TLS_CERT", "c.pem"),
                ("SPEEDTEST_TLS_KEY", "k.pem"),
            ],
        );
        assert_eq!(
            config.tls.unwrap().listen,
            vec!["127.0.0.1:9443".parse().unwrap()]
        );

        assert!(Config::resolve(parse(&["--tls-cert", "c.pem"]), env_from(&[])).is_err());
        assert!(
            Config::resolve(parse(&["--tls-listen", "127.0.0.1:9443"]), env_from(&[])).is_err()
        );
    }

    #[test]
    fn healthcheck_subcommand() {
        let cli =
            Cli::try_parse_from(["speed-test-rs", "healthcheck", "--listen", "[::1]:1"]).unwrap();
        match cli.into_command() {
            Command::Healthcheck(args) => assert_eq!(args.listen, vec!["[::1]:1"]),
            Command::Serve(_) => panic!("expected healthcheck"),
        }
    }
}
