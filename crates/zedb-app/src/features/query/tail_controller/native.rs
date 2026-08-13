use crate::*;

use gpui::prelude::*;

impl Workspace {
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
}
