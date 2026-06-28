//! Shared helpers for the always-on federation-wire / failure-injection integration tests
//! (audit M22 / M29). Compiled into each test binary that does `mod common;`, so unused
//! helpers in a given binary are `#[allow(dead_code)]`.

#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use worksgood::identity::node::{self, NodeLimits};

/// Bring up a real in-process node on an ephemeral localhost port; return its base URL and
/// the temp store dir (kept alive by the returned guard). The accept loop runs on a
/// detached thread that dies with the test process.
pub fn spawn_node() -> (String, tempfile::TempDir) {
    spawn_node_with_limits(NodeLimits::default())
}

/// Like [`spawn_node`] but with explicit limits (e.g. a low connection bound for a flood
/// test).
pub fn spawn_node_with_limits(limits: NodeLimits) -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = dir.path().join("store");
    let (listener, bound) = node::bind("127.0.0.1:0").expect("bind ephemeral port");
    let store_s = store.to_string_lossy().to_string();
    std::thread::spawn(move || {
        let _ = node::serve_on_with_limits(listener, &store_s, limits);
    });
    let base = format!("http://{bound}");
    wait_until_ready(&bound);
    (base, dir)
}

/// The `host:port` of a `http://host:port` base URL.
pub fn addr_of(base: &str) -> &str {
    base.trim_start_matches("http://")
}

/// Poll the node's `/health` until it answers (the serve thread races the first request).
pub fn wait_until_ready(addr: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if http_get(addr, "/wgfed/v1/health").is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("node never became ready at {addr}");
}

/// A dependency-free HTTP/1.1 GET over a raw socket (`reqwest` is not a dev-dependency).
/// Returns `(status_line, body)`.
pub fn http_get(addr: &str, path: &str) -> std::io::Result<(String, String)> {
    let mut stream = TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    )?;
    let mut raw = String::new();
    stream.read_to_string(&mut raw)?;
    Ok(split_response(&raw))
}

/// A dependency-free HTTP/1.1 PUT with a raw byte body. Returns `(status_line, body)`.
pub fn http_put(addr: &str, path: &str, body: &[u8]) -> std::io::Result<(String, String)> {
    let mut stream = TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    write!(
        stream,
        "PUT {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw).to_string();
    Ok(split_response(&text))
}

fn split_response(raw: &str) -> (String, String) {
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw, ""));
    let status = head.lines().next().unwrap_or_default().to_string();
    (status, body.to_string())
}

/// Read a single Prometheus line's integer value from a scrape body. `key` may include a
/// label set, e.g. `wg_node_responses_total{class="2xx"}`.
pub fn metric_value(body: &str, key: &str) -> u64 {
    for line in body.lines() {
        if line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix(key) {
            if let Some(v) = rest.trim().split_whitespace().next() {
                if let Ok(n) = v.parse::<u64>() {
                    return n;
                }
            }
        }
    }
    0
}
