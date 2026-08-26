use std::time::Duration;

use anyhow::Result;
use gpui::{App, Context, Task, Window};
use ropey::Rope;

use crate::input::{popovers::HoverPopover, InputState, RopeExt};

/// Hover provider
///
/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_hover
pub trait HoverProvider {
    /// textDocument/hover
    ///
    /// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_hover
    fn hover(
        &self,
        _text: &Rope,
        _offset: usize,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Task<Result<Option<lsp_types::Hover>>>;
}

impl InputState {
    /// Handle hover trigger LSP request.
    pub(super) fn handle_hover_popover(
        &mut self,
        offset: usize,
        window: &mut Window,
        cx: &mut Context<InputState>,
    ) {
        if self.selecting {
            return;
        }

        let Some(provider) = self.lsp.hover_provider.clone() else {
            return;
        };

        if let Some(hover_popover) = self.hover_popover.as_ref() {
            let hover_popover = hover_popover.read(cx);
            if hover_popover.is_same(offset) {
                return;
            }
            // zeDB patch (hoverable link cards): the pointer is inside
            // a card that carries links; leaving the symbol to reach
            // them must not dismiss it. Moving off the card resumes
            // the normal vanish-on-move behavior.
            if hover_popover.keep_open() {
                return;
            }
        }

        // Currently not implemented.
        let task = provider.hover(&self.text, offset, window, cx);
        let mut symbol_range = self.text.word_range(offset).unwrap_or(offset..offset);
        let editor = cx.entity();
        let should_delay = self.hover_popover.is_none();
        self.lsp._hover_task = cx.spawn_in(window, async move |_, cx| {
            if should_delay {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
            }

            let result = task.await?;

            _ = editor.update(cx, |editor, cx| {
                // zeDB patch (hoverable link cards): the pointer moved
                // onto the card while this lookup was in flight;
                // neither replace nor dismiss under the reader.
                if editor
                    .hover_popover
                    .as_ref()
                    .is_some_and(|popover| popover.read(cx).keep_open())
                {
                    return;
                }
                match result {
                    Some(hover) => {
                        if let Some(range) = hover.range {
                            let start = editor.text.position_to_offset(&range.start);
                            let end = editor.text.position_to_offset(&range.end);
                            symbol_range = start..end;
                        }
                        let hover_popover =
                            HoverPopover::new(cx.entity(), symbol_range, &hover, cx);
                        editor.hover_popover = Some(hover_popover);
                    }
                    None => {
                        editor.hover_popover = None;
                    }
                }
            });

            Ok(())
        });
    }
}
