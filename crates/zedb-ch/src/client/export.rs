use super::*;

// Exports legitimately run far longer than ordinary queries, so the
// shared client's whole-request deadline is overridden with a generous
// ceiling; a stalled peer is caught by the idle deadline instead.
const EXPORT_TOTAL_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
#[cfg(not(test))]
const EXPORT_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
#[cfg(test)]
const EXPORT_IDLE_TIMEOUT: Duration = Duration::from_millis(250);

impl ChClient {
    /// Stream a query's server-formatted output straight to a file,
    /// bypassing RowBinary decode and the grid: the export path runs
    /// at wire speed. Returns bytes written; `on_progress` fires per
    /// received chunk with the running total.
    pub async fn download_to_file(
        &self,
        sql: &str,
        format: &str,
        path: &std::path::Path,
        mut on_progress: impl FnMut(u64),
    ) -> Result<u64> {
        use futures_util::StreamExt as _;
        use tokio::io::AsyncWriteExt as _;

        self.ensure_acceptable_endpoint()?;
        let mut req = self
            .http
            .post(&self.cfg.url)
            .header("X-ClickHouse-User", &self.cfg.user)
            .body(sql.to_string());
        if let Some(password) = &self.cfg.password {
            req = req.header("X-ClickHouse-Key", password);
        }
        if let Some(db) = &self.cfg.database {
            req = req.query(&[("database", db.as_str())]);
        }
        if self.cfg.read_only {
            req = req.query(&[("readonly", "2")]);
        }
        for (name, value) in self.driver_params(true) {
            req = req.query(&[(name.as_str(), value.as_str())]);
        }
        req = req.query(&[("default_format", format)]);
        req = req.timeout(EXPORT_TOTAL_TIMEOUT);

        let resp = req.send().await?;
        let status = resp.status();
        let code = resp
            .headers()
            .get("X-ClickHouse-Exception-Code")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok());
        if !status.is_success() {
            let bytes = collect_response_bounded(resp, MAX_ERROR_RESPONSE_BYTES).await?;
            return Err(ChError::Server {
                code,
                message: String::from_utf8_lossy(&bytes).trim().to_string(),
            });
        }
        let mut file = tokio::fs::File::create(path)
            .await
            .map_err(|error| ChError::Decode(format!("could not create {path:?}: {error}")))?;
        let mut written: u64 = 0;
        let mut stream = resp.bytes_stream();
        loop {
            let chunk = tokio::time::timeout(EXPORT_IDLE_TIMEOUT, stream.next())
                .await
                .map_err(|_| {
                    ChError::Decode("export stalled while waiting for response data".into())
                })?;
            let Some(chunk) = chunk else {
                break;
            };
            let chunk = chunk?;
            file.write_all(&chunk)
                .await
                .map_err(|error| ChError::Decode(format!("write failed: {error}")))?;
            written = written.checked_add(chunk.len() as u64).ok_or_else(|| {
                ChError::Decode("export byte count exceeded the supported range".into())
            })?;
            on_progress(written);
        }
        file.flush()
            .await
            .map_err(|error| ChError::Decode(format!("flush failed: {error}")))?;
        Ok(written)
    }
}

#[cfg(test)]
mod security_tests {
    use super::*;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    #[tokio::test]
    async fn stalled_export_peer_hits_the_idle_deadline() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 8192];
            let _ = stream.read(&mut request).await;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 16\r\n\r\npartial ")
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_secs(2)).await;
        });

        let client = ChClient::new(ChConfig {
            url: format!("http://{address}"),
            user: "security-review".into(),
            password: Some("test-secret".into()),
            database: None,
            read_only: false,
            driver: DriverConfig::default(),
            native_port: None,
        });
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("export.csv");
        let started = std::time::Instant::now();
        let result = client
            .download_to_file("SELECT 1", "CSV", &target, |_| {})
            .await;
        assert!(
            matches!(result, Err(ChError::Decode(ref message)) if message.contains("stalled")),
            "{result:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        server.abort();
    }
}
