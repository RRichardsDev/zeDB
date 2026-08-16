use crate::*;

use gpui::prelude::*;
impl Workspace {
    pub(crate) fn add_query_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.query.next_tab_id;
        self.query.next_tab_id += 1;
        let mut tab = Self::make_query_tab(id, "", self.schema.provider.clone(), window, cx);
        tab.connection = self.active_connection_name();
        self.query.tabs.push(tab);
        self.query.active_tab = self.query.tabs.len() - 1;
        self.show_query_editor = true;
        self.show_fleet = false;
        cx.notify();
    }

    /// The active connection's name, the scope open tabs belong to.
    pub(crate) fn active_connection_name(&self) -> Option<String> {
        self.connection
            .connected
            .as_ref()
            .map(|connected| connected.name.clone())
    }

    /// Whether a tab belongs on screen right now. Unowned tabs (never
    /// run anywhere) show everywhere; owned tabs show on their
    /// connection, and everything shows while disconnected so nothing
    /// silently vanishes.
    pub(crate) fn tab_on_active_connection(&self, tab: &QueryTab) -> bool {
        match (&tab.connection, self.connection.connected.as_ref()) {
            (Some(owner), Some(connected)) => *owner == connected.name,
            _ => true,
        }
    }

    /// Keep two invariants every frame: the active tab is visible on the
    /// current connection, and at least one tab is visible. Runs from
    /// render, the one place a Window is always in hand.
    pub(crate) fn ensure_visible_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active_visible = self
            .query
            .tabs
            .get(self.query.active_tab)
            .is_some_and(|tab| self.tab_on_active_connection(tab));
        if active_visible {
            return;
        }
        let nearest = {
            let visible = |tab: &QueryTab| self.tab_on_active_connection(tab);
            self.query
                .tabs
                .iter()
                .enumerate()
                .skip(self.query.active_tab)
                .find(|(_, tab)| visible(tab))
                .or_else(|| {
                    self.query
                        .tabs
                        .iter()
                        .enumerate()
                        .take(self.query.active_tab)
                        .rev()
                        .find(|(_, tab)| visible(tab))
                })
                .map(|(index, _)| index)
        };
        match nearest {
            Some(index) => self.query.active_tab = index,
            None => self.add_query_tab(window, cx),
        }
        cx.notify();
    }

    pub(crate) fn close_query_tab(&mut self, tab_id: usize, cx: &mut Context<Self>) {
        // Any tab may close, the last one included; the render
        // invariant opens a fresh scratch tab when none is left.
        let Some(index) = self.query.tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };
        if matches!(
            self.query.tabs[index].outcome,
            QueryOutcome::Running | QueryOutcome::StatementError { .. }
        ) {
            return;
        }
        self.stop_tail(tab_id, cx);
        let closed = self.query.tabs.remove(index);
        // A closed tab may hold a huge result; clear it into a
        // background drop before the entity goes away.
        closed.result_grid.update(cx, |grid, _| grid.release_rows());
        drop(closed);
        self.query.active_tab = self
            .query
            .active_tab
            .min(self.query.tabs.len().saturating_sub(1));
        cx.notify();
    }

    /// Close every tab whose id is in `close_ids`, keeping `focus_id`
    /// active. Running / errored tabs are protected (never closed), and the
    /// focus tab always survives, so at least one tab remains.
    pub(crate) fn close_query_tab_ids(
        &mut self,
        close_ids: &[usize],
        focus_id: usize,
        cx: &mut Context<Self>,
    ) {
        if close_ids.is_empty() {
            return;
        }
        let tail_ids: Vec<usize> = self
            .query
            .tabs
            .iter()
            .filter(|tab| {
                tab.id != focus_id
                    && close_ids.contains(&tab.id)
                    && !matches!(
                        tab.outcome,
                        QueryOutcome::Running | QueryOutcome::StatementError { .. }
                    )
            })
            .map(|tab| tab.id)
            .collect();
        for tab_id in tail_ids {
            self.stop_tail(tab_id, cx);
        }
        let mut kept = Vec::with_capacity(self.query.tabs.len());
        let mut dropped = Vec::new();
        for tab in self.query.tabs.drain(..) {
            let closable = !matches!(
                tab.outcome,
                QueryOutcome::Running | QueryOutcome::StatementError { .. }
            );
            if tab.id != focus_id && closable && close_ids.contains(&tab.id) {
                dropped.push(tab);
            } else {
                kept.push(tab);
            }
        }
        self.query.tabs = kept;
        // A closed tab may hold a huge result; drop it in the background.
        for tab in &dropped {
            tab.result_grid.update(cx, |grid, _| grid.release_rows());
        }
        drop(dropped);
        self.query.active_tab = self
            .query
            .tabs
            .iter()
            .position(|tab| tab.id == focus_id)
            .unwrap_or_else(|| {
                self.query
                    .active_tab
                    .min(self.query.tabs.len().saturating_sub(1))
            });
        cx.notify();
    }

    /// Move a query tab from one strip position to another (drag reorder),
    /// keeping the currently-active tab active.
    pub(crate) fn reorder_query_tab(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        let len = self.query.tabs.len();
        if from == to || from >= len || to >= len {
            return;
        }
        let active_id = self.query.tabs.get(self.query.active_tab).map(|tab| tab.id);
        let tab = self.query.tabs.remove(from);
        self.query.tabs.insert(to, tab);
        if let Some(id) = active_id {
            if let Some(pos) = self.query.tabs.iter().position(|tab| tab.id == id) {
                self.query.active_tab = pos;
            }
        }
        cx.notify();
    }

    pub(crate) fn close_other_query_tabs(&mut self, keep_id: usize, cx: &mut Context<Self>) {
        // Bulk closes act on what the user can see: another connection's
        // hidden tabs are not "others", they are elsewhere.
        let ids: Vec<usize> = self
            .query
            .tabs
            .iter()
            .filter(|tab| tab.id != keep_id && self.tab_on_active_connection(tab))
            .map(|tab| tab.id)
            .collect();
        self.close_query_tab_ids(&ids, keep_id, cx);
    }

    pub(crate) fn close_query_tabs_to_right(&mut self, from_id: usize, cx: &mut Context<Self>) {
        let Some(pos) = self.query.tabs.iter().position(|tab| tab.id == from_id) else {
            return;
        };
        let ids: Vec<usize> = self.query.tabs[pos + 1..]
            .iter()
            .filter(|tab| self.tab_on_active_connection(tab))
            .map(|tab| tab.id)
            .collect();
        self.close_query_tab_ids(&ids, from_id, cx);
    }
}
