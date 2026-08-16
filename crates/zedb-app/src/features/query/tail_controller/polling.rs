use crate::*;

use gpui::prelude::*;

impl Workspace {
    /// The timer loop: every cadence, while the tab still hosts this tail
    /// generation on the same connection and isn't paused, run one poll.
    /// The cadence follows the delivery mode ([`TailPush`]): fast over a
    /// live native connection, baseline otherwise; while a direct stream
    /// drives the tail, the timer only idles as its watchdog.
    pub(crate) fn start_tail_loop(
        &mut self,
        tab_id: usize,
        generation: u64,
        connection_name: String,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let mut interval = tail::TAIL_INTERVAL_MS;
            loop {
                Timer::after(Duration::from_millis(interval)).await;
                let alive = this
                    .update(cx, |this, cx| {
                        let on_connection =
                            this.connection.connected.as_ref().map(|c| c.name.as_str())
                                == Some(connection_name.as_str());
                        let config = this
                            .connection
                            .connected
                            .as_ref()
                            .map(|c| c.client_config.clone());
                        let mut lost_native = false;
                        let (live, paused, push) = {
                            let state = this
                                .query
                                .tabs
                                .iter_mut()
                                .find(|tab| tab.id == tab_id)
                                .and_then(|tab| tab.tail.as_mut());
                            match state {
                                Some(state) if on_connection && state.generation == generation => {
                                    // Fast mode needs the native connection;
                                    // when it is gone the polls are silently
                                    // riding HTTP already, so drop back to
                                    // the HTTP cadence and re-offer the
                                    // upgrade once the port answers again.
                                    if state.push == TailPush::Fast {
                                        let native_up = config.as_ref().is_some_and(|config| {
                                            zedb_ch::native::pooled(config).is_some()
                                        });
                                        if !native_up {
                                            state.push = TailPush::Poll;
                                            state.native_available = None;
                                            lost_native = true;
                                        }
                                    }
                                    (true, state.paused, state.push)
                                }
                                _ => (false, true, TailPush::Poll),
                            }
                        };
                        if !live {
                            return None;
                        }
                        if lost_native {
                            this.flash_notice(
                                "Native connection lost; tail back to HTTP polling",
                                cx,
                            );
                            this.probe_native_push(tab_id, generation, connection_name.clone(), cx);
                        }
                        if !paused && !matches!(push, TailPush::Stream | TailPush::Watch) {
                            this.tail_poll_once(tab_id, generation, connection_name.clone(), cx);
                        }
                        Some(match push {
                            TailPush::Fast => tail::TAIL_INTERVAL_FAST_MS,
                            _ => tail::TAIL_INTERVAL_MS,
                        })
                    })
                    .ok()
                    .flatten();
                match alive {
                    Some(next) => interval = next,
                    None => break,
                }
            }
        })
        .detach();
    }

    /// One off-thread poll: `seed_sql` while unprimed (grab the newest rows
    /// and install the header), then `poll_sql` for everything after the
    /// last seen key. New rows append and follow the bottom.
    pub(crate) fn tail_poll_once(
        &mut self,
        tab_id: usize,
        generation: u64,
        connection_name: String,
        cx: &mut Context<Self>,
    ) {
        let (config, sql, key) = {
            let Some(connected) = self.connection.connected.as_ref() else {
                return;
            };
            if connected.name != connection_name {
                return;
            }
            let Some(state) = self
                .query
                .tabs
                .iter()
                .find(|tab| tab.id == tab_id)
                .and_then(|tab| tab.tail.as_ref())
            else {
                return;
            };
            if state.generation != generation {
                return;
            }
            let sql = match &state.last {
                None => tail::seed_sql(&state.query, tail::TAIL_SEED),
                Some(last) => tail::poll_sql(&state.query, last, state.query.limit),
            };
            (
                connected.client_config.clone(),
                sql,
                state.query.key.clone(),
            )
        };
        let priming = self
            .query
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.tail.as_ref())
            .map(|state| state.last.is_none())
            .unwrap_or(false);
        let cap = self
            .query
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.tail.as_ref())
            .and_then(|state| state.cap)
            .unwrap_or(usize::MAX);
        let Some(grid) = self
            .query
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| tab.result_grid.clone())
        else {
            return;
        };

        let task = rt::tokio().spawn(async move { ChClient::new(config).query(&sql).await });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                let mut batch: Option<TailBatch> = None;
                {
                    let Some(state) = this
                        .query
                        .tabs
                        .iter_mut()
                        .find(|tab| tab.id == tab_id)
                        .and_then(|tab| tab.tail.as_mut())
                    else {
                        return;
                    };
                    if state.generation != generation {
                        return;
                    }
                    match result {
                        Ok(Ok(res)) => {
                            state.error = None;
                            if !res.rows.is_empty() {
                                let Some(idx) =
                                    res.columns.iter().position(|column| column.name == key)
                                else {
                                    state.error =
                                        Some(format!("tail key `{key}` is not in the result"));
                                    cx.notify();
                                    return;
                                };
                                state.key_index = idx;
                                if let Some(next) = tail::last_key(&res.rows, idx) {
                                    state.last = Some(next);
                                }
                                let columns = priming.then(|| res.columns.clone());
                                batch = Some((columns, res.rows));
                            }
                        }
                        Ok(Err(error)) => state.error = Some(error.to_string()),
                        Err(error) => state.error = Some(error.to_string()),
                    }
                }
                if let Some((columns, rows)) = batch {
                    let columns_len = columns.as_ref().map(|columns| columns.len());
                    if let Some(columns) = columns {
                        grid.update(cx, |grid, cx| grid.begin_result(columns, None, cx));
                    }
                    grid.update(cx, |grid, cx| grid.prepend_tail(rows, cap, cx));
                    let count = grid.read(cx).row_count();
                    if let Some(tab) = this.query.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                        tab.result_rows = count;
                        tab.has_result = true;
                        if let Some(len) = columns_len {
                            tab.result_columns = len;
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Stop the tail on a tab (its loop notices the cleared/renumbered
    /// generation and exits). The tab and its rows stay.
    pub(crate) fn stop_tail(&mut self, tab_id: usize, cx: &mut Context<Self>) {
        let config = self
            .connection
            .connected
            .as_ref()
            .map(|c| c.client_config.clone());
        if let Some(tab) = self.query.tabs.iter_mut().find(|tab| tab.id == tab_id) {
            if let Some(stream) = tab.tail.as_mut().and_then(|state| state.stream.take()) {
                stream.abort.abort();
            }
            if let (Some(config), Some(watch)) = (
                config,
                tab.tail.as_mut().and_then(|state| state.watch.take()),
            ) {
                drop_tail_view(config, watch.view.clone());
            }
            tab.tail = None;
            cx.notify();
        }
    }

    pub(crate) fn toggle_tail_pause(&mut self, tab_id: usize, cx: &mut Context<Self>) {
        let mut resume_stream = false;
        let connection_name = self.connection.connected.as_ref().map(|c| c.name.clone());
        let mut resume_watch_poll = None;
        if let Some(state) = self
            .query
            .tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.tail.as_mut())
        {
            state.paused = !state.paused;
            if state.push == TailPush::Stream {
                if state.paused {
                    // Do not buffer an unbounded result while paused. Closing
                    // the dedicated connection leaves the saved cursor in
                    // place for an exact resume.
                    if let Some(stream) = state.stream.take() {
                        stream.abort.abort();
                    }
                } else if state.stream.is_none() && !state.stream_connecting {
                    // A pause during the connect window leaves the stream
                    // task in flight; it delivers when it lands, so only a
                    // truly closed stream reopens here.
                    state.push = TailPush::Poll;
                    resume_stream = true;
                }
            } else if !state.paused && state.push == TailPush::Watch {
                resume_watch_poll = Some(state.generation);
            }
            cx.notify();
        }
        if resume_stream {
            self.upgrade_tail_instant(tab_id, cx);
        }
        if let (Some(generation), Some(connection_name)) = (resume_watch_poll, connection_name) {
            self.tail_poll_once(tab_id, generation, connection_name, cx);
        }
    }

    /// Adopt the tab editor's edited query as the tail's new definition. The
    /// edit is validated by running its seed once; if that errors (or the
    /// text isn't a tailable `SELECT ... FROM ... ORDER BY key`), the tail is
    /// left running unchanged and the reason is flashed.
    pub(crate) fn update_tail_from_editor(&mut self, tab_id: usize, cx: &mut Context<Self>) {
        let Some(connection_name) = self.connection.connected.as_ref().map(|c| c.name.clone())
        else {
            return;
        };
        let config = self
            .connection
            .connected
            .as_ref()
            .map(|c| c.client_config.clone());
        let (Some(config), Some(tab)) =
            (config, self.query.tabs.iter().find(|tab| tab.id == tab_id))
        else {
            return;
        };
        let Some(state) = tab.tail.as_ref() else {
            return;
        };
        let generation = state.generation;
        let edited = tab.editor.read(cx).value().to_string();
        let Some(parsed) = tail::parse_tail_query(&edited, tail::TAIL_BATCH) else {
            self.flash_warning(
                "That isn't a tailable query (need SELECT … FROM db.table … ORDER BY key); tail unchanged",
                cx,
            );
            return;
        };

        // Validate by running the seed once before switching over.
        let probe_sql = tail::seed_sql(&parsed, tail::TAIL_SEED);
        let task = rt::tokio().spawn(async move { ChClient::new(config).query(&probe_sql).await });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.connection.connected.as_ref().map(|c| c.name.as_str())
                    != Some(connection_name.as_str())
                {
                    return;
                }
                match result {
                    // The probe IS the seed, so its rows are the newest ones:
                    // adopt the query and repaint the grid from them right
                    // now, without waiting for the next poll or new inserts.
                    Ok(Ok(res)) => {
                        let grid = this.query.tabs
                            .iter()
                            .find(|tab| tab.id == tab_id)
                            .map(|tab| tab.result_grid.clone());
                        let cap = this.query.tabs
                            .iter()
                            .find(|tab| tab.id == tab_id)
                            .and_then(|tab| tab.tail.as_ref())
                            .and_then(|state| state.cap)
                            .unwrap_or(usize::MAX);
                        let key_index = res
                            .columns
                            .iter()
                            .position(|column| column.name == parsed.key);
                        // The cursor needs the ORDER BY key in the output;
                        // if the projection dropped it, keep the old tail.
                        if key_index.is_none() {
                            this.flash_warning(
                                format!(
                                    "The ORDER BY key `{}` must be in the SELECT to tail; tail unchanged",
                                    parsed.key
                                ),
                                cx,
                            );
                            cx.notify();
                            return;
                        }
                        let Some(state) = this.query.tabs
                            .iter_mut()
                            .find(|tab| tab.id == tab_id)
                            .and_then(|tab| tab.tail.as_mut())
                        else {
                            return;
                        };
                        if state.generation != generation {
                            return;
                        }
                        state.query = parsed.clone();
                        state.baseline = edited.clone();
                        state.error = None;
                        state.key_index = key_index.unwrap_or(0);
                        state.last = key_index.and_then(|idx| tail::last_key(&res.rows, idx));
                        // A stream is bound to the old body and cursor. Stop
                        // it and re-negotiate against the edited query.
                        let stale_stream = state.stream.take();
                        let stale_watch = state.watch.take();
                        state.stream_rejected = false;
                        if stale_stream.is_some() || stale_watch.is_some() {
                            state.push = TailPush::Poll;
                        }

                        if let Some(grid) = grid {
                            let columns = res.columns.clone();
                            let columns_len = columns.len();
                            let rows = res.rows;
                            grid.update(cx, |grid, cx| grid.clear_rows(cx));
                            if !rows.is_empty() {
                                grid.update(cx, |grid, cx| grid.begin_result(columns, None, cx));
                                grid.update(cx, |grid, cx| grid.prepend_tail(rows, cap, cx));
                            }
                            let count = grid.read(cx).row_count();
                            if let Some(tab) =
                                this.query.tabs.iter_mut().find(|tab| tab.id == tab_id)
                            {
                                tab.result_rows = count;
                                tab.has_result = true;
                                tab.result_columns = columns_len;
                            }
                        }
                        if let Some(stream) = stale_stream {
                            stream.abort.abort();
                            this.upgrade_tail_instant(tab_id, cx);
                        }
                        if let Some(watch) = stale_watch {
                            if let Some(config) =
                                this.connection.connected.as_ref().map(|c| c.client_config.clone())
                            {
                                drop_tail_view(config, watch.view.clone());
                            }
                            this.upgrade_tail_instant(tab_id, cx);
                        }
                        this.flash_notice("Tail updated", cx);
                    }
                    Ok(Err(error)) => {
                        this.flash_warning(format!("Query failed, tail unchanged: {error}"), cx);
                    }
                    Err(error) => {
                        this.flash_warning(format!("Query failed, tail unchanged: {error}"), cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
