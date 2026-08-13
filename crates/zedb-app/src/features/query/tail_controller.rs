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
}
