use crate::*;

use gpui::prelude::*;

impl Workspace {
    pub(crate) fn field(label: &'static str, input: Entity<TextInput>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().text_xs().text_color(theme::text_dim()).child(label))
            .child(input)
    }

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

    pub(crate) fn node_selector(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let connected = self.connection.connected.as_ref()?;
        let connection = self
            .connection
            .connections
            .iter()
            .find(|connection| connection.name == connected.name)?;
        let health = self.connection.endpoint_health.get(&connected.name);
        // A cluster that puts any two of these nodes on different shards
        // makes the picker label every node's shard in that cluster.
        let shard_cluster = health.and_then(|health| {
            health.iter().find_map(|node| {
                health.iter().find_map(|other| {
                    differentiating_cluster(&node.memberships, &other.memberships)
                })
            })
        });
        let nodes = connection
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                let entry =
                    health.and_then(|health| health.iter().find(|item| item.node_index == index));
                let reachable = entry.map(|item| item.reachable).unwrap_or(false);
                let label = match (&shard_cluster, entry) {
                    (Some(cluster), Some(item)) => item
                        .memberships
                        .iter()
                        .find(|membership| &membership.cluster == cluster)
                        .map(|membership| format!("{}  ·  shard {}", node.name, membership.shard))
                        .unwrap_or_else(|| node.name.clone()),
                    _ => node.name.clone(),
                };
                (index, label, reachable)
            })
            .collect::<Vec<_>>();
        let active_name = connection
            .nodes
            .get(connected.active_node)
            .map(|node| node.name.clone())
            .unwrap_or_else(|| "Select node".into());
        // Clusters the connected node belongs to. Picking one runs
        // schema-apply actions ON CLUSTER instead of just this node.
        let clusters = self.ops_cluster_options();
        let apply_cluster = connected.apply_cluster.clone();
        // In cluster scope the label reads the cluster, not the node.
        let label = match &apply_cluster {
            Some(name) => format!("Cluster: {name}"),
            None => active_name,
        };
        let action_context = self.query.tabs[self.query.active_tab]
            .editor
            .focus_handle(cx);

        Some(
            Button::new("active-node-selector")
                .label(label)
                .dropdown_caret(true)
                .compact()
                .outline()
                .dropdown_menu(move |menu: PopupMenu, _, _| {
                    let mut menu = nodes.iter().cloned().fold(
                        menu.action_context(action_context.clone()).min_w(px(180.)),
                        |menu, (index, name, reachable)| {
                            menu.menu_with_enable(name, Box::new(SelectNode { index }), reachable)
                        },
                    );
                    if !clusters.is_empty() {
                        menu = menu.separator();
                        for cluster in &clusters {
                            menu = menu.menu(
                                format!("Cluster: {cluster}"),
                                Box::new(SetApplyCluster {
                                    cluster: Some(cluster.clone()),
                                }),
                            );
                        }
                    }
                    menu
                }),
        )
    }

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

    pub(crate) fn cluster_overview(&self) -> impl IntoElement {
        let selected = self
            .connection
            .selected
            .and_then(|index| self.connection.connections.get(index));
        let nodes = selected
            .map(|connection| {
                connection
                    .nodes
                    .iter()
                    .enumerate()
                    .map(|(index, configured_node)| {
                        let reachable = self
                            .connection
                            .endpoint_health
                            .get(&connection.name)
                            .and_then(|health| {
                                health
                                    .iter()
                                    .find(|node| node.node_index == index)
                                    .map(|node| node.reachable)
                            });
                        let (label, color) = match reachable {
                            Some(true) => ("reachable", theme::success()),
                            Some(false) => ("failed", theme::danger()),
                            None => ("not tested", theme::text_dim()),
                        };
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().size(px(7.)).rounded_full().bg(color))
                            .child(configured_node.name.clone())
                            .child(
                                div()
                                    .text_color(theme::text_dim())
                                    .child(configured_node.endpoint.clone()),
                            )
                            .child(div().text_xs().text_color(theme::text_dim()).child(label))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        div().size_full().p_6().flex().justify_center().child(
            div()
                .w(px(560.))
                .flex()
                .flex_col()
                .gap_4()
                .child(
                    div()
                        .text_lg()
                        .text_color(theme::text())
                        .child("Cluster connection"),
                )
                .when_some(selected, |panel, connection| {
                    panel
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(connection.name.clone())
                                .child(Self::tier_badge(connection.tier)),
                        )
                        .child(div().flex().flex_col().gap_2().children(nodes))
                        .children(self.topology_section(connection))
                })
                .when(selected.is_none(), |panel| {
                    panel.child("Add or select a cluster connection to begin.")
                }),
        )
    }

    /// Phase 5 M4: a read-only shards-and-replicas view, built entirely
    /// from the memberships each node reported about itself at connect
    /// time. Nothing here is configurable; zeDB displays what the
    /// servers said. Absent topology (never connected, LBs, Cloud)
    /// renders nothing.
    pub(crate) fn topology_section(
        &self,
        connection: &ConnectionConfig,
    ) -> Option<impl IntoElement> {
        let health = self.connection.endpoint_health.get(&connection.name)?;
        // cluster -> shard -> node display names, insertion-ordered.
        type Shards = Vec<(u64, Vec<String>)>;
        let mut clusters: Vec<(String, Shards)> = Vec::new();
        for node in health {
            for membership in &node.memberships {
                // Each node's implicit "default" cluster contains only
                // itself; merging them across nodes would invent a
                // cluster that does not exist.
                if membership.cluster == "default" {
                    continue;
                }
                let cluster = match clusters
                    .iter_mut()
                    .find(|(name, _)| *name == membership.cluster)
                {
                    Some((_, shards)) => shards,
                    None => {
                        clusters.push((membership.cluster.clone(), Vec::new()));
                        &mut clusters.last_mut().expect("just pushed").1
                    }
                };
                match cluster
                    .iter_mut()
                    .find(|(shard, _)| *shard == membership.shard)
                {
                    Some((_, members)) => members.push(node.name.clone()),
                    None => cluster.push((membership.shard, vec![node.name.clone()])),
                }
            }
        }
        if clusters.is_empty() {
            return None;
        }
        for (_, shards) in &mut clusters {
            shards.sort_by_key(|(shard, _)| *shard);
        }

        Some(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().text_color(theme::text_dim()).child("Topology"))
                .children(clusters.into_iter().map(|(cluster, shards)| {
                    let shard_count = shards.len();
                    let replicas_per_shard = shards
                        .first()
                        .map(|(_, members)| members.len())
                        .unwrap_or(0);
                    let uniform = shards
                        .iter()
                        .all(|(_, members)| members.len() == replicas_per_shard);
                    let replica_summary = if uniform {
                        format!("{shard_count} shard(s) \u{d7} {replicas_per_shard} replica(s)")
                    } else {
                        format!("{shard_count} shards")
                    };
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .p_3()
                        .rounded(px(4.))
                        .border_1()
                        .border_color(theme::border())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().text_color(theme::text()).child(cluster))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme::text_dim())
                                        .child(replica_summary),
                                ),
                        )
                        .children(shards.into_iter().map(|(shard, members)| {
                            div()
                                .flex()
                                .gap_2()
                                .text_sm()
                                .child(
                                    div()
                                        .w(px(80.))
                                        .flex_none()
                                        .text_color(theme::text_dim())
                                        .child(format!("shard {shard}")),
                                )
                                .child(div().text_color(theme::text()).child(members.join(", ")))
                        }))
                })),
        )
    }
}
