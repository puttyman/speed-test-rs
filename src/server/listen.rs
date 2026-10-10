//! TCP listeners (plain HTTP and optional TLS) and graceful shutdown.

use std::future::Future;
use std::io;
use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::serve::ListenerExt;
use axum_server::tls_rustls::RustlsConfig;
use socket2::{Domain, Protocol, Socket, Type};
use tokio::sync::watch;
use tokio::task::JoinSet;

/// How long in-flight requests get to finish after a shutdown signal.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// Binds one TCP listener. IPv6 sockets get `IPV6_V6ONLY` so `0.0.0.0:<port>` and
/// `[::]:<port>` can both be bound on every platform.
pub fn bind(addr: SocketAddr) -> io::Result<TcpListener> {
    let socket = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
    if addr.is_ipv6() {
        socket.set_only_v6(true)?;
    }
    #[cfg(not(windows))]
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    socket.listen(1024)?;
    Ok(socket.into())
}

/// Binds every address. An IPv6 address that fails for a reason other than "in use" or
/// "permission denied" (typically: IPv6 is disabled on this host) is skipped with a warning,
/// as long as at least one other address could be bound.
pub fn bind_all(addrs: &[SocketAddr]) -> io::Result<Vec<TcpListener>> {
    let mut listeners = Vec::new();
    let mut skipped = Vec::new();
    for &addr in addrs {
        match bind(addr) {
            Ok(listener) => listeners.push(listener),
            Err(e)
                if addr.is_ipv6()
                    && !matches!(
                        e.kind(),
                        io::ErrorKind::AddrInUse | io::ErrorKind::PermissionDenied
                    ) =>
            {
                skipped.push((addr, e));
            }
            Err(e) => {
                return Err(io::Error::new(
                    e.kind(),
                    format!("cannot listen on {addr}: {e}"),
                ));
            }
        }
    }
    if listeners.is_empty() {
        let (addr, e) = skipped
            .into_iter()
            .next()
            .expect("bind_all called with no addresses");
        return Err(io::Error::new(
            e.kind(),
            format!("cannot listen on {addr}: {e}"),
        ));
    }
    for (addr, e) in skipped {
        tracing::warn!("IPv6 unavailable, not listening on {addr}: {e}; continuing on IPv4 only");
    }
    Ok(listeners)
}

/// Loads a PEM certificate chain and private key into a rustls server configuration
/// (ring crypto provider, HTTP/1.1 ALPN).
pub fn load_tls(cert: &Path, key: &Path) -> Result<RustlsConfig, String> {
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};

    let certs = CertificateDer::pem_file_iter(cert)
        .and_then(|iter| iter.collect::<Result<Vec<_>, _>>())
        .map_err(|e| format!("cannot read TLS certificate {}: {e}", cert.display()))?;
    if certs.is_empty() {
        return Err(format!("no certificate found in {}", cert.display()));
    }
    let key = PrivateKeyDer::from_pem_file(key)
        .map_err(|e| format!("cannot read TLS key {}: {e}", key.display()))?;

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS setup failed: {e}"))?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("invalid TLS certificate or key: {e}"))?;
    // HTTP/1.1 only: WebSockets need it, and one TCP connection per measurement stream
    // keeps throughput comparable with the plain HTTP listener.
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(RustlsConfig::from_config(Arc::new(config)))
}

/// Serves `app` on every listener until `shutdown` resolves, then shuts all of them down
/// gracefully. Handlers can extract `ConnectInfo<SocketAddr>` for the client address.
pub async fn serve(
    app: Router,
    http: Vec<TcpListener>,
    tls: Option<(Vec<TcpListener>, RustlsConfig)>,
    shutdown: impl Future<Output = ()>,
) -> io::Result<()> {
    let (stop_tx, stop_rx) = watch::channel(false);
    let mut tasks = JoinSet::new();

    for listener in http {
        // TCP_NODELAY so latency probes (ping/pong) aren't held back by Nagle's algorithm.
        let listener = tokio::net::TcpListener::from_std(listener)?.tap_io(|stream| {
            let _ = stream.set_nodelay(true);
        });
        let app = app.clone();
        let mut stop = stop_rx.clone();
        tasks.spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async move {
                let _ = stop.wait_for(|stop| *stop).await;
            })
            .await
        });
    }

    let handle = axum_server::Handle::new();
    if let Some((listeners, config)) = tls {
        for listener in listeners {
            let acceptor = axum_server::tls_rustls::RustlsAcceptor::new(config.clone())
                .acceptor(axum_server::accept::NoDelayAcceptor::new());
            let server = axum_server::from_tcp(listener)?
                .acceptor(acceptor)
                .handle(handle.clone());
            let app = app.clone();
            tasks.spawn(async move {
                server
                    .serve(app.into_make_service_with_connect_info::<SocketAddr>())
                    .await
            });
        }
    }

    // Run until a shutdown signal, or until a server stops on its own (an error).
    let early = tokio::select! {
        () = shutdown => None,
        res = tasks.join_next() => res,
    };
    let _ = stop_tx.send(true);
    handle.graceful_shutdown(Some(SHUTDOWN_GRACE));

    let mut result = early.map(flatten).unwrap_or(Ok(()));
    let drain = async {
        while let Some(res) = tasks.join_next().await {
            if let Err(e) = flatten(res) {
                result = Err(e);
            }
        }
    };
    if tokio::time::timeout(SHUTDOWN_GRACE + Duration::from_secs(1), drain)
        .await
        .is_err()
    {
        tracing::warn!("some connections did not close in time");
    }
    result
}

fn flatten(res: Result<io::Result<()>, tokio::task::JoinError>) -> io::Result<()> {
    res.map_err(io::Error::other)?
}

/// Resolves on Ctrl+C or SIGTERM so `docker stop` shuts the server down cleanly.
pub async fn shutdown_signal() {
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
        () = ctrl_c => {},
        () = terminate => {},
    }
    tracing::info!("shutting down");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dual_stack_on_the_same_port() {
        // Find a free port, then bind both stacks on it.
        let port = bind("127.0.0.1:0".parse().unwrap())
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let addrs = crate::server::config::dual_stack(port);
        let listeners = bind_all(&addrs).unwrap();
        assert!(!listeners.is_empty());
        assert_eq!(listeners[0].local_addr().unwrap().port(), port);
        if listeners.len() == 2 {
            assert!(listeners[1].local_addr().unwrap().is_ipv6());
        }
    }

    #[test]
    fn port_in_use_is_an_error() {
        let first = bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let addr = first.local_addr().unwrap();
        assert!(bind_all(&[addr]).is_err());
    }
}
