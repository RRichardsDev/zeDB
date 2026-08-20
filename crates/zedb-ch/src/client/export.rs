use super::*;

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

        self.ensure_secure_endpoint()?;
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

        let resp = req.send().await?;
        let status = resp.status();
        let code = resp
            .headers()
            .get("X-ClickHouse-Exception-Code")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok());
        if !status.is_success() {
            let bytes = resp.bytes().await?;
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
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk)
                .await
                .map_err(|error| ChError::Decode(format!("write failed: {error}")))?;
            written += chunk.len() as u64;
            on_progress(written);
        }
        file.flush()
            .await
            .map_err(|error| ChError::Decode(format!("flush failed: {error}")))?;
        Ok(written)
    }
}
