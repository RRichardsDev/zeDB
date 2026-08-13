use crate::*;

use gpui::prelude::*;

impl Workspace {
    pub(crate) fn connection_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self
            .connection
            .selected
            .and_then(|index| self.connection.connections.get(index));
        let header_connection = self
            .connection
            .connected
            .as_ref()
            .and_then(|connected| {
                self.connection
                    .connections
                    .iter()
                    .find(|connection| connection.name == connected.name)
            })
            .or(selected);
        let selected_connected = selected
            .map(|connection| {
                self.connection
                    .connected
                    .as_ref()
                    .map(|connected| connected.name.as_str())
                    == Some(connection.name.as_str())
            })
            .unwrap_or(false);
        div()
            .h(px(38.))
            .flex_none()
            .w_full()
            .px_3()
            .flex()
            .items_center()
            .justify_between()
            .bg(theme::bg_sidebar())
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when_some(header_connection, |row, connection| {
                        row.child(connection.name.clone())
                            .child(Self::tier_badge(connection.tier))
                            .when_some(self.node_selector(cx), |row, selector| row.child(selector))
                    })
                    .when(header_connection.is_none(), |row| {
                        row.child("Select a connection")
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("open-fleet")
                            .group("btn-fleet")
                            .size(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.))
                            .border_1()
                            .map(|button| {
                                if self.connection.connected.is_none() {
                                    // Disabled: the fleet view is per-connection.
                                    button
                                        .border_color(theme::disabled_border())
                                        .child(
                                            svg()
                                                .path("icons/fleet.svg")
                                                .size(px(14.))
                                                .text_color(theme::disabled()),
                                        )
                                        .tooltip(|window, cx| {
                                            gpui_component::tooltip::Tooltip::new(
                                                "Connect to a cluster first",
                                            )
                                            .build(window, cx)
                                        })
                                } else {
                                    button
                                        .border_color(theme::border())
                                        .when(self.show_fleet, |button| {
                                            button.bg(theme::selected())
                                        })
                                        .child(
                                            svg()
                                                .path("icons/fleet.svg")
                                                .size(px(14.))
                                                .text_color(if self.show_fleet {
                                                    theme::text()
                                                } else {
                                                    theme::text_dim()
                                                })
                                                .group_hover("btn-fleet", |icon| {
                                                    icon.text_color(theme::text())
                                                }),
                                        )
                                        .hover(|button| button.bg(theme::hover()).cursor_pointer())
                                        .tooltip(|window, cx| {
                                            gpui_component::tooltip::Tooltip::new("Fleet view")
                                                .build(window, cx)
                                        })
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.toggle_fleet(cx)),
                                        )
                                }
                            }),
                    )
                    .child(
                        div()
                            .id("open-ops")
                            .group("btn-ops")
                            .size(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.))
                            .border_1()
                            .map(|button| {
                                if self.connection.connected.is_none() {
                                    button
                                        .border_color(theme::disabled_border())
                                        .child(
                                            svg()
                                                .path("icons/ops.svg")
                                                .size(px(14.))
                                                .text_color(theme::disabled()),
                                        )
                                        .tooltip(|window, cx| {
                                            gpui_component::tooltip::Tooltip::new(
                                                "Connect to a cluster first",
                                            )
                                            .build(window, cx)
                                        })
                                } else {
                                    button
                                        .border_color(theme::border())
                                        .when(self.show_ops, |button| button.bg(theme::selected()))
                                        .child(
                                            svg()
                                                .path("icons/ops.svg")
                                                .size(px(14.))
                                                .text_color(if self.show_ops {
                                                    theme::text()
                                                } else {
                                                    theme::text_dim()
                                                })
                                                .group_hover("btn-ops", |icon| {
                                                    icon.text_color(theme::text())
                                                }),
                                        )
                                        .hover(|button| button.bg(theme::hover()).cursor_pointer())
                                        .tooltip(|window, cx| {
                                            gpui_component::tooltip::Tooltip::new(
                                                "Ops: what the cluster is doing right now",
                                            )
                                            .build(window, cx)
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| this.ops_toggle(cx)))
                                }
                            }),
                    )
                    .child(
                        div()
                            .id("open-query-editor")
                            .group("btn-query")
                            .size(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.))
                            .border_1()
                            .map(|button| {
                                if self.connection.connected.is_none() {
                                    // Disabled; running from an existing tab
                                    // still gets the connect-first warning.
                                    button
                                        .border_color(theme::disabled_border())
                                        .child(
                                            svg()
                                                .path("icons/query-plus.svg")
                                                .size(px(14.))
                                                .text_color(theme::disabled()),
                                        )
                                        .tooltip(|window, cx| {
                                            gpui_component::tooltip::Tooltip::new(
                                                "Connect to a cluster first",
                                            )
                                            .build(window, cx)
                                        })
                                } else {
                                    button
                                        .border_color(theme::border())
                                        .when(!self.show_fleet, |button| {
                                            button.bg(theme::selected())
                                        })
                                        .child(
                                            svg()
                                                .path("icons/query-plus.svg")
                                                .size(px(14.))
                                                .text_color(if self.show_fleet {
                                                    theme::text_dim()
                                                } else {
                                                    theme::text()
                                                })
                                                .group_hover("btn-query", |icon| {
                                                    icon.text_color(theme::text())
                                                }),
                                        )
                                        .hover(|button| button.bg(theme::hover()).cursor_pointer())
                                        .tooltip(|window, cx| {
                                            gpui_component::tooltip::Tooltip::new("New query")
                                                .build(window, cx)
                                        })
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                this.open_query_editor(cx)
                                            }),
                                        )
                                }
                            }),
                    )
                    .child(
                        div()
                            .id("open-agent-pane")
                            .group("btn-agent")
                            .size(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.))
                            .border_1()
                            .border_color(theme::border())
                            .when(self.agent.open, |button| button.bg(theme::selected()))
                            .child(
                                svg()
                                    .path("icons/sparkle.svg")
                                    .size(px(14.))
                                    .text_color(if self.agent.open {
                                        theme::text()
                                    } else {
                                        theme::text_dim()
                                    })
                                    .group_hover("btn-agent", |icon| {
                                        icon.text_color(theme::text())
                                    }),
                            )
                            .hover(|button| button.bg(theme::hover()).cursor_pointer())
                            .tooltip(|window, cx| {
                                gpui_component::tooltip::Tooltip::new(
                                    "Agent pane: AI threads with your installed agents",
                                )
                                .build(window, cx)
                            })
                            .on_click(
                                cx.listener(|this, _, window, cx| this.agent_toggle(window, cx)),
                            ),
                    )
                    .when(selected_connected, |toolbar| {
                        toolbar.child(
                            div()
                                .id("disconnect")
                                .size(px(28.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(3.))
                                .border_1()
                                .border_color(theme::danger())
                                .child(
                                    svg()
                                        .path("icons/plug-off.svg")
                                        .size(px(14.))
                                        .text_color(theme::danger()),
                                )
                                .hover(|button| button.bg(theme::danger_hover()).cursor_pointer())
                                .tooltip(|window, cx| {
                                    gpui_component::tooltip::Tooltip::new("Disconnect")
                                        .build(window, cx)
                                })
                                .on_click(cx.listener(|this, _, _, cx| this.disconnect(cx))),
                        )
                    })
                    .when(!selected_connected, |toolbar| {
                        toolbar.child(
                            div()
                                .id("connect-toggle")
                                .group("btn-connect")
                                .size(px(28.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(3.))
                                .border_1()
                                .map(|button| {
                                    if self.connection.connecting.is_some() {
                                        button
                                            .border_color(theme::border())
                                            .child(
                                                svg()
                                                    .path("icons/plug.svg")
                                                    .size(px(14.))
                                                    .text_color(theme::success()),
                                            )
                                            .tooltip(|window, cx| {
                                                gpui_component::tooltip::Tooltip::new(
                                                    "Connecting...",
                                                )
                                                .build(window, cx)
                                            })
                                    } else if selected.is_some() {
                                        button
                                            .border_color(theme::border())
                                            .child(
                                                svg()
                                                    .path("icons/plug.svg")
                                                    .size(px(14.))
                                                    .text_color(theme::text_dim())
                                                    .group_hover("btn-connect", |icon| {
                                                        icon.text_color(theme::success())
                                                    }),
                                            )
                                            .hover(|button| {
                                                button
                                                    .bg(rgb(if theme::is_dark() {
                                                        0x294132
                                                    } else {
                                                        0xdcefdf
                                                    }))
                                                    .border_color(theme::success())
                                                    .cursor_pointer()
                                            })
                                            .tooltip(|window, cx| {
                                                gpui_component::tooltip::Tooltip::new("Connect")
                                                    .build(window, cx)
                                            })
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.connect_selected(cx)
                                            }))
                                    } else {
                                        // Disabled: nothing selected to connect to.
                                        button
                                            .border_color(theme::disabled_border())
                                            .child(
                                                svg()
                                                    .path("icons/plug.svg")
                                                    .size(px(14.))
                                                    .text_color(theme::disabled()),
                                            )
                                            .tooltip(|window, cx| {
                                                gpui_component::tooltip::Tooltip::new(
                                                    "Select a connection first",
                                                )
                                                .build(window, cx)
                                            })
                                    }
                                }),
                        )
                    }),
            )
    }
}
