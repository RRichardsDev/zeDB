//! Query history + saved queries (docs/PHASE-7.1-IDEAS.md, first
//! bite): every statement zeDB runs is recorded locally per
//! connection; named snippets live in settings.json and sync. The
//! drawer docks to the right of the query editor, split into History
//! and Saved tabs. Bookmarking saves immediately under a name derived
//! from the query; renaming happens inline on the Saved tab.

use zedb_core::HistoryEntry;

use crate::TextInput;

/// Rows shown per section before search narrows things down.
pub(crate) const HISTORY_SHOWN: usize = 200;

pub(crate) struct HistoryState {
    pub(crate) open: bool,
    pub(crate) entries: Vec<HistoryEntry>,
    pub(crate) search: gpui::Entity<TextInput>,
    pub(crate) renaming: Option<(String, gpui::Entity<TextInput>)>,
    pub(crate) saved_tab_renaming: Option<(String, String, gpui::Entity<TextInput>)>,
    pub(crate) saved_tabs: Vec<zedb_core::SavedTab>,
    pub(crate) clear_armed: bool,
    pub(crate) width: f32,
    pub(crate) resizing: Option<(f32, f32)>,
    pub(crate) tab: HistoryTab,
}

impl HistoryState {
    pub(crate) fn new(
        entries: Vec<HistoryEntry>,
        saved_tabs: Vec<zedb_core::SavedTab>,
        search: gpui::Entity<TextInput>,
    ) -> Self {
        Self {
            open: false,
            entries,
            search,
            renaming: None,
            saved_tab_renaming: None,
            saved_tabs,
            clear_armed: false,
            width: 320.0,
            resizing: None,
            tab: HistoryTab::default(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum HistoryTab {
    #[default]
    History,
    Saved,
    Tabs,
}

impl HistoryTab {
    pub(crate) const ALL: [HistoryTab; 3] =
        [HistoryTab::History, HistoryTab::Saved, HistoryTab::Tabs];

    pub(crate) fn label(self) -> &'static str {
        match self {
            HistoryTab::History => "History",
            HistoryTab::Saved => "Saved",
            HistoryTab::Tabs => "Tabs",
        }
    }
}
