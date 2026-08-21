//! The command palette (cmd-shift-p): type to filter, arrows to move,
//! Enter to run. Commands are a fixed list gated on current state, so
//! the palette always tells the truth about what zeDB can do right now.

use gpui::{div, prelude::*, px, Context, Entity, Focusable, Window};

use crate::components::text_input::TextInput;
use crate::theme;
use crate::Workspace;

pub struct PaletteState {
    pub open: bool,
    pub input: Entity<TextInput>,
    pub selected: usize,
    /// Where focus was when the palette opened; restored on close so
    /// key dispatch (and menu key equivalents) keep a live path.
    pub previous_focus: Option<gpui::FocusHandle>,
}

#[cfg(test)]
#[path = "command_palette/gpui_tests.rs"]
mod gpui_tests;

impl PaletteState {
    pub fn new(cx: &mut Context<Workspace>) -> Self {
        let input = cx.new(|cx| TextInput::new("", "Type a command...", false, cx));
        cx.observe(&input, |_, _, cx| cx.notify()).detach();
        Self {
            open: false,
            input,
            selected: 0,
            previous_focus: None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PaletteCommand {
    OpenSettingsFile,
    OpenPreferences,
    NewQuery,
    ToggleFleet,
    ToggleAgentPane,
    ToggleVimMode,
    SyncSettingsNow,
    Disconnect,
    CheckForUpdates,
    ThemeDark,
    ThemeLight,
    ThemeSystem,
    ToggleOps,
    ToggleQueryHistory,
    ExplainQuery,
    EstimateQuery,
    ExportResults,
    LinkCloud,
}

const ALL_COMMANDS: &[PaletteCommand] = &[
    PaletteCommand::NewQuery,
    PaletteCommand::OpenPreferences,
    PaletteCommand::OpenSettingsFile,
    PaletteCommand::ToggleFleet,
    PaletteCommand::ToggleAgentPane,
    PaletteCommand::ToggleVimMode,
    PaletteCommand::SyncSettingsNow,
    PaletteCommand::Disconnect,
    PaletteCommand::CheckForUpdates,
    PaletteCommand::ThemeDark,
    PaletteCommand::ThemeLight,
    PaletteCommand::ThemeSystem,
    PaletteCommand::ToggleOps,
    PaletteCommand::ToggleQueryHistory,
    PaletteCommand::ExplainQuery,
    PaletteCommand::EstimateQuery,
    PaletteCommand::ExportResults,
    PaletteCommand::LinkCloud,
];

impl PaletteCommand {
    fn label(self) -> &'static str {
        match self {
            Self::OpenSettingsFile => "Open settings.json",
            Self::OpenPreferences => "Preferences",
            Self::NewQuery => "New query tab",
            Self::ToggleFleet => "Toggle fleet view",
            Self::ToggleAgentPane => "Toggle agent pane",
            Self::ToggleVimMode => "Toggle vim mode",
            Self::SyncSettingsNow => "Sync settings now",
            Self::Disconnect => "Disconnect",
            Self::CheckForUpdates => "Check for updates",
            Self::ThemeDark => "Theme: Dark",
            Self::ThemeLight => "Theme: Light",
            Self::ThemeSystem => "Theme: System",
            Self::ToggleOps => "Ops view",
            Self::ToggleQueryHistory => "Query history and saved queries",
            Self::ExplainQuery => "Explain query (plan and index pruning)",
            Self::EstimateQuery => "Estimate query cost (parts, rows, marks)",
            Self::LinkCloud => "Link ClickHouse Cloud",
            Self::ExportResults => "Export current query results",
        }
    }

    fn available(self, workspace: &Workspace) -> bool {
        match self {
            Self::SyncSettingsNow => workspace.preferences.settings_sync_url.is_some(),
            Self::Disconnect
            | Self::NewQuery
            | Self::ToggleFleet
            | Self::ToggleOps
            | Self::ExplainQuery
            | Self::EstimateQuery => workspace.connection.connected.is_some(),
            Self::ExportResults => workspace.export_available(),
            _ => true,
        }
    }

    fn run(self, workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
        match self {
            Self::OpenSettingsFile => workspace.open_settings_file(cx),
            Self::OpenPreferences => workspace.open_preferences(cx),
            Self::NewQuery => workspace.open_query_editor(cx),
            Self::ToggleFleet => workspace.toggle_fleet(cx),
            Self::ToggleAgentPane => workspace.agent_toggle(window, cx),
            Self::ToggleVimMode => workspace.toggle_vim_mode(cx),
            Self::SyncSettingsNow => workspace.settings_sync_tick(cx),
            Self::Disconnect => workspace.disconnect(cx),
            Self::CheckForUpdates => workspace.check_for_updates_now(cx),
            Self::ThemeDark => workspace.set_theme_preference("dark", window, cx),
            Self::ThemeLight => workspace.set_theme_preference("light", window, cx),
            Self::ThemeSystem => workspace.set_theme_preference("system", window, cx),
            Self::ToggleOps => workspace.ops_toggle(cx),
            Self::ToggleQueryHistory => workspace.history_toggle(cx),
            Self::ExplainQuery => workspace.explain_query(window, cx),
            Self::EstimateQuery => workspace.estimate_query(window, cx),
            Self::ExportResults => workspace.export_open(cx),
            Self::LinkCloud => workspace.cloud_open(cx),
        }
    }
}

impl Workspace {
    /// The settings file in the user's editor of choice (whatever
    /// macOS associates with .json). Written first if it never was.
    pub(crate) fn open_settings_file(&mut self, cx: &mut Context<Self>) {
        let path = match zedb_core::settings_file_path() {
            Ok(path) => path,
            Err(error) => {
                self.flash_warning(format!("Could not locate settings: {error}"), cx);
                return;
            }
        };
        if !path.exists() {
            if let Err(error) = zedb_core::save_preferences(&self.preferences) {
                self.flash_warning(format!("Could not write settings: {error}"), cx);
                return;
            }
        }
        let opened = std::process::Command::new("open").arg(&path).status();
        if !matches!(opened, Ok(status) if status.success()) {
            self.flash_warning(format!("Could not open {}", path.display()), cx);
        }
    }

    pub(crate) fn palette_toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.open {
            self.palette_close(window, cx);
            return;
        }
        self.palette.previous_focus = window.focused(cx);
        self.palette.open = true;
        self.palette.selected = 0;
        let input = self.palette.input.clone();
        input.update(cx, |input, cx| input.set_text("", cx));
        window.focus(&input.read(cx).focus_handle(cx));
        cx.notify();
    }

    /// The preference a palette command previews, if any.
    fn palette_theme_target(command: PaletteCommand) -> Option<&'static str> {
        match command {
            PaletteCommand::ThemeDark => Some("dark"),
            PaletteCommand::ThemeLight => Some("light"),
            PaletteCommand::ThemeSystem => Some("system"),
            _ => None,
        }
    }

    /// Selecting a theme command previews it immediately; moving off it
    /// (or closing the palette) falls back to the saved preference.
    /// Nothing is persisted here.
    pub(crate) fn palette_theme_preview(&mut self, cx: &mut Context<Self>) {
        let commands = self.palette_filtered(cx);
        let selected = self.palette.selected.min(commands.len().saturating_sub(1));
        let target = commands
            .get(selected)
            .copied()
            .and_then(Self::palette_theme_target);
        let preference = target
            .map(str::to_string)
            .or_else(|| self.preferences.theme.clone());
        let mode = crate::resolve_theme_mode(preference.as_deref(), cx);
        if mode.is_dark() != gpui_component::Theme::global(cx).is_dark() {
            crate::apply_theme_preference(preference.as_deref(), None, cx);
            cx.notify();
        }
    }

    pub(crate) fn palette_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette.open = false;
        // A previewed theme that was never chosen reverts.
        let preference = self.preferences.theme.clone();
        let mode = crate::resolve_theme_mode(preference.as_deref(), cx);
        if mode.is_dark() != gpui_component::Theme::global(cx).is_dark() {
            crate::apply_theme_preference(preference.as_deref(), None, cx);
        }
        // Focus must land somewhere live, or the window loses its key
        // dispatch path and every shortcut goes dead. Never focus a
        // maybe-unrendered element; no focus at all is a working state.
        match self.palette.previous_focus.take() {
            Some(previous) => window.focus(&previous),
            None => window.blur(),
        }
        cx.notify();
    }

    pub(crate) fn palette_filtered(&self, cx: &Context<Self>) -> Vec<PaletteCommand> {
        let query = self.palette.input.read(cx).text().trim().to_lowercase();
        ALL_COMMANDS
            .iter()
            .copied()
            .filter(|command| command.available(self))
            .filter(|command| query.is_empty() || command.label().to_lowercase().contains(&query))
            .collect()
    }

    pub(crate) fn palette_move(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.palette_filtered(cx).len();
        if count == 0 {
            return;
        }
        let current = self.palette.selected.min(count - 1) as isize;
        self.palette.selected = (current + delta).rem_euclid(count as isize) as usize;
        self.palette_theme_preview(cx);
        cx.notify();
    }

    pub(crate) fn palette_run_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let commands = self.palette_filtered(cx);
        let Some(command) = commands
            .get(self.palette.selected.min(commands.len().saturating_sub(1)))
            .copied()
        else {
            return;
        };
        self.palette_close(window, cx);
        command.run(self, window, cx);
    }

    pub(crate) fn command_palette_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let commands = self.palette_filtered(cx);
        let selected = self.palette.selected.min(commands.len().saturating_sub(1));
        gpui::deferred(
            div()
                .absolute()
                .inset_0()
                .child(
                    // Backdrop: a click anywhere outside dismisses.
                    div()
                        .id("palette-backdrop")
                        .absolute()
                        .inset_0()
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(|this, _, window, cx| this.palette_close(window, cx)),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(120.))
                        .left_0()
                        .right_0()
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .w(px(520.))
                                .max_h(px(420.))
                                .flex()
                                .flex_col()
                                .rounded(px(6.))
                                .border_1()
                                .border_color(theme::border())
                                .bg(theme::bg())
                                .shadow_lg()
                                .on_mouse_down(
                                    gpui::MouseButton::Left,
                                    cx.listener(|_, _, _, cx| cx.stop_propagation()),
                                )
                                .child(
                                    div()
                                        .p_2()
                                        .border_b_1()
                                        .border_color(theme::border())
                                        .child(self.palette.input.clone()),
                                )
                                .child(div().flex_1().min_h_0().overflow_hidden().children(
                                    commands.iter().enumerate().map(|(index, command)| {
                                        let command = *command;
                                        div()
                                            .id(command.label())
                                            .px_3()
                                            .py_1p5()
                                            .flex()
                                            .items_center()
                                            .text_color(theme::text())
                                            .when(index == selected, |row| {
                                                row.bg(theme::selected())
                                            })
                                            .hover(|row| {
                                                row.bg(theme::bg_sidebar()).cursor_pointer()
                                            })
                                            .on_hover(cx.listener(
                                                move |this, hovered: &bool, _, cx| {
                                                    if *hovered && this.palette.selected != index {
                                                        this.palette.selected = index;
                                                        this.palette_theme_preview(cx);
                                                        cx.notify();
                                                    }
                                                },
                                            ))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.palette_close(window, cx);
                                                command.run(this, window, cx);
                                            }))
                                            .child(command.label())
                                    }),
                                ))
                                .when(commands.is_empty(), |panel| {
                                    panel.child(
                                        div()
                                            .px_3()
                                            .py_2()
                                            .text_sm()
                                            .text_color(theme::text_dim())
                                            .child("No matching commands"),
                                    )
                                }),
                        ),
                ),
        )
    }
}
