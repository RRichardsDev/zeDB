use gpui::{Context, Point, Window};

use crate::input::{
    InputState, MoveDown, MoveEnd, MoveHome, MoveLeft, MovePageDown, MovePageUp, MoveRight,
    MoveToEnd, MoveToNextWord, MoveToPreviousWord, MoveToStart, MoveUp, RopeExt as _,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MoveDirection {
    Up,
    Down,
}

impl InputState {
    /// Called after moving the cursor. Updates preferred_column if we know where the cursor now is.
    pub(super) fn update_preferred_column(&mut self) {
        let Some(last_layout) = &self.last_layout else {
            self.preferred_column = None;
            return;
        };

        let point = self.text.offset_to_point(self.cursor());
        let row = point.row.saturating_sub(last_layout.visible_range.start);
        let Some(line) = last_layout.lines.get(row) else {
            self.preferred_column = None;
            return;
        };

        let Some(pos) = line.position_for_index(point.column, last_layout.line_height) else {
            self.preferred_column = None;
            return;
        };

        self.preferred_column = Some((pos.x, point.column));
    }

    /// Move the cursor to the given offset.
    ///
    /// The offset is the UTF-8 offset.
    ///
    /// Ensure the offset use self.next_boundary or self.previous_boundary to get the correct offset.
    pub(crate) fn move_to(
        &mut self,
        offset: usize,
        direction: Option<MoveDirection>,
        cx: &mut Context<Self>,
    ) {
        let offset = offset.clamp(0, self.text.len());
        self.selected_range = (offset..offset).into();
        self.scroll_to(offset, direction, cx);
        self.pause_blink_cursor(cx);
        self.update_preferred_column();
        self.hide_context_menu(cx);
        self.clear_inline_completion(cx);
        cx.notify()
    }

    /// Move the cursor vertically by one line (up or down) while preserving the column if possible.
    ///
    /// move_lines: Number of lines to move vertically (positive for down, negative for up).
    pub(super) fn move_vertical(
        &mut self,
        move_lines: isize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.mode.is_single_line() {
            return;
        }
        let Some(last_layout) = &self.last_layout else {
            return;
        };

        let offset = self.cursor();
        let was_preferred_column = self.preferred_column;

        let mut display_point = self.text_wrapper.offset_to_display_point(offset);
        display_point.row = display_point.row.saturating_add_signed(move_lines);
        display_point.column = 0;
        let mut new_offset = self.text_wrapper.display_point_to_offset(display_point);

        if let Some((preferred_x, column)) = was_preferred_column {
            // Get display point again to update local_row.
            let mut next_display_point = self.text_wrapper.offset_to_display_point(new_offset);
            next_display_point.column = 0;
            let next_point = self.text_wrapper.display_point_to_point(next_display_point);
            let line_start_offset = self.text.line_start_offset(next_point.row);

            // If in visible range, prefer to use position to get column.
            if let Some(line) = last_layout.line(next_point.row) {
                if let Some(x) = line.closest_index_for_position(
                    Point {
                        x: preferred_x,
                        y: next_display_point.local_row * last_layout.line_height,
                    },
                    last_layout.line_height,
                ) {
                    new_offset = line_start_offset + x;
                }
            } else {
                // Not in visible range, use column directly.
                let max_line_len = self.text.slice_line(next_point.row).len();
                new_offset = line_start_offset + column.min(max_line_len);
            }
        }

        self.pause_blink_cursor(cx);
        let direction = if move_lines < 0 {
            MoveDirection::Up
        } else {
            MoveDirection::Down
        };
        self.move_to(new_offset, Some(direction), cx);
        // Set back the preferred_column
        self.preferred_column = was_preferred_column;
        cx.notify();
    }

    pub(super) fn left(&mut self, _: &MoveLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.pause_blink_cursor(cx);
        // zeDB patch (multi-cursor): a plain arrow drops the highlight
        // and collapses every selection to a bare cursor at its start;
        // once already collapsed, it moves every cursor left by one,
        // keeping the multi-cursor set alive throughout.
        if self.is_multi_selection() {
            if self.is_multi_collapsed() {
                self.move_multi_cursors(false, cx);
            } else {
                self.collapse_multi_to_edge(false, cx);
            }
            return;
        }
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor()), None, cx);
        } else {
            self.move_to(self.selected_range.start, None, cx)
        }
    }

    pub(super) fn right(&mut self, _: &MoveRight, _: &mut Window, cx: &mut Context<Self>) {
        self.pause_blink_cursor(cx);
        // zeDB patch (multi-cursor): collapse every selection to a bare
        // cursor at its end; once already collapsed, move every cursor
        // right by one, keeping the multi-cursor set alive.
        if self.is_multi_selection() {
            if self.is_multi_collapsed() {
                self.move_multi_cursors(true, cx);
            } else {
                self.collapse_multi_to_edge(true, cx);
            }
            return;
        }
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), None, cx);
        } else {
            self.move_to(self.selected_range.end, None, cx)
        }
    }

    /// zeDB patch (multi-cursor): collapse the whole selection set to
    /// bare cursors. `to_end` picks each selection's end (Right arrow);
    /// otherwise its start (Left arrow). The primary stays primary and
    /// the extras stay extras, so the multi-cursor survives; typing
    /// then inserts at every cursor. The reversed flag is cleared so
    /// the now-empty primary has no stale direction.
    /// zeDB patch (multi-cursor): true when every cursor in the set
    /// (primary and extras) is already a bare, empty cursor.
    fn is_multi_collapsed(&self) -> bool {
        self.selected_range.is_empty() && self.extra_selections.iter().all(|s| s.is_empty())
    }

    /// zeDB patch (multi-cursor): move every bare cursor by one grapheme
    /// boundary, `forward` right or left, keeping the multi-cursor set.
    /// Cursors that collide (or land on the primary) after the move are
    /// deduped so a later edit doesn't fire twice at one spot.
    fn move_multi_cursors(&mut self, forward: bool, cx: &mut Context<Self>) {
        let boundary = |this: &Self, off: usize| {
            if forward {
                this.next_boundary(off)
            } else {
                this.previous_boundary(off)
            }
        };
        let primary = boundary(self, self.cursor());
        let moved: Vec<usize> = self
            .extra_selections
            .iter()
            .map(|s| boundary(self, s.start))
            .collect();

        let mut seen = std::collections::HashSet::new();
        seen.insert(primary);
        self.extra_selections.clear();
        for at in moved {
            if seen.insert(at) {
                self.extra_selections.push((at..at).into());
            }
        }
        self.extra_selections.sort_by_key(|s| s.start);

        self.selected_range = (primary..primary).into();
        self.selection_reversed = false;
        self.scroll_to(primary, None, cx);
        self.pause_blink_cursor(cx);
        self.update_preferred_column();
        self.hide_context_menu(cx);
        self.clear_inline_completion(cx);
        cx.notify();
    }

    fn collapse_multi_to_edge(&mut self, to_end: bool, cx: &mut Context<Self>) {
        let edge = |s: &crate::input::cursor::Selection| if to_end { s.end } else { s.start };
        let primary = edge(&self.selected_range);
        for selection in &mut self.extra_selections {
            let at = edge(selection);
            selection.start = at;
            selection.end = at;
        }
        self.selected_range = (primary..primary).into();
        self.selection_reversed = false;
        self.scroll_to(primary, None, cx);
        self.pause_blink_cursor(cx);
        self.update_preferred_column();
        self.hide_context_menu(cx);
        self.clear_inline_completion(cx);
        cx.notify();
    }

    pub(super) fn up(&mut self, action: &MoveUp, window: &mut Window, cx: &mut Context<Self>) {
        if self.handle_action_for_context_menu(Box::new(action.clone()), window, cx) {
            return;
        }

        if self.mode.is_single_line() {
            return;
        }

        if !self.selected_range.is_empty() {
            self.move_to(
                self.previous_boundary(self.selected_range.start.saturating_sub(1)),
                Some(MoveDirection::Up),
                cx,
            );
        }
        self.pause_blink_cursor(cx);
        self.move_vertical(-1, window, cx);
    }

    pub(super) fn down(&mut self, action: &MoveDown, window: &mut Window, cx: &mut Context<Self>) {
        if self.handle_action_for_context_menu(Box::new(action.clone()), window, cx) {
            return;
        }

        if self.mode.is_single_line() {
            return;
        }

        if !self.selected_range.is_empty() {
            self.move_to(
                self.next_boundary(self.selected_range.end.saturating_sub(1)),
                Some(MoveDirection::Down),
                cx,
            );
        }

        self.pause_blink_cursor(cx);
        self.move_vertical(1, window, cx);
    }

    pub(super) fn page_up(&mut self, _: &MovePageUp, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode.is_single_line() {
            return;
        }

        let Some(last_layout) = &self.last_layout else {
            return;
        };

        let display_lines = (self.input_bounds.size.height / last_layout.line_height) as isize;
        self.move_vertical(-display_lines, window, cx);
    }

    pub(super) fn page_down(
        &mut self,
        _: &MovePageDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.mode.is_single_line() {
            return;
        }

        let Some(last_layout) = &self.last_layout else {
            return;
        };

        let display_lines = (self.input_bounds.size.height / last_layout.line_height) as isize;
        self.move_vertical(display_lines, window, cx);
    }

    pub(super) fn home(&mut self, _: &MoveHome, _: &mut Window, cx: &mut Context<Self>) {
        self.pause_blink_cursor(cx);
        let offset = self.start_of_line();
        self.move_to(offset, Some(MoveDirection::Up), cx);
    }

    pub(super) fn end(&mut self, _: &MoveEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.pause_blink_cursor(cx);
        let offset = self.end_of_line();
        self.move_to(offset, Some(MoveDirection::Down), cx);
    }

    pub(super) fn move_to_start(
        &mut self,
        _: &MoveToStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_to(0, None, cx);
    }

    pub(super) fn move_to_end(&mut self, _: &MoveToEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.text.len(), None, cx);
    }

    pub(super) fn move_to_previous_word(
        &mut self,
        _: &MoveToPreviousWord,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = self.previous_start_of_word();
        self.move_to(offset, None, cx);
    }

    pub(super) fn move_to_next_word(
        &mut self,
        _: &MoveToNextWord,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = self.next_end_of_word();
        self.move_to(offset, None, cx);
    }
}
