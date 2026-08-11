use gpui::{App, Entity, EntityId, Global, Pixels, Point, WeakEntity};

use crate::text::TextViewState;

pub(crate) fn init(cx: &mut App) {
    cx.set_global(GlobalState::new());
}

impl Global for GlobalState {}

/// zeDB patch: a text selection that can span several sibling selectable
/// [`TextView`]s (e.g. the messages of an agent transcript). Both points
/// are in window coordinates so every view can hit-test its own
/// window-absolute glyph positions against the same band. See
/// docs/VENDOR-PATCHES.md.
#[derive(Clone, Copy)]
pub(crate) struct TextSelection {
    pub(crate) anchor: Point<Pixels>,
    pub(crate) active: Point<Pixels>,
    /// The [`TextViewState`] where the drag started; only that view drives
    /// move/up so the band updates even when the cursor leaves it.
    pub(crate) anchor_view: EntityId,
    pub(crate) selecting: bool,
}

pub(crate) struct GlobalState {
    pub(crate) text_view_state_stack: Vec<Entity<TextViewState>>,
    // zeDB patch: cross-view text selection shared by all selectable
    // TextViews painted this frame.
    text_selection: Option<TextSelection>,
    /// Selectable views in paint (top-to-bottom) order, so a cross-view
    /// copy can assemble the selected text in document order.
    selection_views: Vec<WeakEntity<TextViewState>>,
}

impl GlobalState {
    pub(crate) fn new() -> Self {
        Self {
            text_view_state_stack: Vec::new(),
            text_selection: None,
            selection_views: Vec::new(),
        }
    }

    pub(crate) fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    pub(crate) fn global_mut(cx: &mut App) -> &mut Self {
        cx.global_mut::<Self>()
    }

    pub(crate) fn text_view_state(&self) -> Option<&Entity<TextViewState>> {
        self.text_view_state_stack.last()
    }

    // --- zeDB patch: cross-view text selection ---

    pub(crate) fn text_selection(&self) -> Option<TextSelection> {
        self.text_selection
    }

    /// True when there is a non-empty (anchor != active) selection.
    pub(crate) fn has_text_selection(&self) -> bool {
        self.text_selection
            .map(|sel| sel.anchor != sel.active)
            .unwrap_or(false)
    }

    /// Begin a fresh collapsed selection, discarding the previous one and
    /// the view registry (rebuilt as views repaint).
    pub(crate) fn start_text_selection(&mut self, pos: Point<Pixels>, anchor_view: EntityId) {
        self.text_selection = Some(TextSelection {
            anchor: pos,
            active: pos,
            anchor_view,
            selecting: true,
        });
        self.selection_views.clear();
    }

    pub(crate) fn update_text_selection(&mut self, pos: Point<Pixels>) {
        if let Some(sel) = self.text_selection.as_mut() {
            sel.active = pos;
        }
    }

    pub(crate) fn end_text_selection(&mut self) {
        if let Some(sel) = self.text_selection.as_mut() {
            sel.selecting = false;
        }
    }

    pub(crate) fn clear_text_selection(&mut self) {
        self.text_selection = None;
        self.selection_views.clear();
    }

    /// Record a selectable view in paint order (deduplicated).
    pub(crate) fn register_selection_view(&mut self, view: WeakEntity<TextViewState>) {
        if self
            .selection_views
            .iter()
            .any(|existing| existing.entity_id() == view.entity_id())
        {
            return;
        }
        self.selection_views.push(view);
    }

    pub(crate) fn selection_views(&self) -> &[WeakEntity<TextViewState>] {
        &self.selection_views
    }
}
