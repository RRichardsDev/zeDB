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
}
