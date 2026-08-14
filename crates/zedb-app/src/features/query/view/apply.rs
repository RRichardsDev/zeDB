use crate::*;

use gpui::prelude::*;

impl Workspace {
    /// A small spinning indicator for a slow in-place apply, a rotating
    /// refresh icon (gpui-component's Spinner needs an asset the app does
    /// not serve, so this reuses the whitelisted icon).
    pub(crate) fn advice_spinner() -> impl IntoElement {
        use gpui::{percentage, Animation, AnimationExt as _, Transformation};
        use gpui_component::Sizable as _;
        gpui_component::Icon::empty()
            .path("icons/refresh.svg")
            .with_size(gpui_component::Size::Small)
            .text_color(theme::text_dim())
            .with_animation(
                "advice-spin",
                Animation::new(Duration::from_secs(1)).repeat(),
                |icon, delta| icon.transform(Transformation::rotate(percentage(delta))),
            )
    }

    /// The large-table apply confirmation. Deferred so
    /// it paints above everything, with an occluding backdrop that dims
    /// the window and dismisses on an outside click.
    pub(crate) fn apply_confirm_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let size = self
            .schema
            .selected_object
            .as_ref()
            .and_then(|selected| selected.object.total_bytes)
            .map(Self::format_bytes)
            .unwrap_or_default();
        gpui::deferred(
            div()
                .id("apply-confirm")
                .occlude()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::rgba(0x00000088))
                .on_click(cx.listener(|this, _, _, cx| this.cancel_apply(cx)))
                .child(
                    div()
                        .id("apply-dialog")
                        .occlude()
                        .w(px(440.))
                        .p_4()
                        .rounded(px(6.))
                        .bg(theme::bg())
                        .border_1()
                        .border_color(theme::border())
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(div().text_color(theme::text()).child("Apply this change?"))
                        .child(div().text_xs().text_color(theme::text_dim()).child(format!(
                            "This rewrites the whole table (about {size}). It can take a while \
                         and use significant resources. Continue?"
                        )))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    div()
                                        .id("apply-cancel")
                                        .px_3()
                                        .py_1()
                                        .rounded(px(4.))
                                        .text_xs()
                                        .text_color(theme::text_dim())
                                        .cursor_pointer()
                                        .hover(|button| {
                                            button.bg(theme::hover()).text_color(theme::text())
                                        })
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.cancel_apply(cx)),
                                        )
                                        .child("Cancel"),
                                )
                                .child(
                                    div()
                                        .id("apply-continue")
                                        .group("apply-continue")
                                        .px_3()
                                        .py_1()
                                        .rounded(px(4.))
                                        .border_1()
                                        .border_color(theme::warning())
                                        .text_xs()
                                        .text_color(theme::warning())
                                        .cursor_pointer()
                                        .hover(|button| {
                                            button
                                                .bg(theme::warning())
                                                .border_color(theme::warning())
                                        })
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.confirm_apply(window, cx)
                                        }))
                                        .child(
                                            div()
                                                .group_hover("apply-continue", |label| {
                                                    label.text_color(rgb(0x14171c))
                                                })
                                                .child("Continue"),
                                        ),
                                ),
                        ),
                ),
        )
    }
}
