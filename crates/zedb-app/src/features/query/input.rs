use crate::*;

use gpui::prelude::*;
impl Workspace {
    pub(crate) fn run_query_action(
        &mut self,
        _: &RunQuery,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_query(window, cx);
    }

    pub(crate) fn run_selection_action(
        &mut self,
        _: &RunSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_selection(window, cx);
    }

    pub(crate) fn modalkit_key(keystroke: &Keystroke) -> Option<String> {
        if keystroke.modifiers.platform || keystroke.modifiers.function {
            return None;
        }

        let special = match keystroke.key.as_str() {
            "escape" => Some("Esc"),
            "enter" => Some("Enter"),
            "backspace" => Some("BS"),
            "delete" => Some("Del"),
            "tab" => Some("Tab"),
            "space" => Some("Space"),
            "left" => Some("Left"),
            "right" => Some("Right"),
            "up" => Some("Up"),
            "down" => Some("Down"),
            "home" => Some("Home"),
            "end" => Some("End"),
            "pageup" => Some("PageUp"),
            "pagedown" => Some("PageDown"),
            _ => None,
        };
        let key = special
            .map(str::to_string)
            .or_else(|| keystroke.key_char.clone())
            .unwrap_or_else(|| keystroke.key.clone());

        if keystroke.modifiers.control || keystroke.modifiers.alt || special.is_some() {
            let mut modifiers = String::new();
            if keystroke.modifiers.control {
                modifiers.push_str("C-");
            }
            if keystroke.modifiers.alt {
                modifiers.push_str("A-");
            }
            if keystroke.modifiers.shift && special.is_some() {
                modifiers.push_str("S-");
            }
            Some(format!("<{modifiers}{key}>"))
        } else {
            Some(key)
        }
    }

    pub(crate) fn vim_keystroke(
        &mut self,
        keystroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.preferences.vim_mode {
            return;
        }
        let Some(key) = Self::modalkit_key(keystroke) else {
            return;
        };
        // Reserved for RunSelection; must reach action dispatch instead of
        // modalkit (where it would decrement numbers in normal mode).
        if key == "<C-x>" {
            return;
        }
        let Some(tab) = self.query.tabs.get_mut(self.query.active_tab) else {
            return;
        };
        if !tab.editor.focus_handle(cx).is_focused(window) {
            return;
        }
        // While the completion popup is open, arrows, enter, and escape
        // belong to it: skip modalkit so the editor's own bindings route
        // these keys into the menu (navigate, confirm, dismiss).
        if !keystroke.modifiers.modified()
            && matches!(keystroke.key.as_str(), "up" | "down" | "enter" | "escape")
            && tab.editor.read(cx).completion_menu_open(cx)
        {
            return;
        }
        // In visual mode the editor cursor tracks the selection end, not the
        // Vim head, so feeding it back would drift the selection; in command
        // mode keystrokes edit the command line, not the buffer.
        if !matches!(
            tab.vim.mode(),
            modalkit::env::vim::VimMode::Visual
                | modalkit::env::vim::VimMode::Select
                | modalkit::env::vim::VimMode::Command
        ) {
            let cursor = tab.editor.read(cx).cursor_position();
            tab.vim
                .set_cursor(cursor.line as usize, cursor.character as usize);
        }
        let mut snapshot = match tab.vim.input(&key) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.notice = Some(format!("Vim input error: {error}"));
                cx.stop_propagation();
                cx.notify();
                return;
            }
        };
        tab.vim_command_line = snapshot.command_line.take();
        tab.vim_recording = snapshot.recording;
        let editor = tab.editor.clone();
        editor.update(cx, |state, cx| {
            let text_changed = state.value().as_ref() != snapshot.text;
            let old_utf16_len = state.value().encode_utf16().count();
            EntityInputHandler::replace_text_in_range(
                state,
                Some(0..old_utf16_len),
                &snapshot.text,
                window,
                cx,
            );
            state.set_cursor_position(
                Position::new(snapshot.line as u32, snapshot.column as u32),
                window,
                cx,
            );
            // set_cursor_position deliberately closes editor popovers. In
            // Vim Insert mode that happens after every modalkit edit, so
            // request completion again from the final cursor position.
            if text_changed && tab.vim.mode() == modalkit::env::vim::VimMode::Insert {
                state.retrigger_completion(window, cx);
            }
            if let Some(selection) = &snapshot.selection {
                let start = Self::utf16_offset(&snapshot.text, selection.start);
                let end = Self::utf16_offset(&snapshot.text, selection.end);
                if start < end {
                    let mut adjusted_range = None;
                    let selected = EntityInputHandler::text_for_range(
                        state,
                        start..end,
                        &mut adjusted_range,
                        window,
                        cx,
                    );
                    if let Some(selected) = selected.filter(|selected| !selected.is_empty()) {
                        // InputState has no public API for setting an arbitrary
                        // selection; a same-text IME replace positions one
                        // without changing the buffer.
                        EntityInputHandler::replace_and_mark_text_in_range(
                            state,
                            Some(start..end),
                            &selected,
                            Some(0..0),
                            window,
                            cx,
                        );
                        EntityInputHandler::unmark_text(state, window, cx);
                    }
                }
            }
        });
        if let Some(unsupported) = snapshot.unsupported {
            self.notice = Some(format!("Vim action not available here: {unsupported}"));
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(crate) fn utf16_offset(text: &str, (line, column): (usize, usize)) -> usize {
        let mut offset = 0;
        for (index, line_text) in text.split('\n').enumerate() {
            if index == line {
                return offset
                    + line_text
                        .chars()
                        .take(column)
                        .map(char::len_utf16)
                        .sum::<usize>();
            }
            offset += line_text.encode_utf16().count() + 1;
        }
        // `line` is past the last line (e.g. a line-wise selection ending at
        // the final line); the loop overcounts by one virtual newline.
        offset.saturating_sub(1)
    }

    /// A neutral, self-clearing status message (e.g. "Applied").
    pub(crate) fn flash_notice(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.notice = Some(message.into());
        self.notice_warning = false;
        self.notice_flash_id += 1;
        let flash_id = self.notice_flash_id;
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_secs(2)).await;
            this.update(cx, |this, cx| {
                if this.notice_flash_id == flash_id {
                    this.notice = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn flash_warning(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.notice = Some(message.into());
        self.notice_warning = true;
        self.notice_flash_id += 1;
        let flash_id = self.notice_flash_id;
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_secs(1)).await;
            this.update(cx, |this, cx| {
                if this.notice_flash_id == flash_id {
                    this.notice_warning = false;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Text the user has highlighted in the active editor, if any.
    pub(crate) fn selected_text(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let editor = self
            .query
            .tabs
            .get(self.query.active_tab)
            .map(|tab| tab.editor.clone())?;
        editor.update(cx, |editor, cx| {
            let selection = EntityInputHandler::selected_text_range(editor, false, window, cx);
            selection
                .filter(|selection| !selection.range.is_empty())
                .and_then(|selection| {
                    let mut adjusted_range = None;
                    EntityInputHandler::text_for_range(
                        editor,
                        selection.range,
                        &mut adjusted_range,
                        window,
                        cx,
                    )
                })
        })
    }
}
