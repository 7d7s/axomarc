// The health-check use case.
//
// Used by the deploy flow to decide "is the new container serving
// traffic yet?" and by the doctor to probe a running app.
//
// Two probe kinds:
//   - TCP:  open a socket; success = port reachable
//   - HTTP: GET the path; success = 2xx
//
// The use case is intentionally a free function (not on AppState) so
// unit tests can call it without spinning up storage or a runtime.

use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, ToSocketAddrs};

use crate::ports::HealthResult;

/// Default per-attempt timeout for the TCP probe.
pub const TCP_TIMEOUT: Duration = Duration::from_secs(1);

/// Default per-attempt timeout for the HTTP probe.
pub const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

/// Number of retries before reporting failure. The total wall-clock
/// budget is `timeout * retries` at worst.
pub const RETRIES: u32 = 3;

/// Probe a TCP port. `host:port` is resolved via the OS resolver.
/// Returns `HealthResult` with `ok=true` iff at least one connect
/// succeeded within the retry budget.
pub async fn probe_tcp<A: ToSocketAddrs + Clone>(
    addr: A,
    per_attempt: Duration,
) -> HealthResult {
    let start = Instant::now();
    let mut last_err = String::from("unreachable");
    for attempt in 0..RETRIES {
        match tokio::time::timeout(per_attempt, TcpStream::connect(addr.clone())).await {
            Ok(Ok(_stream)) => {
                return HealthResult {
                    ok: true,
                    latency_ms: start.elapsed().as_millis() as u64,
                    error: None,
                };
            }
            Ok(Err(e)) => last_err = format!("connect: {e}"),
            Err(_) => last_err = format!("timeout after {per_attempt:?}"),
        }
        // Exponential backoff: 0, 100ms, 400ms.
        if attempt + 1 < RETRIES {
            let backoff = Duration::from_millis(100 * 4u64.pow(attempt));
            tokio::time::sleep(backoff).await;
        }
    }
    HealthResult {
        ok: false,
        latency_ms: start.elapsed().as_millis() as u64,
        error: Some(last_err),
    }
}

/// Probe an HTTP endpoint. `host` is the bare hostname (no scheme).
/// Issues a `GET path`; success = 2xx.
pub async fn probe_http(
    host: &str,
    port: u16,
    path: &str,
    per_attempt: Duration,
) -> HealthResult {
    let start = Instant::now();
    let mut last_err = String::from("no response");
    for attempt in 0..RETRIES {
        match tokio::time::timeout(per_attempt, http_get(host, port, path)).await {
            Ok(Ok(status)) if (200..300).contains(&status) => {
                return HealthResult {
                    ok: true,
                    latency_ms: start.elapsed().as_millis() as u64,
                    error: None,
                };
            }
            Ok(Ok(status)) => last_err = format!("http {status}"),
            Ok(Err(e)) => last_err = format!("http: {e}"),
            Err(_) => last_err = format!("timeout after {per_attempt:?}"),
        }
        if attempt + 1 < RETRIES {
            let backoff = Duration::from_millis(100 * 4u64.pow(attempt));
            tokio::time::sleep(backoff).await;
        }
    }
    HealthResult {
        ok: false,
        latency_ms: start.elapsed().as_millis() as u64,
        error: Some(last_err),
    }
}

/// Minimal hand-rolled HTTP/1.1 GET — avoids pulling `reqwest` into
/// sovereign-core. The doctor and the deploy probe only need status;
/// no body is read.
async fn http_get(host: &str, port: u16, path: &str) -> std::io::Result<u16> {
    let addr = (host, port);
    let mut stream = TcpStream::connect(addr).await?;
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nUser-Agent: sovereign/0.1\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await?;
    let mut buf = Vec::with_capacity(512);
    let mut tmp = [0u8; 512];
    loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.len() > 8192 {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let status_line = text.lines().next().unwrap_or("");
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    /// Start a one-shot listener that accepts one connection and
    /// immediately closes. Returns the bound address.
    async fn listening_then_close() -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut s, _)) = listener.accept().await {
                let _ = s.shutdown().await;
            }
        });
        addr
    }

    #[tokio::test]
    async fn tcp_probe_succeeds_when_port_open() {
        let addr = listening_then_close().await;
        // Use a small timeout so the test is fast even when retries
        // are exhausted.
        let r = probe_tcp(addr, Duration::from_millis(200)).await;
        assert!(r.ok, "expected ok, got error {:?}", r.error);
        assert!(r.latency_ms < 1000);
    }

    #[tokio::test]
    async fn tcp_probe_fails_when_port_closed() {
        // Bind + immediately drop the listener to get a port nothing
        // is listening on.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let r = probe_tcp(addr, Duration::from_millis(100)).await;
        assert!(!r.ok);
        assert!(r.error.is_some());
    }

    #[tokio::test]
    async fn http_probe_succeeds_on_2xx() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut s, _)) = listener.accept().await {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf).await;
                let _ = s
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await;
                let _ = s.shutdown().await;
            }
        });
        let r = probe_http(&addr.ip().to_string(), addr.port(), "/", Duration::from_millis(500)).await;
        assert!(r.ok, "expected ok, got error {:?}", r.error);
    }
}
