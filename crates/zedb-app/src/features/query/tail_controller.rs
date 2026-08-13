use crate::*;

use gpui::prelude::*;
impl Workspace {
    /// Begin a live tail of a table (Phase 10): open a fresh tab, resolve
    /// the monotonic key (the table's leading ORDER BY column), and start
    /// polling `WHERE key > :last` off the main thread.
    pub(crate) fn start_tail(
        &mut self,
        database: String,
        object: String,
        cap: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(connected) = self.connection.connected.as_ref() else {
            self.flash_warning("Connect before tailing a table", cx);
            return;
        };
        let config = connected.client_config.clone();
        let connection_name = connected.name.clone();

        // A dedicated tab hosts the tail so it never fights a real query.
        self.add_query_tab(window, cx);
        let tab_id = self.query.tabs[self.query.active_tab].id;

        self.query.next_tail_generation += 1;
        let generation = self.query.next_tail_generation;
        self.query.next_tail_number += 1;
        let number = self.query.next_tail_number;
        let qualified = format!("{database}.{object}");
        let task = rt::tokio().spawn(async move {
            let client = ChClient::new(config);
            fetch_table_keys(&client, Some(&qualified))
                .await
                .and_then(|(order_by, _)| order_by.into_iter().next())
                .and_then(|first| first_tail_key(&first))
        });
        cx.spawn_in(window, async move |this, cx| {
            let key = task.await.ok().flatten();
            this.update_in(cx, |this, window, cx| {
                let Some(key) = key else {
                    this.flash_warning(
                        format!("{database}.{object} has no simple ORDER BY key to tail on"),
                        cx,
                    );
                    // Leave the empty tab in place; the user can close it.
                    return;
                };
                if let Some(tab) = this.query.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                    let query = tail::TailQuery {
                        body: tail::table_body(&database, &object),
                        key,
                        limit: tail::TAIL_BATCH,
                    };
                    // Show the runnable base query in the tab editor; editing
                    // it and pressing "update tail" re-parses it.
                    let baseline = tail::base_sql(&query);
                    let editor = tab.editor.clone();
                    let display = baseline.clone();
                    editor.update(cx, |editor, cx| editor.set_value(display, window, cx));
                    tab.tail = Some(TailState {
                        number,
                        query,
                        baseline,
                        last: None,
                        key_index: 0,
                        cap,
                        native_available: None,
                        push: TailPush::Poll,
                        stream_cursor: None,
                        stream: None,
                        watch: None,
                        stream_rejected: false,
                        generation,
                        paused: false,
                        error: None,
                    });
                    tab.has_result = true;
                }
                // One immediate poll to prime, then the timer loop.
                this.tail_poll_once(tab_id, generation, connection_name.clone(), cx);
                this.start_tail_loop(tab_id, generation, connection_name.clone(), cx);
                // Discover whether a native (TCP) port is reachable, to
                // offer the "instant updates" upgrade only when possible.
                this.probe_native_push(tab_id, generation, connection_name, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

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
                } else if state.stream.is_none() {
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
                        state.stream_cursor = None;
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

    /// Discover whether this connection's server is reachable over the
    /// native (TCP) protocol, by actually establishing the pooled native
    /// connection: the server names its own ports, the socket is proven
    /// to be the same server, and general reads start riding it right
    /// away. Success surfaces the "instant updates" button; poll-over-HTTP
    /// tail works everywhere regardless.
    pub(crate) fn probe_native_push(
        &mut self,
        tab_id: usize,
        generation: u64,
        connection_name: String,
        cx: &mut Context<Self>,
    ) {
        let Some(config) = self
            .connection
            .connected
            .as_ref()
            .map(|connected| connected.client_config.clone())
        else {
            return;
        };
        let task = rt::tokio().spawn(async move { zedb_ch::native::connect_pooled(&config).await });
        cx.spawn(async move |this, cx| {
            let reachable = task.await.is_ok_and(|connected| connected.is_ok());
            this.update(cx, |this, cx| {
                if this.connection.connected.as_ref().map(|c| c.name.as_str())
                    != Some(connection_name.as_str())
                {
                    return;
                }
                if let Some(state) = this
                    .query
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.id == tab_id)
                    .and_then(|tab| tab.tail.as_mut())
                {
                    if state.generation == generation {
                        state.native_available = Some(reachable);
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Switch a tail to "instant updates" over native TCP. Experimental
    /// STREAM is opt-in; WATCH remains the normal push path on versions that
    /// support Live Views, followed by native fast polling.
    pub(crate) fn upgrade_tail_instant(&mut self, tab_id: usize, cx: &mut Context<Self>) {
        let Some(connected) = self.connection.connected.as_ref() else {
            return;
        };
        let config = connected.client_config.clone();
        let connection_name = connected.name.clone();
        let Some(state) = self
            .query
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.tail.as_ref())
        else {
            return;
        };
        if state.push != TailPush::Poll {
            return;
        }
        let generation = state.generation;
        let body = state.query.body.clone();
        let stream_sql = self
            .preferences
            .experimental_streaming_queries
            .then(|| tail::stream_sql(&state.query, state.stream_cursor, state.last.as_deref()))
            .flatten()
            .filter(|_| !state.stream_rejected);
        let stream_requested = stream_sql.is_some();
        self.query.next_stream_epoch += 1;
        let epoch = self.query.next_stream_epoch;
        let view = format!("zedb_tail_{epoch}");
        self.flash_notice("Connecting for instant updates…", cx);

        enum Instant {
            Stream {
                receiver: tokio::sync::mpsc::UnboundedReceiver<TailStreamBatch>,
                abort: tokio::task::AbortHandle,
            },
            Watch {
                receiver: tokio::sync::mpsc::UnboundedReceiver<()>,
                abort: tokio::task::AbortHandle,
                stream_rejected: bool,
            },
            FastPoll {
                stream_rejected: bool,
            },
        }
        let read_only = config.read_only;
        let setup_config = config.clone();
        let setup_view = view.clone();
        let task = rt::tokio().spawn(async move {
            let pooled = zedb_ch::native::connect_pooled(&setup_config).await?;
            let mut stream_rejected = false;
            if let Some(stream_sql) = stream_sql {
                let version = pooled
                    .query("SELECT version()")
                    .await
                    .ok()
                    .and_then(|result| result.rows.first().cloned())
                    .and_then(|row| row.first().map(ToString::to_string));
                if version
                    .as_deref()
                    .is_some_and(tail::supports_streaming_version)
                {
                    if let Ok(streamer) =
                        zedb_ch::native::NativeClient::connect(&setup_config).await
                    {
                        let preflight = streamer
                            .query(&format!("EXPLAIN SYNTAX {stream_sql}"))
                            .await;
                        if preflight.is_ok() {
                            let (sender, receiver) =
                                tokio::sync::mpsc::unbounded_channel::<TailStreamBatch>();
                            let stream_task = tokio::spawn(async move {
                                let _ = streamer
                                    .stream_blocks(&stream_sql, |columns, rows| {
                                        sender.send((columns, rows)).is_ok()
                                    })
                                    .await;
                            });
                            let abort = stream_task.abort_handle();
                            return Ok::<Instant, zedb_ch::ChError>(Instant::Stream {
                                receiver,
                                abort,
                            });
                        }
                    }
                }
                stream_rejected = true;
            }
            if read_only {
                return Ok(Instant::FastPoll { stream_rejected });
            }
            // The WATCH holds its own connection open indefinitely: the
            // native protocol runs one query at a time per connection, so
            // it must never share the pooled one.
            let Ok(watcher) = zedb_ch::native::NativeClient::connect(&setup_config).await else {
                return Ok(Instant::FastPoll { stream_rejected });
            };
            let experimental = watcher
                .execute("SET allow_experimental_live_view = 1")
                .await;
            let created = match experimental {
                Ok(()) => {
                    watcher
                        .execute(&format!(
                            "CREATE LIVE VIEW {setup_view} AS SELECT count() FROM ({body})"
                        ))
                        .await
                }
                Err(error) => Err(error),
            };
            if created.is_err() {
                // Live views are experimental and semi-deprecated; any
                // refusal (setting locked, feature removed, no grant)
                // lands here and fast polling takes over.
                return Ok(Instant::FastPoll { stream_rejected });
            }
            let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<()>();
            let watch_task = tokio::spawn(async move {
                // Runs until the server ends the stream, the connection
                // drops, or the consumer goes away (send fails).
                let _ = watcher
                    .stream_blocks(&format!("WATCH {setup_view} EVENTS"), |_, _| {
                        sender.send(()).is_ok()
                    })
                    .await;
            });
            let abort = watch_task.abort_handle();
            Ok::<Instant, zedb_ch::ChError>(Instant::Watch {
                receiver,
                abort,
                stream_rejected,
            })
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                let alive = this.connection.connected.as_ref().map(|c| c.name.as_str())
                    == Some(connection_name.as_str());
                let Some(state) = this
                    .query
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.id == tab_id)
                    .and_then(|tab| tab.tail.as_mut())
                    .filter(|state| alive && state.generation == generation)
                else {
                    // The tail is gone; if a watch got set up, tear its
                    // view down (the stream ends when the receiver drops).
                    if let Ok(Ok(Instant::Watch { abort, .. })) = &outcome {
                        abort.abort();
                        drop_tail_view(config.clone(), view.clone());
                    }
                    if let Ok(Ok(Instant::Stream { abort, .. })) = &outcome {
                        abort.abort();
                    }
                    return;
                };
                match outcome {
                    Ok(Ok(Instant::Stream { receiver, abort })) => {
                        state.push = TailPush::Stream;
                        state.stream = Some(TailStream { epoch, abort });
                        this.flash_notice("Instant updates on: experimental STREAM over TCP", cx);
                        this.start_tail_stream_consumer(
                            tab_id,
                            generation,
                            epoch,
                            connection_name.clone(),
                            receiver,
                            cx,
                        );
                    }
                    Ok(Ok(Instant::Watch {
                        receiver,
                        abort,
                        stream_rejected,
                    })) => {
                        state.stream_rejected |= stream_rejected;
                        state.push = TailPush::Watch;
                        state.watch = Some(TailWatch {
                            view: view.clone(),
                            epoch,
                            abort,
                        });
                        this.flash_notice("Instant updates on: server push over TCP", cx);
                        this.start_tail_watch_consumer(
                            tab_id,
                            generation,
                            epoch,
                            connection_name.clone(),
                            receiver,
                            cx,
                        );
                    }
                    Ok(Ok(Instant::FastPoll { stream_rejected })) => {
                        state.stream_rejected |= stream_rejected;
                        state.push = TailPush::Fast;
                        this.flash_notice(
                            if stream_requested && stream_rejected {
                                "STREAM unavailable; using fast native polling"
                            } else {
                                "Instant updates on: fast polling over the native connection"
                            },
                            cx,
                        );
                    }
                    Ok(Err(error)) => {
                        state.native_available = Some(false);
                        this.flash_warning(
                            format!("Couldn't connect to the native port: {error}"),
                            cx,
                        );
                    }
                    Err(error) => {
                        state.native_available = Some(false);
                        this.flash_warning(format!("Instant updates failed: {error}"), cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Consume rows returned directly by ClickHouse `STREAM CURSOR`. The two
    /// private leading columns advance the resumable server cursor and are
    /// removed before rows reach the grid.
    pub(crate) fn start_tail_stream_consumer(
        &mut self,
        tab_id: usize,
        generation: u64,
        epoch: u64,
        connection_name: String,
        mut receiver: tokio::sync::mpsc::UnboundedReceiver<TailStreamBatch>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            while let Some((mut columns, mut rows)) = receiver.recv().await {
                let alive = this
                    .update(cx, |this, cx| {
                        let on_connection =
                            this.connection.connected.as_ref().map(|c| c.name.as_str())
                                == Some(connection_name.as_str());
                        let Some(state) = this
                            .query
                            .tabs
                            .iter_mut()
                            .find(|tab| tab.id == tab_id)
                            .and_then(|tab| tab.tail.as_mut())
                            .filter(|state| {
                                on_connection
                                    && state.generation == generation
                                    && state
                                        .stream
                                        .as_ref()
                                        .is_some_and(|stream| stream.epoch == epoch)
                            })
                        else {
                            return false;
                        };
                        if columns.len() < 2
                            || columns[0].name != tail::STREAM_BLOCK_COLUMN
                            || columns[1].name != tail::STREAM_OFFSET_COLUMN
                        {
                            state.error = Some("STREAM did not return a resumable cursor".into());
                            return false;
                        }
                        if let Some(cursor) = rows.last().and_then(|row| tail::stream_cursor(row)) {
                            state.stream_cursor = Some(cursor);
                        }
                        columns.drain(..2);
                        for row in &mut rows {
                            if row.len() >= 2 {
                                row.drain(..2);
                            }
                        }
                        let Some(key_index) = columns
                            .iter()
                            .position(|column| column.name == state.query.key)
                        else {
                            state.error = Some(format!(
                                "tail key `{}` is not in the streamed result",
                                state.query.key
                            ));
                            return false;
                        };
                        state.key_index = key_index;
                        if let Some(last) = tail::last_key(&rows, key_index) {
                            state.last = Some(last);
                        }
                        state.error = None;
                        let cap = state.cap.unwrap_or(usize::MAX);
                        let grid = this
                            .query
                            .tabs
                            .iter()
                            .find(|tab| tab.id == tab_id)
                            .map(|tab| tab.result_grid.clone());
                        if let Some(grid) = grid {
                            grid.update(cx, |grid, cx| grid.prepend_tail(rows, cap, cx));
                            let count = grid.read(cx).row_count();
                            if let Some(tab) =
                                this.query.tabs.iter_mut().find(|tab| tab.id == tab_id)
                            {
                                tab.result_rows = count;
                                tab.has_result = true;
                                tab.result_columns = columns.len();
                            }
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !alive {
                    break;
                }
            }

            this.update(cx, |this, cx| {
                let on_connection = this.connection.connected.as_ref().map(|c| c.name.as_str())
                    == Some(connection_name.as_str());
                let ended = this
                    .query
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.id == tab_id)
                    .and_then(|tab| tab.tail.as_mut())
                    .filter(|state| {
                        on_connection
                            && state.generation == generation
                            && state
                                .stream
                                .as_ref()
                                .is_some_and(|stream| stream.epoch == epoch)
                    })
                    .map(|state| {
                        state.stream = None;
                        state.stream_rejected = true;
                        state.push = TailPush::Poll;
                    })
                    .is_some();
                if ended {
                    this.flash_notice("Experimental STREAM ended; trying WATCH", cx);
                    this.upgrade_tail_instant(tab_id, cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Consume one watch's events: each server-pushed event triggers one
    /// poll (which rides the pooled native connection). When the stream
    /// ends, the tail drops back to plain polling and the upgrade is
    /// re-offered once the native port answers again.
    pub(crate) fn start_tail_watch_consumer(
        &mut self,
        tab_id: usize,
        generation: u64,
        epoch: u64,
        connection_name: String,
        mut receiver: tokio::sync::mpsc::UnboundedReceiver<()>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            loop {
                let event = receiver.recv().await;
                // Coalesce a burst of events into one poll.
                while receiver.try_recv().is_ok() {}
                let ended = event.is_none();
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
                        let mut downgraded = false;
                        let (live, paused) = {
                            let state = this
                                .query
                                .tabs
                                .iter_mut()
                                .find(|tab| tab.id == tab_id)
                                .and_then(|tab| tab.tail.as_mut());
                            match state {
                                Some(state)
                                    if on_connection
                                        && state.generation == generation
                                        && state
                                            .watch
                                            .as_ref()
                                            .is_some_and(|watch| watch.epoch == epoch) =>
                                {
                                    if ended {
                                        // Server-push ended (connection
                                        // drop, live view gone): back to
                                        // polling, silently resuming from
                                        // the last-seen key.
                                        let view =
                                            state.watch.take().map(|watch| watch.view.clone());
                                        state.push = TailPush::Poll;
                                        state.native_available = None;
                                        downgraded = true;
                                        if let (Some(config), Some(view)) = (config, view) {
                                            drop_tail_view(config, view);
                                        }
                                    }
                                    (true, state.paused)
                                }
                                _ => (false, true),
                            }
                        };
                        if downgraded {
                            this.flash_notice("Instant updates ended; tail back to polling", cx);
                            this.probe_native_push(tab_id, generation, connection_name.clone(), cx);
                            cx.notify();
                        }
                        if live && !ended && !paused {
                            this.tail_poll_once(tab_id, generation, connection_name.clone(), cx);
                        }
                        live
                    })
                    .unwrap_or(false);
                if ended || !alive {
                    break;
                }
            }
        })
        .detach();
    }

    /// The live-tail status strip above the editor: what's tailing, the
    /// retained row count, and Pause / Stop.
    pub(crate) fn tail_strip(
        &self,
        info: TailStripInfo,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let TailStripInfo {
            tab_id,
            key,
            paused,
            error,
            rows,
            native_available,
            push,
            experimental_streaming_enabled,
            dirty,
        } = info;
        let icon_button = |id: &'static str, icon: &'static str, color: gpui::Hsla| {
            div()
                .id(id)
                .size(px(22.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(3.))
                .hover(|button| button.bg(theme::hover()).cursor_pointer())
                .child(svg().path(icon).size(px(13.)).text_color(color))
        };
        div()
            .flex_none()
            .h(px(30.))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .bg(theme::bg_sidebar())
            // An orange outline on the whole strip while the query is edited
            // (unapplied), alongside the green Update Tail button.
            .when(dirty, |strip| {
                strip.border_1().border_color(theme::warning())
            })
            .child(
                // A live dot: accent when following, dim when paused.
                div().size(px(7.)).rounded_full().bg(if paused {
                    theme::text_dim()
                } else {
                    theme::accent()
                }),
            )
            .child(div().text_xs().text_color(theme::text()).child(if paused {
                format!("Tail paused · advancing on {key}")
            } else {
                format!("Tailing · advancing on {key}")
            }))
            .child(
                div()
                    .text_xs()
                    .text_color(theme::text_dim())
                    .child(format!("· {rows} rows")),
            )
            .when(push != TailPush::Poll, |row| {
                // Instant updates active: name the mechanism.
                row.child(
                    div()
                        .text_xs()
                        .text_color(theme::accent())
                        .child(match push {
                            TailPush::Stream => "· instant (STREAM)",
                            TailPush::Watch => "· instant (WATCH)",
                            _ => "· instant (native)",
                        }),
                )
            })
            .when_some(error, |row, error| {
                row.child(
                    div()
                        .text_xs()
                        .text_color(theme::danger())
                        .child(format!("· {error}")),
                )
            })
            .child(div().flex_1())
            .when(dirty, |row| {
                // The editor query was edited; a green-outlined text button
                // (left of "Get instant updates") that reads as "apply your
                // changes".
                row.child(
                    div()
                        .id("tail-update")
                        .px_2()
                        .py_0p5()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(theme::success())
                        .text_xs()
                        .text_color(theme::success())
                        .child("Update Tail")
                        .hover(|button| button.bg(theme::hover()).cursor_pointer())
                        .tooltip(|window, cx| {
                            gpui_component::tooltip::Tooltip::new(
                                "Apply the edited query to the tail",
                            )
                            .build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.update_tail_from_editor(tab_id, cx)
                        })),
                )
            })
            .when(native_available && push == TailPush::Poll, |row| {
                row.child(
                    icon_button(
                        "tail-experimental-settings",
                        "icons/experimental.svg",
                        if experimental_streaming_enabled {
                            theme::warning()
                        } else {
                            theme::text_dim()
                        },
                    )
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(if experimental_streaming_enabled {
                            "Experimental STREAM tails enabled. Open Preferences"
                        } else {
                            "Experimental STREAM tails disabled. Open Preferences"
                        })
                        .build(window, cx)
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.open_preferences(cx))),
                )
            })
            .when(native_available && push == TailPush::Poll, |row| {
                // Discovery found a native port: offer the server-push
                // upgrade, accent-tinted so it reads as an offer.
                row.child(
                    div()
                        .id("tail-instant")
                        .px_2()
                        .py_0p5()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(theme::accent())
                        .text_xs()
                        .text_color(theme::accent())
                        .child("Get instant updates")
                        .hover(|button| button.bg(theme::hover()).cursor_pointer())
                        .tooltip(|window, cx| {
                            gpui_component::tooltip::Tooltip::new(
                                "Switch to the native (TCP) connection for instant updates",
                            )
                            .build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.upgrade_tail_instant(tab_id, cx);
                        })),
                )
            })
            .child(
                // Paused shows green Play (resume); running shows orange
                // Pause. Stop is always red.
                if paused {
                    icon_button("tail-play", "icons/play.svg", theme::success()).tooltip(
                        |window, cx| {
                            gpui_component::tooltip::Tooltip::new("Resume").build(window, cx)
                        },
                    )
                } else {
                    icon_button("tail-pause", "icons/pause.svg", theme::warning()).tooltip(
                        |window, cx| {
                            gpui_component::tooltip::Tooltip::new("Pause").build(window, cx)
                        },
                    )
                }
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_tail_pause(tab_id, cx))),
            )
            .child(
                icon_button("tail-stop", "icons/stop.svg", theme::danger())
                    .tooltip(|window, cx| {
                        gpui_component::tooltip::Tooltip::new("Stop").build(window, cx)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.stop_tail(tab_id, cx))),
            )
    }
}
