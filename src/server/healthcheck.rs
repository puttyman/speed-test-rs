use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// Performs `GET /api/health` against the local server and returns a process exit
/// code (0 = healthy). The port is taken from `LEPTOS_SITE_ADDR`.
pub fn run() -> i32 {
    let port = std::env::var("LEPTOS_SITE_ADDR")
        .ok()
        .and_then(|addr| addr.parse::<SocketAddr>().ok())
        .map(|addr| addr.port())
        .unwrap_or(3000);

    match check(SocketAddr::from(([127, 0, 0, 1], port))) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("healthcheck failed: {err}");
            1
        }
    }
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
