use crate::*;

use gpui::prelude::*;

impl Workspace {
    pub(crate) fn form_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let form = self
            .connection
            .form
            .as_ref()
            .expect("form panel requires a form");
        let endpoint_count = form.nodes.len();
        let endpoint_rows = form
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(150.)).flex_none().child(node.name.clone()))
                    .child(div().flex_1().child(node.endpoint.clone()))
                    // Explicit native (TCP) port; empty leaves discovery
                    // (advertised port, then the remap offset) in charge.
                    .child(div().w(px(90.)).flex_none().child(node.native_port.clone()))
                    .when(endpoint_count > 1, |row| {
                        row.child(
                            div()
                                .id(("remove-endpoint", index))
                                .w(px(30.))
                                .h(px(30.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(3.))
                                .border_1()
                                .border_color(theme::border())
                                .child("-")
                                .hover(|button| button.bg(theme::bg_sidebar()).cursor_pointer())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.remove_endpoint(index, cx)
                                })),
                        )
                    })
            })
            .collect::<Vec<_>>();
        let heading = if form.editing.is_some() {
            "Edit cluster connection"
        } else {
            "Add cluster connection"
        };
        div()
            .id("connection-form-scroll")
            .size_full()
            .overflow_y_scroll()
            .bg(theme::bg())
            .p_6()
            // Centering lives on a non-scroll wrapper: a flex scroll
            // container stretches its child to the viewport height and
            // clips the overflow before scrolling ever sees it.
            .child(
                div().flex().justify_center().w_full().child(
                    div()
                        .w(px(520.))
                        .flex()
                        .flex_col()
                        .gap_4()
                        .child(div().text_lg().text_color(theme::text()).child(heading))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(div().text_xs().text_color(theme::text_dim()).child("NAME"))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(div().flex_1().child(form.name.clone()))
                                        .child(
                                            div()
                                                .id("cycle-tier")
                                                .h(px(34.))
                                                .px_1()
                                                .flex()
                                                .items_center()
                                                .rounded(px(3.))
                                                .child(Self::tier_badge(form.tier))
                                                .hover(|button| {
                                                    button.bg(theme::bg_sidebar()).cursor_pointer()
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.cycle_tier(cx)
                                                })),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme::text_dim())
                                                .child("CLUSTER NODES"),
                                        )
                                        .child(
                                            div()
                                                .id("add-endpoint")
                                                .px_2()
                                                .py_1()
                                                .rounded(px(3.))
                                                .border_1()
                                                .border_color(theme::border())
                                                .child("+ Add node")
                                                .hover(|button| {
                                                    button.bg(theme::bg_sidebar()).cursor_pointer()
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.add_endpoint(cx)
                                                })),
                                        ),
                                )
                                .children(endpoint_rows),
                        )
                        .child(Self::field("USER", form.user.clone()))
                        .child(Self::field("DATABASE", form.database.clone()))
                        .child(Self::field("PASSWORD", form.password.clone()))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme::text_dim())
                                                .child("DRIVER"),
                                        )
                                        .child(
                                            div()
                                                .id("add-driver-setting")
                                                .px_2()
                                                .py_0p5()
                                                .rounded(px(3.))
                                                .text_xs()
                                                .text_color(theme::text_dim())
                                                .hover(|button| {
                                                    button
                                                        .bg(theme::bg_sidebar())
                                                        .text_color(theme::text())
                                                        .cursor_pointer()
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.add_driver_setting(cx)
                                                }))
                                                .child("+ Add setting"),
                                        ),
                                )
                                .children(form.driver_settings.iter().enumerate().map(
                                    |(index, setting)| {
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .w(px(220.))
                                                    .flex_none()
                                                    .child(setting.name.clone()),
                                            )
                                            .child(div().flex_1().child(setting.value.clone()))
                                            .child(
                                                div()
                                                    .id(("remove-driver-setting", index))
                                                    .w(px(30.))
                                                    .h(px(30.))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .rounded(px(3.))
                                                    .border_1()
                                                    .border_color(theme::border())
                                                    .child("-")
                                                    .hover(|button| {
                                                        button
                                                            .bg(theme::bg_sidebar())
                                                            .cursor_pointer()
                                                    })
                                                    .on_click(cx.listener(
                                                        move |this, _, _, cx| {
                                                            this.remove_driver_setting(index, cx)
                                                        },
                                                    )),
                                            )
                                    },
                                ))
                                .when(!form.driver_settings.is_empty(), |section| {
                                    section.child(
                                        div().text_xs().text_color(theme::text_dim()).child(
                                            "Sent with every query on this cluster; \
                                         connect_timeout configures the driver instead. \
                                         Rows without a value are dropped on save.",
                                        ),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .items_center()
                                .gap_3()
                                .child("Read only")
                                .child(
                                    // Same switch as the Vim mode toggle.
                                    div()
                                        .id("toggle-read-only")
                                        .w(px(54.))
                                        .h(px(28.))
                                        .px_1()
                                        .rounded_full()
                                        .flex()
                                        .items_center()
                                        .when(form.read_only, |toggle| {
                                            toggle.justify_end().bg(theme::toggle_on())
                                        })
                                        .when(!form.read_only, |toggle| {
                                            toggle.justify_start().bg(theme::toggle_off())
                                        })
                                        .hover(|toggle| toggle.cursor_pointer())
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.toggle_read_only(cx)),
                                        )
                                        .child(div().size(px(20.)).rounded_full().bg(
                                            if form.read_only {
                                                theme::toggle_knob_on()
                                            } else {
                                                theme::toggle_knob_off()
                                            },
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    div()
                                        .id("cancel-connection")
                                        .px_4()
                                        .py_2()
                                        .rounded(px(3.))
                                        .border_1()
                                        .border_color(theme::border())
                                        .child("Cancel")
                                        .when(self.connection.connecting.is_none(), |button| {
                                            button
                                                .hover(|button| {
                                                    button.bg(theme::bg_sidebar()).cursor_pointer()
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.cancel_form(cx)
                                                }))
                                        }),
                                )
                                .child(
                                    div()
                                        .id("save-offline")
                                        .px_4()
                                        .py_2()
                                        .rounded(px(3.))
                                        .border_1()
                                        .border_color(theme::border())
                                        .child("Save without testing")
                                        .when(self.connection.connecting.is_none(), |button| {
                                            button
                                                .hover(|button| {
                                                    button.bg(theme::bg_sidebar()).cursor_pointer()
                                                })
                                                .on_click(
                                                    cx.listener(|this, _, _, cx| {
                                                        this.save_form(cx)
                                                    }),
                                                )
                                        }),
                                )
                                .child(
                                    div()
                                        .id("save-and-connect")
                                        .px_4()
                                        .py_2()
                                        .rounded(px(3.))
                                        .bg(theme::primary())
                                        .text_color(theme::primary_foreground())
                                        .child(if self.connection.connecting.is_some() {
                                            "Testing nodes..."
                                        } else {
                                            "Save & Connect"
                                        })
                                        .when(self.connection.connecting.is_none(), |button| {
                                            button
                                                .hover(|button| {
                                                    button
                                                        .bg(theme::primary_hover())
                                                        .cursor_pointer()
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.save_and_connect(cx)
                                                }))
                                        }),
                                ),
                        ),
                ),
            )
    }
}
