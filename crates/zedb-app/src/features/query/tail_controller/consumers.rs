use crate::*;

use gpui::prelude::*;

impl Workspace {
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
}
