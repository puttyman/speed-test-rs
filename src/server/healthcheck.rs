use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::time::Duration;

/// Performs `GET /api/health` against the local server and returns a process exit code
/// (0 = healthy). `listen` is the resolved listen configuration (same flags and env vars as
/// `serve`); unspecified addresses are checked through loopback. The first address that
/// answers wins.
pub fn run(listen: &[SocketAddr]) -> i32 {
    let mut last_err = None;
    for addr in targets(listen) {
        match check(addr) {
            Ok(()) => return 0,
            Err(err) => last_err = Some(format!("{addr}: {err}")),
        }
    }
    eprintln!(
        "healthcheck failed: {}",
        last_err.unwrap_or_else(|| "no listen address".into())
    );
    1
}

/// Where to connect for each listen address: `0.0.0.0` → `127.0.0.1`, `[::]` → `[::1]`.
fn targets(listen: &[SocketAddr]) -> Vec<SocketAddr> {
    let mut targets = Vec::new();
    for addr in listen {
        let ip = match addr.ip() {
            IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
            ip => ip,
        };
        let target = SocketAddr::new(ip, addr.port());
        if !targets.contains(&target) {
            targets.push(target);
        }
    }
    targets
}

fn check(addr: SocketAddr) -> std::io::Result<()> {
    let timeout = Duration::from_secs(2);
    let mut stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.write_all(b"GET /api/health HTTP/1.0\r\nHost: localhost\r\n\r\n")?;

    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    let status_line = response.lines().next().unwrap_or_default();
    if status_line.split_whitespace().nth(1) == Some("200") {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "unexpected response: {status_line}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unspecified_addresses_map_to_loopback() {
        let listen = crate::server::config::dual_stack(8080);
        assert_eq!(
            targets(&listen),
            vec![
                "127.0.0.1:8080".parse::<SocketAddr>().unwrap(),
                "[::1]:8080".parse().unwrap()
            ]
        );
        let listen = vec!["192.168.1.2:9000".parse().unwrap()];
        assert_eq!(targets(&listen), listen);
    }
}
