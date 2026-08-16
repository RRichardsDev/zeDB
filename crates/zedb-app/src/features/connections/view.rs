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
        let full = value.clone();
        // Long values (Cloud endpoint URLs) show their identifying
        // start and ending port around a middle ellipsis; the tooltip
        // has the whole thing.
        let value = if value.chars().count() > 44 {
            let head: String = value.chars().take(24).collect();
            let tail: String = {
                let chars: Vec<char> = value.chars().collect();
                chars[chars.len() - 12..].iter().collect()
            };
            format!("{head}\u{2026}{tail}")
        } else {
            value
        };
        div()
            .id(gpui::SharedString::from(format!("locked-{value}")))
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
            // Long values (Cloud endpoints) clip; the tooltip carries
            // the whole thing.
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(value),
            )
            .child(
                gpui::svg()
                    .path("icons/lock.svg")
                    .size(px(11.))
                    .flex_none()
                    .text_color(theme::text_dim()),
            )
            .tooltip(move |window, cx| {
                gpui_component::tooltip::Tooltip::new(full.clone()).build(window, cx)
            })
    }
}
