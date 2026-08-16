use crate::*;

use gpui::prelude::*;

#[path = "view/form.rs"]
mod form;
#[path = "view/node_selector.rs"]
mod node_selector;
#[path = "view/toolbar.rs"]
mod toolbar;
#[path = "view/topology.rs"]
mod topology;

impl Workspace {
    pub(crate) fn field(label: &'static str, input: Entity<TextInput>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().text_xs().text_color(theme::text_dim()).child(label))
            .child(input)
    }

    /// A field the user cannot edit, styled like a disabled input:
    /// for values the ClickHouse Cloud control plane owns (name,
    /// endpoint, port), where a local edit would only break the link.
    pub(crate) fn locked_value(value: String) -> impl IntoElement {
        div()
            .h(px(34.))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .rounded(px(3.))
            .border_1()
            .border_color(theme::border())
            .bg(theme::bg_sidebar())
            .text_color(theme::text_dim())
            .child(div().flex_1().child(value))
            .child(
                gpui::svg()
                    .path("icons/lock.svg")
                    .size(px(11.))
                    .text_color(theme::text_dim()),
            )
    }
}
