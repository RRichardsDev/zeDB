//! Shared scaffolding for tests that need a real ClickHouse: locating a
//! trust-verified binary in the pin cache and querying an
//! [`crate::ephemeral::EphemeralServer`] over HTTP. Compiled only under
//! `cfg(test)` or the `test-support` feature; never part of a normal
//! build.

use std::path::PathBuf;

/// Any trust-verified version from the pin cache; which version does
/// not matter for tests. Binaries that no longer match the checked-in
/// trust manifest are ignored, exactly as the fleet harness would
/// ignore them.
pub fn any_cached_binary() -> Option<PathBuf> {
    let root = crate::binary_cache_dir();
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let version = entry.file_name().to_string_lossy().to_string();
        if let Some(binary) = crate::cached_binary(&version) {
            return Some(binary);
        }
    }
    None
}

/// `any_cached_binary`, with an explicit opt-in repair path: when
/// `ZEDB_E2E_DOWNLOAD=1` is set, an empty (or stale, no longer
/// manifest-matching) cache is refilled through the same verified
/// `ensure_binary` download the fleet harness uses, at the newest
/// trusted version for this platform. Without the variable, tests
/// never download anything.
pub fn e2e_binary() -> Option<PathBuf> {
    if let Some(binary) = any_cached_binary() {
        return Some(binary);
    }
    if std::env::var_os("ZEDB_E2E_DOWNLOAD").is_none() {
        return None;
    }
    let version = crate::pin::newest_trusted_version()?;
    tokio::runtime::Runtime::new()
        .ok()?
        .block_on(crate::ensure_binary(&version))
        .ok()
}

/// One SQL statement against an ephemeral server over blocking HTTP,
/// returning the response body; panics on transport failure or a
/// non-200 status so test assertions read cleanly.
pub fn http_query(server: &crate::ephemeral::EphemeralServer, sql: &str) -> String {
    use std::io::{Read, Write};
    let address = server.http_url.trim_start_matches("http://").to_string();
    let mut stream = std::net::TcpStream::connect(&address).expect("connect to ephemeral server");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .expect("set read timeout");
    write!(
        stream,
        "POST / HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{sql}",
        sql.len()
    )
    .expect("send query");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read query response");
    let (head, body) = response
        .split_once("\r\n\r\n")
        .expect("http response has headers");
    assert!(
        head.starts_with("HTTP/1.1 200"),
        "query failed: {sql}\n{head}\n{body}"
    );
    // Connection: close makes the body everything after the headers;
    // strip HTTP/1.1 chunked framing when the server uses it.
    if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        let mut decoded = String::new();
        let mut rest = body;
        loop {
            let Some((size, tail)) = rest.split_once("\r\n") else {
                break;
            };
            let Ok(size) = usize::from_str_radix(size.trim(), 16) else {
                break;
            };
            if size == 0 {
                break;
            }
            decoded.push_str(&tail[..size.min(tail.len())]);
            rest = tail.get(size + 2..).unwrap_or_default();
        }
        decoded
    } else {
        body.to_string()
    }
}
