//! Query history + saved queries (docs/PHASE-7.1-IDEAS.md, first
//! bite): every statement zeDB runs is recorded locally per
//! connection; named snippets live in settings.json and sync. The
//! drawer docks to the right of the query editor, split into History
//! and Saved tabs. Bookmarking saves immediately under a name derived
//! from the query; renaming happens inline on the Saved tab.

use std::cell::RefCell;
use std::collections::HashMap;

use gpui::{div, prelude::*, px, svg, Context, Focusable as _, HighlightStyle, Window};
use zedb_core::{HistoryEntry, SavedQuery};

use crate::{theme, Workspace};

/// Rows shown per section before search narrows things down.
const HISTORY_SHOWN: usize = 200;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum HistoryTab {
    #[default]
    History,
    Saved,
    Tabs,
}

impl HistoryTab {
    const ALL: [HistoryTab; 3] = [HistoryTab::History, HistoryTab::Saved, HistoryTab::Tabs];

    fn label(self) -> &'static str {
        match self {
            HistoryTab::History => "History",
            HistoryTab::Saved => "Saved",
            HistoryTab::Tabs => "Tabs",
        }
    }
}

impl Workspace {
    pub(crate) fn save_active_query_tab(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.query.tabs.get(self.query.active_tab) else {
            self.flash_warning("There is no active query tab to save", cx);
            return;
        };
        let saved_id = tab
            .saved_tab_id
            .clone()
            .unwrap_or_else(|| zedb_core::new_local_id("saved-tab"));
        let name = crate::tab_display_name(tab);
        let saved = zedb_core::SavedTab {
            id: saved_id.clone(),
            name: name.clone(),
            sql: tab.editor.read(cx).value().to_string(),
            row_limit: tab.max_rows.limit(),
            saved_at: unix_now(),
        };
        let mut updated = self.saved_tabs.clone();
        if let Some(index) = updated.iter().position(|tab| tab.id == saved_id) {
            updated.remove(index);
            updated.insert(0, saved);
        } else {
            updated.insert(0, saved);
            updated.truncate(zedb_core::SAVED_TAB_CAP);
        }

        if let Err(error) = zedb_core::save_saved_tabs(&updated) {
            self.flash_warning(format!("Could not save tab: {error}"), cx);
            return;
        }
        self.saved_tabs = updated;
        self.query.tabs[self.query.active_tab].saved_tab_id = Some(saved_id);
        self.history_tab = HistoryTab::Tabs;
        self.show_history = true;
        self.history_clear_armed = false;
        if self.connection.connected.is_some() {
            self.show_query_editor = true;
            self.show_ops = false;
            self.show_fleet = false;
        }
        self.flash_notice(format!("Saved tab {name}"), cx);
    }

    fn saved_tab_rename_commit(&mut self, cx: &mut Context<Self>) {
        let Some((id, original, input)) = self.saved_tab_renaming.take() else {
            return;
        };
        let new_name = input.read(cx).text().trim().to_string();
        if new_name.is_empty() || new_name == original {
            cx.notify();
            return;
        }
        let mut updated = self.saved_tabs.clone();
        if let Some(saved) = updated.iter_mut().find(|saved| saved.id == id) {
            saved.name = new_name.clone();
        }
        if let Err(error) = zedb_core::save_saved_tabs(&updated) {
            self.flash_warning(format!("Could not rename saved tab: {error}"), cx);
            return;
        }
        self.saved_tabs = updated;
        for tab in &mut self.query.tabs {
            if tab.saved_tab_id.as_deref() == Some(id.as_str()) {
                tab.name = new_name.clone();
            }
        }
        cx.notify();
    }

    fn saved_tab_delete(&mut self, id: &str, cx: &mut Context<Self>) {
        let mut updated = self.saved_tabs.clone();
        updated.retain(|saved| saved.id != id);
        if let Err(error) = zedb_core::save_saved_tabs(&updated) {
            self.flash_warning(format!("Could not delete saved tab: {error}"), cx);
            return;
        }
        self.saved_tabs = updated;
        for tab in &mut self.query.tabs {
            if tab.saved_tab_id.as_deref() == Some(id) {
                tab.saved_tab_id = None;
            }
        }
        cx.notify();
    }

    fn saved_tab_open(&mut self, saved_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(saved) = self
            .saved_tabs
            .iter()
            .find(|saved| saved.id == saved_id)
            .cloned()
        else {
            return;
        };
        let name = saved.name.clone();
        let id = self.query.next_tab_id;
        self.query.next_tab_id += 1;
        let mut tab =
            Self::make_query_tab(id, &saved.sql, self.schema.provider.clone(), window, cx);
        tab.saved_tab_id = Some(saved.id);
        tab.name = saved.name;
        tab.max_rows = crate::max_rows_from_limit(saved.row_limit);
        self.query.tabs.push(tab);
        self.query.active_tab = self.query.tabs.len() - 1;
        self.show_query_editor = true;
        self.show_fleet = false;
        self.show_ops = false;
        self.flash_notice(format!("Opened saved tab {name}"), cx);
    }

    pub(crate) fn history_toggle(&mut self, cx: &mut Context<Self>) {
        self.show_history = !self.show_history;
        // The drawer lives beside the editor; surface it.
        if self.show_history && self.connection.connected.is_some() && !self.show_query_editor {
            self.show_query_editor = true;
            self.show_ops = false;
            self.show_fleet = false;
        }
        cx.notify();
    }

    /// Record a finished run. Called from the run-completion handler
    /// with whatever actually executed.
    pub(crate) fn history_record(
        &mut self,
        sqls: &[String],
        duration_ms: Option<u64>,
        rows: Option<u64>,
        error: Option<&str>,
    ) {
        let Some(connected) = &self.connection.connected else {
            return;
        };
        let connection = connected.name.clone();
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs() as i64)
            .unwrap_or(0);
        let single = sqls.len() == 1;
        let last = sqls.len().saturating_sub(1);
        for (index, sql) in sqls.iter().enumerate() {
            let error_line = error
                .filter(|_| index == last)
                .map(|error| error.lines().next().unwrap_or_default().to_string())
                .map(|mut line| {
                    if line.len() > 200 {
                        let mut cut = 200;
                        while !line.is_char_boundary(cut) {
                            cut -= 1;
                        }
                        line.truncate(cut);
                        line.push('\u{2026}');
                    }
                    line
                });
            zedb_core::push_entry(
                &mut self.history,
                HistoryEntry {
                    sql: sql.clone(),
                    connection: connection.clone(),
                    at,
                    duration_ms: duration_ms.filter(|_| single),
                    rows: rows.filter(|_| index == last && error.is_none()),
                    error: error_line,
                },
            );
        }
        let _ = zedb_core::save_history(&self.history);
    }

    fn history_insert(&mut self, sql: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.query.tabs.get(self.query.active_tab) else {
            return;
        };
        let editor = tab.editor.clone();
        editor.update(cx, |editor, cx| {
            let text = editor.value().to_string();
            let cursor = editor.cursor();
            // Land on its own paragraph: a blank line before (unless
            // at the buffer start), and its own line after.
            let mut insert = String::new();
            if cursor > 0 {
                let before = &text[..cursor];
                if before.ends_with("\n\n") {
                    // Already a blank line.
                } else if before.ends_with('\n') {
                    insert.push('\n');
                } else {
                    insert.push_str("\n\n");
                }
            }
            insert.push_str(sql);
            if !sql.ends_with(';') {
                insert.push(';');
            }
            if !text[cursor..].starts_with('\n') && !text[cursor..].is_empty() {
                insert.push('\n');
            }
            editor.insert(insert, window, cx);
        });
        let handle = editor.read(cx).focus_handle(cx);
        window.focus(&handle);
        cx.notify();
    }

    /// Bookmark: save immediately under a name derived from the query
    /// (first 25 chars), uniqued with a numeric suffix. Rename later
    /// from the Saved tab.
    fn history_save(&mut self, sql: &str, cx: &mut Context<Self>) {
        let base = default_name(sql);
        let mut name = base.clone();
        let mut counter = 2;
        while self
            .preferences
            .saved_queries
            .iter()
            .any(|saved| saved.name == name)
        {
            name = format!("{base} {counter}");
            counter += 1;
        }
        let saved_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs() as i64)
            .unwrap_or(0);
        self.preferences.saved_queries.push(SavedQuery {
            name,
            sql: sql.to_string(),
            favorite: false,
            saved_at,
        });
        sort_saved(&mut self.preferences.saved_queries);
        if let Err(error) = zedb_core::save_preferences(&self.preferences) {
            self.flash_warning(format!("Could not save query: {error}"), cx);
        }
        self.settings_sync_tick(cx);
        cx.notify();
    }

    fn history_rename_commit(&mut self, cx: &mut Context<Self>) {
        let Some((original, input)) = self.history_renaming.take() else {
            return;
        };
        let new_name = input.read(cx).text().trim().to_string();
        if new_name.is_empty() || new_name == original {
            cx.notify();
            return;
        }
        // The name is the key: renaming onto an existing name replaces it.
        self.preferences
            .saved_queries
            .retain(|saved| saved.name != new_name);
        if let Some(saved) = self
            .preferences
            .saved_queries
            .iter_mut()
            .find(|saved| saved.name == original)
        {
            saved.name = new_name;
        }
        sort_saved(&mut self.preferences.saved_queries);
        let _ = zedb_core::save_preferences(&self.preferences);
        self.settings_sync_tick(cx);
        cx.notify();
    }

    fn history_clear(&mut self, cx: &mut Context<Self>) {
        self.history.clear();
        let _ = zedb_core::save_history(&self.history);
        self.history_clear_armed = false;
        cx.notify();
    }

    fn history_delete_saved(&mut self, name: &str, cx: &mut Context<Self>) {
        self.preferences
            .saved_queries
            .retain(|saved| saved.name != name);
        let _ = zedb_core::save_preferences(&self.preferences);
        self.settings_sync_tick(cx);
        cx.notify();
    }

    fn history_toggle_favorite(&mut self, name: &str, cx: &mut Context<Self>) {
        if let Some(saved) = self
            .preferences
            .saved_queries
            .iter_mut()
            .find(|saved| saved.name == name)
        {
            saved.favorite = !saved.favorite;
        }
        sort_saved(&mut self.preferences.saved_queries);
        let _ = zedb_core::save_preferences(&self.preferences);
        self.settings_sync_tick(cx);
        cx.notify();
    }

    /// The shared splitter pattern: a 1px divider with a wide invisible
    /// drag target centered on it.
    pub(crate) fn history_resize_handle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        gpui::deferred(
            div()
                .id("history-resize-handle")
                .w(px(13.))
                .h_full()
                .ml(px(-6.))
                .mr(px(-6.))
                .flex_none()
                .relative()
                .cursor_col_resize()
                .child(
                    div()
                        .absolute()
                        .left(px(6.))
                        .top_0()
                        .bottom_0()
                        .w(px(1.))
                        .bg(theme::border()),
                )
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                        this.history_resizing =
                            Some((this.history_width, f32::from(event.position.x)));
                        cx.notify();
                    }),
                ),
        )
    }

    pub(crate) fn history_drawer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let filter = self.history_search.read(cx).text().trim().to_lowercase();
        let connection = self
            .connection
            .connected
            .as_ref()
            .map(|connected| connected.name.clone());
        let active_tab = self.history_tab;

        let tabs = div()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .flex_1()
                    .flex()
                    .gap_4()
                    .children(HistoryTab::ALL.into_iter().map(|tab| {
                        let active = tab == active_tab;
                        div()
                            .id(tab.label())
                            .py_1p5()
                            .border_b_2()
                            .border_color(if active {
                                theme::accent()
                            } else {
                                gpui::transparent_black()
                            })
                            .text_sm()
                            .text_color(if active {
                                theme::text()
                            } else {
                                theme::text_dim()
                            })
                            .hover(|label| label.text_color(theme::text()).cursor_pointer())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.history_tab = tab;
                                this.history_clear_armed = false;
                                cx.notify();
                            }))
                            .child(tab.label())
                    })),
            );

        let header = div()
            .flex_none()
            .px_3()
            .pt_1()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div().flex().justify_end().child(
                    div()
                        .id("history-close")
                        .flex_none()
                        .px_1()
                        .rounded(px(3.))
                        .text_color(theme::text_dim())
                        .child("\u{00d7}")
                        .hover(|close| {
                            close
                                .bg(theme::hover())
                                .text_color(theme::text())
                                .cursor_pointer()
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_history = false;
                            cx.notify();
                        })),
                ),
            )
            // Explicit width: the input's own w_full does not resolve
            // against this container reliably.
            .child(
                div()
                    .w(px(self.history_width - 24.0))
                    .child(self.history_search.clone()),
            )
            .child(tabs);

        let content: gpui::AnyElement = match active_tab {
            HistoryTab::History => {
                let entries: Vec<&HistoryEntry> = self
                    .history
                    .iter()
                    .filter(|entry| filter.is_empty() || entry.sql.to_lowercase().contains(&filter))
                    .take(HISTORY_SHOWN)
                    .collect();
                let rows: Vec<_> = entries
                    .iter()
                    .enumerate()
                    .map(|(index, entry)| {
                        let sql = entry.sql.clone();
                        let save_sql = entry.sql.clone();
                        let hover_sql = entry.sql.clone();
                        let failed = entry.error.is_some();
                        // The relative time goes all the way to the right;
                        // connection / rows / ms (or the error) stay left.
                        let when = relative_time(entry.at);
                        let mut parts: Vec<String> = Vec::new();
                        if let Some(connection_name) = &connection {
                            if *connection_name != entry.connection {
                                parts.push(entry.connection.clone());
                            }
                        }
                        if let Some(rows) = entry.rows {
                            parts.push(format!("{rows} rows"));
                        }
                        if let Some(duration) = entry.duration_ms {
                            parts.push(format!("{duration} ms"));
                        }
                        let left_meta = match &entry.error {
                            Some(error) => error.clone(),
                            None => parts.join(" \u{b7} "),
                        };
                        div()
                            .id(("history-entry", index))
                            .group("history-row")
                            .px_3()
                            .py_1p5()
                            .flex()
                            .items_center()
                            .gap_2()
                            .hover(|row| row.bg(theme::row_hover()).cursor_pointer())
                            .tooltip(move |window, cx| sql_tooltip(&hover_sql).build(window, cx))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.history_insert(&sql, window, cx)
                            }))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .text_xs()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .font_family("Menlo")
                                            .text_color(theme::text())
                                            .child(first_line(&entry.sql)),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .text_xs()
                                                    .overflow_hidden()
                                                    .whitespace_nowrap()
                                                    .text_color(if failed {
                                                        theme::danger()
                                                    } else {
                                                        theme::text_dim()
                                                    })
                                                    .child(left_meta),
                                            )
                                            .child(
                                                div()
                                                    .flex_none()
                                                    .text_xs()
                                                    .text_color(if failed {
                                                        theme::danger()
                                                    } else {
                                                        theme::text_dim()
                                                    })
                                                    .child(when),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .id(("save-history", index))
                                    .flex_none()
                                    .size(px(20.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(3.))
                                    .invisible()
                                    .group_hover("history-row", |button| button.visible())
                                    .child(
                                        svg()
                                            .path("icons/bookmark.svg")
                                            .size(px(11.))
                                            .text_color(theme::text_dim()),
                                    )
                                    .hover(|button| button.bg(theme::hover()).cursor_pointer())
                                    .tooltip(|window, cx| {
                                        gpui_component::tooltip::Tooltip::new("Save query")
                                            .build(window, cx)
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.history_save(&save_sql, cx);
                                    })),
                            )
                    })
                    .collect();
                div()
                    .children(rows)
                    .when(entries.is_empty(), |list| {
                        list.child(empty_line(if filter.is_empty() {
                            "Statements you run will appear here."
                        } else {
                            "No matches."
                        }))
                    })
                    .into_any_element()
            }
            HistoryTab::Saved => {
                let renaming_name = self.history_renaming.as_ref().map(|(name, _)| name.clone());
                let saved: Vec<&SavedQuery> = self
                    .preferences
                    .saved_queries
                    .iter()
                    .filter(|saved| {
                        filter.is_empty()
                            || saved.name.to_lowercase().contains(&filter)
                            || saved.sql.to_lowercase().contains(&filter)
                    })
                    .collect();
                let rows: Vec<_> = saved
                    .iter()
                    .enumerate()
                    .map(|(index, saved)| {
                        // A human-given name inserts with a provenance
                        // comment; the auto-derived default is not
                        // worth repeating above its own text.
                        let sql = if saved.name != default_name(&saved.sql) {
                            format!("-- Saved: {}\n{}", saved.name, saved.sql)
                        } else {
                            saved.sql.clone()
                        };
                        let name = saved.name.clone();
                        let hover_sql = saved.sql.clone();
                        let advise_sql = saved.sql.clone();
                        let delete_name = saved.name.clone();
                        let favorite_name = saved.name.clone();
                        let rename_name = saved.name.clone();
                        let favorite = saved.favorite;
                        // "saved N ago", using the same relative-time style
                        // as History; blank for queries saved before the
                        // timestamp existed.
                        let saved_meta =
                            (saved.saved_at > 0).then(|| relative_time(saved.saved_at));
                        let renaming = renaming_name.as_deref() == Some(saved.name.as_str());

                        if renaming {
                            let input = self
                                .history_renaming
                                .as_ref()
                                .map(|(_, input)| input.clone())
                                .expect("renaming row has an input");
                            return div()
                                .id(("saved-query", index))
                                .px_3()
                                .py_1p5()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().w(px(self.history_width - 130.0)).child(input))
                                .child(
                                    div()
                                        .id(("rename-commit", index))
                                        .flex_none()
                                        .px_2()
                                        .py_0p5()
                                        .rounded(px(3.))
                                        .bg(theme::selected())
                                        .text_xs()
                                        .text_color(theme::text())
                                        .child("Save")
                                        .hover(|button| button.cursor_pointer())
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.history_rename_commit(cx)
                                        })),
                                )
                                .child(
                                    div()
                                        .id(("rename-cancel", index))
                                        .flex_none()
                                        .px_1()
                                        .rounded(px(3.))
                                        .text_color(theme::text_dim())
                                        .child("\u{00d7}")
                                        .hover(|button| button.bg(theme::hover()).cursor_pointer())
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.history_renaming = None;
                                            cx.notify();
                                        })),
                                );
                        }

                        let action_button =
                            |id: (&'static str, usize),
                             icon: &'static str,
                             always_visible: bool,
                             color: gpui::Hsla| {
                                div()
                                    .id(id)
                                    .flex_none()
                                    .size(px(20.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(3.))
                                    .when(!always_visible, |button| {
                                        button
                                            .invisible()
                                            .group_hover("saved-row", |button| button.visible())
                                    })
                                    .child(svg().path(icon).size(px(11.)).text_color(color))
                            };

                        div()
                            .id(("saved-query", index))
                            .group("saved-row")
                            .px_3()
                            .py_1p5()
                            .flex()
                            .flex_col()
                            .hover(|row| row.bg(theme::row_hover()).cursor_pointer())
                            .tooltip(move |window, cx| sql_tooltip(&hover_sql).build(window, cx))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.history_insert(&sql, window, cx)
                            }))
                            .child(
                                div()
                                    .w_full()
                                    .text_sm()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_color(theme::text())
                                    .child(name),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .text_xs()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .font_family("Menlo")
                                    .text_color(theme::text_dim())
                                    .child(first_line(&saved.sql)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .pt_0p5()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .child(
                                                action_button(
                                                    ("favorite-saved", index),
                                                    "icons/star.svg",
                                                    favorite,
                                                    if favorite {
                                                        theme::sort_indicator()
                                                    } else {
                                                        theme::text_dim()
                                                    },
                                                )
                                                .hover(|button| {
                                                    button.bg(theme::hover()).cursor_pointer()
                                                })
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.history_toggle_favorite(
                                                        &favorite_name,
                                                        cx,
                                                    );
                                                })),
                                            )
                                            .child(
                                                action_button(
                                                    ("rename-saved", index),
                                                    "icons/edit.svg",
                                                    false,
                                                    theme::text_dim(),
                                                )
                                                .hover(|button| {
                                                    button.bg(theme::hover()).cursor_pointer()
                                                })
                                                .tooltip(|window, cx| {
                                                    gpui_component::tooltip::Tooltip::new("Rename")
                                                        .build(window, cx)
                                                })
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.history_renaming = Some((
                                                        rename_name.clone(),
                                                        Self::input(
                                                            rename_name.clone(),
                                                            "Name",
                                                            false,
                                                            cx,
                                                        ),
                                                    ));
                                                    cx.notify();
                                                })),
                                            )
                                            .child(
                                                action_button(
                                                    ("advise-saved", index),
                                                    "icons/advise.svg",
                                                    false,
                                                    theme::text_dim(),
                                                )
                                                .hover(|button| {
                                                    button.bg(theme::hover()).cursor_pointer()
                                                })
                                                .tooltip(|window, cx| {
                                                    gpui_component::tooltip::Tooltip::new(
                                                        "Run & advise on this query",
                                                    )
                                                    .build(window, cx)
                                                })
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        cx.stop_propagation();
                                                        this.advise_saved_query(
                                                            advise_sql.clone(),
                                                            window,
                                                            cx,
                                                        );
                                                    },
                                                )),
                                            )
                                            .child(
                                                action_button(
                                                    ("delete-saved", index),
                                                    "icons/trash.svg",
                                                    false,
                                                    theme::text_dim(),
                                                )
                                                .hover(|button| {
                                                    button
                                                        .bg(theme::danger_hover())
                                                        .cursor_pointer()
                                                })
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.history_delete_saved(&delete_name, cx);
                                                })),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_xs()
                                            .text_color(theme::text_dim())
                                            .when_some(saved_meta, |label, meta| label.child(meta)),
                                    ),
                            )
                    })
                    .collect();
                div()
                    .children(rows)
                    .when(saved.is_empty(), |list| {
                        list.child(empty_line(if filter.is_empty() {
                            "Bookmark a history entry to keep and sync it."
                        } else {
                            "No matches."
                        }))
                    })
                    .into_any_element()
            }
            HistoryTab::Tabs => {
                let renaming_id = self
                    .saved_tab_renaming
                    .as_ref()
                    .map(|(id, _, _)| id.clone());
                let saved: Vec<&zedb_core::SavedTab> = self
                    .saved_tabs
                    .iter()
                    .filter(|saved| {
                        filter.is_empty()
                            || saved.name.to_lowercase().contains(&filter)
                            || saved.sql.to_lowercase().contains(&filter)
                    })
                    .collect();
                let rows: Vec<_> = saved
                    .iter()
                    .enumerate()
                    .map(|(index, saved)| {
                        let saved_id = saved.id.clone();
                        let name = saved.name.clone();
                        let open_id = saved_id.clone();
                        let rename_id = saved_id.clone();
                        let rename_name = name.clone();
                        let delete_id = saved_id;
                        let preview = first_line(&saved.sql);
                        let hover_sql = saved.sql.clone();
                        let saved_meta =
                            (saved.saved_at > 0).then(|| relative_time(saved.saved_at));
                        let renaming = renaming_id.as_deref() == Some(saved.id.as_str());

                        if renaming {
                            let input = self
                                .saved_tab_renaming
                                .as_ref()
                                .map(|(_, _, input)| input.clone())
                                .expect("renaming saved tab has an input");
                            return div()
                                .id(("saved-tab", index))
                                .px_3()
                                .py_1p5()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().w(px(self.history_width - 130.0)).child(input))
                                .child(
                                    div()
                                        .id(("saved-tab-rename-commit", index))
                                        .flex_none()
                                        .px_2()
                                        .py_0p5()
                                        .rounded(px(3.))
                                        .bg(theme::selected())
                                        .text_xs()
                                        .text_color(theme::text())
                                        .child("Save")
                                        .hover(|button| button.cursor_pointer())
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.saved_tab_rename_commit(cx)
                                        })),
                                )
                                .child(
                                    div()
                                        .id(("saved-tab-rename-cancel", index))
                                        .flex_none()
                                        .px_1()
                                        .rounded(px(3.))
                                        .text_color(theme::text_dim())
                                        .child("\u{00d7}")
                                        .hover(|button| button.bg(theme::hover()).cursor_pointer())
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.saved_tab_renaming = None;
                                            cx.notify();
                                        })),
                                );
                        }

                        let action_button = |id: (&'static str, usize), icon: &'static str| {
                            div()
                                .id(id)
                                .flex_none()
                                .size(px(20.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(3.))
                                .invisible()
                                .group_hover("saved-tab-row", |button| button.visible())
                                .child(svg().path(icon).size(px(11.)).text_color(theme::text_dim()))
                        };

                        div()
                            .id(("saved-tab", index))
                            .group("saved-tab-row")
                            .px_3()
                            .py_1p5()
                            .flex()
                            .flex_col()
                            .hover(|row| row.bg(theme::row_hover()).cursor_pointer())
                            .tooltip(move |window, cx| sql_tooltip(&hover_sql).build(window, cx))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.saved_tab_open(&open_id, window, cx)
                            }))
                            .child(
                                div()
                                    .w_full()
                                    .text_sm()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_color(theme::text())
                                    .child(name),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .text_xs()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .font_family("Menlo")
                                    .text_color(theme::text_dim())
                                    .child(preview),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .pt_0p5()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .child(
                                                action_button(
                                                    ("rename-saved-tab", index),
                                                    "icons/edit.svg",
                                                )
                                                .hover(|button| {
                                                    button.bg(theme::hover()).cursor_pointer()
                                                })
                                                .tooltip(|window, cx| {
                                                    gpui_component::tooltip::Tooltip::new("Rename")
                                                        .build(window, cx)
                                                })
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.saved_tab_renaming = Some((
                                                        rename_id.clone(),
                                                        rename_name.clone(),
                                                        Self::input(
                                                            rename_name.clone(),
                                                            "Name",
                                                            false,
                                                            cx,
                                                        ),
                                                    ));
                                                    cx.notify();
                                                })),
                                            )
                                            .child(
                                                action_button(
                                                    ("delete-saved-tab", index),
                                                    "icons/trash.svg",
                                                )
                                                .hover(|button| {
                                                    button
                                                        .bg(theme::danger_hover())
                                                        .cursor_pointer()
                                                })
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.saved_tab_delete(&delete_id, cx);
                                                })),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_xs()
                                            .text_color(theme::text_dim())
                                            .when_some(saved_meta, |label, meta| label.child(meta)),
                                    ),
                            )
                    })
                    .collect();
                div()
                    .child(
                        div()
                            .px_3()
                            .py_2()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::text_dim())
                                    .child("Save the active tab with \u{2318}S"),
                            )
                            .child(
                                div()
                                    .id("save-current-tab")
                                    .px_2()
                                    .py_0p5()
                                    .rounded(px(3.))
                                    .border_1()
                                    .border_color(theme::border())
                                    .text_xs()
                                    .text_color(theme::text())
                                    .child("Save now")
                                    .hover(|button| button.bg(theme::hover()).cursor_pointer())
                                    .on_click(
                                        cx.listener(|this, _, _, cx| {
                                            this.save_active_query_tab(cx)
                                        }),
                                    ),
                            ),
                    )
                    .children(rows)
                    .when(saved.is_empty(), |list| {
                        list.child(empty_line(if filter.is_empty() {
                            "No saved tabs yet."
                        } else {
                            "No matches."
                        }))
                    })
                    .into_any_element()
            }
        };

        let footer = (active_tab == HistoryTab::History && !self.history.is_empty()).then(|| {
            let armed = self.history_clear_armed;
            let count = self.history.len();
            div()
                .flex_none()
                .h(px(26.))
                .px_3()
                .border_t_1()
                .border_color(theme::border())
                .flex()
                .items_center()
                .justify_end()
                .gap_2()
                .when(armed, |gutter| {
                    gutter.child(
                        div()
                            .id("history-clear-cancel")
                            .px_1p5()
                            .rounded(px(3.))
                            .text_xs()
                            .text_color(theme::text_dim())
                            .child("Cancel")
                            .hover(|button| {
                                button
                                    .bg(theme::hover())
                                    .text_color(theme::text())
                                    .cursor_pointer()
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.history_clear_armed = false;
                                cx.notify();
                            })),
                    )
                })
                .child(
                    div()
                        .id("history-clear")
                        .px_1p5()
                        .rounded(px(3.))
                        .text_xs()
                        .map(|button| {
                            if armed {
                                button
                                    .text_color(theme::danger())
                                    .child(format!("Clear {count} entries?"))
                            } else {
                                button.text_color(theme::text_dim()).child("Clear history")
                            }
                        })
                        .hover(|button| {
                            button
                                .bg(theme::danger_hover())
                                .text_color(theme::danger())
                                .cursor_pointer()
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.history_clear_armed {
                                this.history_clear(cx);
                            } else {
                                this.history_clear_armed = true;
                                cx.notify();
                            }
                        })),
                )
        });

        div()
            .w(px(self.history_width))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(theme::bg_sidebar())
            .child(header)
            .child(
                div()
                    .id("history-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(content),
            )
            .children(footer)
    }
}

type HighlightRuns = Vec<(std::ops::Range<usize>, HighlightStyle)>;

thread_local! {
    /// One compiled SQL highlighter for hover cards (compilation is
    /// ~10ms; parsing is microseconds) plus a per-text run cache so a
    /// visible tooltip costs nothing per frame.
    static HOVER_HL: RefCell<Option<gpui_component::highlighter::SyntaxHighlighter>> =
        const { RefCell::new(None) };
    static HOVER_CACHE: RefCell<HashMap<(String, bool), HighlightRuns>> =
        RefCell::new(HashMap::new());
}

fn hover_runs(sql: &str, cx: &gpui::App) -> HighlightRuns {
    let dark = theme::is_dark();
    let key = (sql.to_string(), dark);
    HOVER_CACHE.with(|cache| {
        if let Some(runs) = cache.borrow().get(&key) {
            return runs.clone();
        }
        let runs = HOVER_HL.with(|slot| {
            let mut slot = slot.borrow_mut();
            let highlighter = slot
                .get_or_insert_with(|| gpui_component::highlighter::SyntaxHighlighter::new("sql"));
            highlighter.replace_all(&gpui_component::Rope::from(sql));
            let highlight_theme = &gpui_component::Theme::global(cx).highlight_theme;
            highlighter.styles(&(0..sql.len()), highlight_theme)
        });
        let mut cache = cache.borrow_mut();
        if cache.len() > 500 {
            cache.clear();
        }
        cache.insert(key, runs.clone());
        runs
    })
}

/// Hover card with the full statement, tree-sitter colored, size-capped.
fn sql_tooltip(sql: &str) -> gpui_component::tooltip::Tooltip {
    const MAX_CHARS: usize = 4000;
    let mut text = sql.to_string();
    let mut truncated = false;
    if text.len() > MAX_CHARS {
        let mut cut = MAX_CHARS;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        truncated = true;
    }
    gpui_component::tooltip::Tooltip::element(move |_, cx| {
        let runs = hover_runs(&text, cx);
        div()
            .max_w(px(560.))
            .font_family("Menlo")
            .text_xs()
            .child(gpui::StyledText::new(text.clone()).with_highlights(runs))
            .when(truncated, |card| {
                card.child(div().text_color(theme::text_dim()).child("\u{2026}"))
            })
            .into_any_element()
    })
}

/// Favorites first, then by name.
fn sort_saved(saved: &mut [SavedQuery]) {
    saved.sort_by(|a, b| {
        b.favorite
            .cmp(&a.favorite)
            .then_with(|| a.name.cmp(&b.name))
    });
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

fn empty_line(message: &'static str) -> gpui::Div {
    div()
        .px_3()
        .py_2()
        .text_sm()
        .text_color(theme::text_dim())
        .child(message)
}

/// The default saved-query name: the first 25 characters of the
/// statement's first line.
fn default_name(sql: &str) -> String {
    let line = sql.lines().next().unwrap_or_default().trim();
    let mut name = line.to_string();
    if name.len() > 25 {
        let mut cut = 25;
        while !name.is_char_boundary(cut) {
            cut -= 1;
        }
        name.truncate(cut);
        name.push('\u{2026}');
    }
    if name.is_empty() {
        name = "unnamed".into();
    }
    name
}

fn first_line(sql: &str) -> String {
    let line = sql.lines().next().unwrap_or_default().trim();
    let mut line = line.to_string();
    if line.len() > 90 {
        let mut cut = 90;
        while !line.is_char_boundary(cut) {
            cut -= 1;
        }
        line.truncate(cut);
        line.push('\u{2026}');
    }
    line
}

fn relative_time(at: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0);
    let delta = (now - at).max(0);
    if delta < 60 {
        "just now".into()
    } else if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86_400 {
        format!("{}h ago", delta / 3600)
    } else {
        format!("{}d ago", delta / 86_400)
    }
}
