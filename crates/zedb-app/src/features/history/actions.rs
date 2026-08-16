use gpui::{Context, Window};
use zedb_core::{HistoryEntry, SavedQuery};

use super::model::HistoryTab;
use super::view::{default_name, sort_saved, unix_now};
use crate::*;

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
        let mut updated = self.history.saved_tabs.clone();
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
        self.history.saved_tabs = updated;
        self.query.tabs[self.query.active_tab].saved_tab_id = Some(saved_id);
        self.history.tab = HistoryTab::Tabs;
        self.history.open = true;
        self.history.clear_armed = false;
        if self.connection.connected.is_some() {
            self.show_query_editor = true;
            self.show_ops = false;
            self.show_fleet = false;
        }
        self.flash_notice(format!("Saved tab {name}"), cx);
    }

    pub(crate) fn saved_tab_rename_commit(&mut self, cx: &mut Context<Self>) {
        let Some((id, original, input)) = self.history.saved_tab_renaming.take() else {
            return;
        };
        let new_name = input.read(cx).text().trim().to_string();
        if new_name.is_empty() || new_name == original {
            cx.notify();
            return;
        }
        let mut updated = self.history.saved_tabs.clone();
        if let Some(saved) = updated.iter_mut().find(|saved| saved.id == id) {
            saved.name = new_name.clone();
        }
        if let Err(error) = zedb_core::save_saved_tabs(&updated) {
            self.flash_warning(format!("Could not rename saved tab: {error}"), cx);
            return;
        }
        self.history.saved_tabs = updated;
        for tab in &mut self.query.tabs {
            if tab.saved_tab_id.as_deref() == Some(id.as_str()) {
                tab.name = new_name.clone();
            }
        }
        cx.notify();
    }

    pub(crate) fn saved_tab_delete(&mut self, id: &str, cx: &mut Context<Self>) {
        let mut updated = self.history.saved_tabs.clone();
        updated.retain(|saved| saved.id != id);
        if let Err(error) = zedb_core::save_saved_tabs(&updated) {
            self.flash_warning(format!("Could not delete saved tab: {error}"), cx);
            return;
        }
        self.history.saved_tabs = updated;
        for tab in &mut self.query.tabs {
            if tab.saved_tab_id.as_deref() == Some(id) {
                tab.saved_tab_id = None;
            }
        }
        cx.notify();
    }

    pub(crate) fn saved_tab_open(
        &mut self,
        saved_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(saved) = self
            .history
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
        tab.connection = self.active_connection_name();
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
        self.history.open = !self.history.open;
        // The drawer lives beside the editor; surface it.
        if self.history.open && self.connection.connected.is_some() && !self.show_query_editor {
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
                &mut self.history.entries,
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
        let _ = zedb_core::save_history(&self.history.entries);
    }

    pub(crate) fn history_insert(
        &mut self,
        sql: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
    pub(crate) fn history_save(&mut self, sql: &str, cx: &mut Context<Self>) {
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

    pub(crate) fn history_rename_commit(&mut self, cx: &mut Context<Self>) {
        let Some((original, input)) = self.history.renaming.take() else {
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

    pub(crate) fn history_clear(&mut self, cx: &mut Context<Self>) {
        self.history.entries.clear();
        let _ = zedb_core::save_history(&self.history.entries);
        self.history.clear_armed = false;
        cx.notify();
    }

    pub(crate) fn history_delete_saved(&mut self, name: &str, cx: &mut Context<Self>) {
        self.preferences
            .saved_queries
            .retain(|saved| saved.name != name);
        let _ = zedb_core::save_preferences(&self.preferences);
        self.settings_sync_tick(cx);
        cx.notify();
    }

    pub(crate) fn history_toggle_favorite(&mut self, name: &str, cx: &mut Context<Self>) {
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
}
