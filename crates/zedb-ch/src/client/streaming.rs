use super::*;

impl ChClient {
    /// Run a query and report decoded columns and rows as soon as complete
    /// values arrive from ClickHouse. Aborting the caller's task cancels the
    /// underlying HTTP request. `params` are ClickHouse query parameters
    /// (for `{name:Type}` placeholders), sent as `param_<name>`; each
    /// request is its own session, so they must ride along every time.
    pub async fn query_stream(
        &self,
        sql: &str,
        params: &[(String, String)],
        row_limit: usize,
        mut on_event: impl FnMut(QueryStreamEvent),
    ) -> Result<QueryStreamSummary> {
        self.ensure_secure_endpoint()?;
        let mut request = self
            .http
            .post(&self.cfg.url)
            .header("X-ClickHouse-User", &self.cfg.user)
            .body(sql.to_string());
        if let Some(password) = &self.cfg.password {
            request = request.header("X-ClickHouse-Key", password);
        }
        if let Some(database) = &self.cfg.database {
            request = request.query(&[("database", database.as_str())]);
        }
        for (name, value) in params {
            request = request.query(&[(format!("param_{name}"), value.as_str())]);
        }
        if self.cfg.read_only {
            request = request.query(&[("readonly", "2")]);
        }
        for (name, value) in self.driver_params(true) {
            request = request.query(&[(name.as_str(), value.as_str())]);
        }
        let query_id = next_query_id();
        request = request.query(&[("query_id", query_id.as_str())]);
        request = request.query(&[("default_format", "RowBinaryWithNamesAndTypes")]);
        on_event(QueryStreamEvent::Started {
            query_id: query_id.clone(),
        });

        let mut progress_interval = tokio::time::interval(Duration::from_millis(100));
        progress_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut response_future = Box::pin(request.send());
        let response = loop {
            tokio::select! {
                response = &mut response_future => break response?,
                _ = progress_interval.tick() => {
                    if let Some(progress) = self.query_progress(&query_id).await {
                        on_event(QueryStreamEvent::Progress(progress));
                    }
                }
            }
        };
        let status = response.status();
        let code = response
            .headers()
            .get("X-ClickHouse-Exception-Code")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok());
        if !status.is_success() {
            let bytes = response.bytes().await?;
            return Err(ChError::Server {
                code,
                message: String::from_utf8_lossy(&bytes).trim().to_string(),
            });
        }

        let mut decoder = rowbinary::StreamingDecoder::new();
        let mut sent_columns = false;
        let mut sent_rows = 0;
        let mut pending_rows = Vec::new();
        let mut received_bytes = 0;
        let mut body = response.bytes_stream();
        loop {
            tokio::select! {
                chunk = body.next() => {
                    let Some(chunk) = chunk else {
                        break;
                    };
                    let chunk = chunk?;
                    received_bytes += chunk.len() as u64;
                    let mut rows = decoder.push(&chunk)?;
                    if !sent_columns {
                        if let Some(columns) = decoder.columns() {
                            on_event(QueryStreamEvent::Columns(columns.to_vec()));
                            sent_columns = true;
                        }
                    }
                    if !rows.is_empty() {
                        let remaining = row_limit.saturating_sub(sent_rows + pending_rows.len());
                        let capped = rows.len() > remaining;
                        if capped {
                            rows.truncate(remaining);
                        }
                        pending_rows.extend(rows);
                        if pending_rows.len() >= 512 || capped {
                            sent_rows += pending_rows.len();
                            on_event(QueryStreamEvent::Rows(std::mem::take(&mut pending_rows)));
                        }
                        if capped {
                            on_event(QueryStreamEvent::Progress(QueryProgress {
                                received_bytes,
                                ..QueryProgress::default()
                            }));
                            return Ok(QueryStreamSummary {
                                rows: sent_rows,
                                capped: true,
                            });
                        }
                    }
                }
                _ = progress_interval.tick() => {
                    let mut progress = self.query_progress(&query_id).await.unwrap_or_default();
                    progress.received_bytes = received_bytes;
                    on_event(QueryStreamEvent::Progress(progress));
                }
            }
        }
        if !pending_rows.is_empty() {
            sent_rows += pending_rows.len();
            on_event(QueryStreamEvent::Rows(pending_rows));
        }
        on_event(QueryStreamEvent::Progress(QueryProgress {
            received_bytes,
            ..QueryProgress::default()
        }));
        decoder.finish()?;
        Ok(QueryStreamSummary {
            rows: sent_rows,
            capped: false,
        })
    }

    async fn query_progress(&self, query_id: &str) -> Option<QueryProgress> {
        let sql = format!(
            "SELECT read_rows, read_bytes, total_rows_approx FROM system.processes \
             WHERE query_id = '{query_id}' LIMIT 1"
        );
        let result = self.query(&sql).await.ok()?;
        let row = result.rows.first()?;
        Some(QueryProgress {
            read_rows: value_as_u64(row.first()?),
            read_bytes: value_as_u64(row.get(1)?),
            total_rows: value_as_u64(row.get(2)?).filter(|total| *total > 0),
            received_bytes: 0,
        })
    }
}
