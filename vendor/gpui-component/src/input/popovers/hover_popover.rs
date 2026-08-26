use std::{ops::Range, rc::Rc};

use gpui::{
    deferred, div, point, prelude::FluentBuilder as _, px, AnyElement, App, AppContext as _,
    AvailableSpace, Bounds, Element, ElementId, Entity, InteractiveElement, IntoElement,
    MouseDownEvent, ParentElement as _, Pixels, Render, StatefulInteractiveElement as _,
    StyleRefinement, Styled, Window,
};

use crate::{
    input::{popovers::render_markdown, InputState},
    StyledExt,
};

pub struct HoverPopover {
    editor: Entity<InputState>,
    /// The symbol range byte of the hover trigger.
    pub(crate) symbol_range: Range<usize>,
    pub(crate) hover: Rc<lsp_types::Hover>,
    /// zeDB patch (hoverable link cards): whether the contents carry a
    /// markdown link, and whether the pointer is currently inside the
    /// card. A link-bearing card is kept open while hovered so its
    /// links are reachable; link-free cards keep the lighter
    /// vanish-on-move behavior.
    has_link: bool,
    hovered: Rc<std::cell::Cell<bool>>,
}

impl HoverPopover {
    pub fn new(
        editor: Entity<InputState>,
        symbol_range: Range<usize>,
        hover: &lsp_types::Hover,
        cx: &mut App,
    ) -> Entity<Self> {
        let hover = Rc::new(hover.clone());
        let has_link = match &hover.contents {
            lsp_types::HoverContents::Markup(markup) => markup.value.contains("]("),
            lsp_types::HoverContents::Scalar(lsp_types::MarkedString::String(text)) => {
                text.contains("](")
            }
            _ => false,
        };

        cx.new(|_| Self {
            editor,
            symbol_range,
            hover,
            has_link,
            hovered: Rc::new(std::cell::Cell::new(false)),
        })
    }

    pub(crate) fn is_same(&self, offset: usize) -> bool {
        self.symbol_range.contains(&offset)
    }

    /// zeDB patch (hoverable link cards): the card must survive this
    /// mouse move because the pointer is inside it reading/clicking.
    pub(crate) fn keep_open(&self) -> bool {
        self.has_link && self.hovered.get()
    }
}

impl Render for HoverPopover {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        let contents = match self.hover.contents.clone() {
            lsp_types::HoverContents::Scalar(scalar) => match scalar {
                lsp_types::MarkedString::String(s) => s,
                lsp_types::MarkedString::LanguageString(ls) => ls.value,
            },
            lsp_types::HoverContents::Array(arr) => arr
                .into_iter()
                .map(|item| match item {
                    lsp_types::MarkedString::String(s) => s,
                    lsp_types::MarkedString::LanguageString(ls) => ls.value,
                })
                .collect::<Vec<_>>()
                .join("\n\n"),
            lsp_types::HoverContents::Markup(markup) => markup.value,
        };

        let popover = Popover::new(
            "hover-popover",
            self.editor.clone(),
            self.symbol_range.clone(),
            move |window, cx| render_markdown("message", contents.clone(), window, cx),
        );
        // zeDB patch (hoverable link cards).
        let popover = if self.has_link {
            popover.track_hover(self.hovered.clone())
        } else {
            popover
        };
        popover.into_any_element()
    }
}

pub(crate) struct Popover {
    id: ElementId,
    style: StyleRefinement,
    editor: Entity<InputState>,
    range: Range<usize>,
    width_limit: Range<Pixels>,
    content_builder: Box<dyn Fn(&mut Window, &mut App) -> AnyElement>,
    /// zeDB patch (hoverable link cards): set true while the pointer
    /// is inside the card.
    hover_flag: Option<Rc<std::cell::Cell<bool>>>,
}

impl Styled for Popover {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl Popover {
    pub fn new<F, E>(
        id: impl Into<ElementId>,
        editor: Entity<InputState>,
        range: Range<usize>,
        f: F,
    ) -> Self
    where
        F: Fn(&mut Window, &mut App) -> E + 'static,
        E: IntoElement,
    {
        Self {
            id: id.into(),
            editor,
            range,
            style: StyleRefinement::default(),
            width_limit: px(200.)..px(500.),
            content_builder: Box::new(move |window, cx| (f)(window, cx).into_any_element()),
            hover_flag: None,
        }
    }

    /// zeDB patch (hoverable link cards): report pointer presence
    /// inside the card through `flag`.
    pub(crate) fn track_hover(mut self, flag: Rc<std::cell::Cell<bool>>) -> Self {
        self.hover_flag = Some(flag);
        self
    }

    /// Get the bounds of the range in the editor, if it is visible.
    fn trigger_bounds(&self, cx: &App) -> Option<Bounds<Pixels>> {
        let editor = self.editor.read(cx);
        let Some(last_layout) = editor.last_layout.as_ref() else {
            return None;
        };

        let Some(last_bounds) = editor.last_bounds else {
            return None;
        };

        let (_, _, start_pos) = editor.line_and_position_for_offset(self.range.start);
        let (_, _, end_pos) = editor.line_and_position_for_offset(self.range.end);

        let Some(start_pos) = start_pos else {
            return None;
        };
        let Some(end_pos) = end_pos else {
            return None;
        };

        Some(Bounds::from_corners(
            last_bounds.origin + start_pos,
            last_bounds.origin + end_pos + point(px(0.), last_layout.line_height),
        ))
    }
}

impl IntoElement for Popover {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

pub(crate) struct PopoverLayoutState {
    state: Entity<bool>,
    bounds: Bounds<Pixels>,
    element: Option<AnyElement>,
}

impl Element for Popover {
    type RequestLayoutState = PopoverLayoutState;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, Self::RequestLayoutState) {
        let open_state = window.use_keyed_state("popover-open", cx, |_, _| true);
        let trigger_bounds = match self.trigger_bounds(cx) {
            Some(bounds) => bounds,
            None => {
                return (
                    div().into_any_element().request_layout(window, cx),
                    PopoverLayoutState {
                        bounds: Bounds::default(),
                        element: None,
                        state: open_state,
                    },
                )
            }
        };

        let max_width = self
            .width_limit
            .end
            .min(window.bounds().size.width - SNAP_TO_EDGE * 2)
            .max(px(200.));
        let max_height = (window.bounds().size.height - SNAP_TO_EDGE * 2).min(px(320.));

        let is_open = *open_state.read(cx);

        let mut popover = deferred(
            div()
                .id("hover-popover-content")
                .when(!is_open, |s| s.invisible())
                .flex_none()
                // zeDB patch: the hover card is informational and must
                // not swallow the caret-placing click. `.occlude()` ate
                // clicks over its footprint, so clicking into a
                // statement under the card left the caret (and the next
                // Run) on the previous statement. Clicks now pass
                // through to the editor; wheel scrolling stays
                // contained to the card.
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                // zeDB patch (hoverable link cards): pointer presence
                // keeps a link-bearing card open (see lsp/hover.rs).
                .when_some(self.hover_flag.clone(), |style, flag| {
                    style.on_hover(move |hovered, _, _| flag.set(*hovered))
                })
                // zeDB patch: roomier padding so hover cards (schema
                // db.table.column + type) don't read as cramped.
                .px_2p5()
                .py_1p5()
                .text_xs()
                .popover_style(cx)
                .shadow_md()
                .max_w(max_width)
                .max_h(max_height)
                .overflow_y_scroll()
                .refine_style(&self.style)
                // zeDB patch: a tail pad inside the scroll area so the
                // last line of a long card (setting descriptions) ends
                // with air instead of on the border.
                .child(div().pb_1p5().child((self.content_builder)(window, cx))),
        )
        .into_any_element();

        let popover_size = popover.layout_as_root(AvailableSpace::min_size(), window, cx);
        const SNAP_TO_EDGE: Pixels = px(8.);
        let top_space = trigger_bounds.top() - SNAP_TO_EDGE;
        let right_space = window.bounds().size.width - trigger_bounds.left() - SNAP_TO_EDGE;

        let mut pos = point(
            trigger_bounds.left(),
            trigger_bounds.top() - popover_size.height,
        );
        if popover_size.height > top_space {
            pos.y = trigger_bounds.bottom();
        }
        if popover_size.width > right_space {
            pos.x = trigger_bounds.right() - popover_size.width;
        }

        let mut empty = div().into_any_element();
        let layout_id = empty.request_layout(window, cx);
        (
            layout_id,
            PopoverLayoutState {
                bounds: Bounds {
                    origin: pos,
                    size: popover_size,
                },
                element: Some(popover),
                state: open_state,
            },
        )
    }

    fn prepaint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let bounds = request_layout.bounds;
        let Some(popover) = request_layout.element.as_mut() else {
            return;
        };

        window.with_absolute_element_offset(bounds.origin, |window| {
            popover.prepaint(window, cx);
        })
    }

    fn paint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let bounds = request_layout.bounds;
        let Some(popover) = request_layout.element.as_mut() else {
            return;
        };

        popover.paint(window, cx);

        let open_state = request_layout.state.clone();
        // Mouse down out to hide.
        window.on_mouse_event(move |event: &MouseDownEvent, _, _, cx| {
            if !bounds.contains(&event.position) {
                open_state.update(cx, |open, cx| {
                    *open = false;
                    cx.notify();
                })
            }
        })
    }
}
