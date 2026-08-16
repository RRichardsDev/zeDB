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
        // Values the Cloud control plane owns render locked: editing
        // them locally would only break the service link.
        let cloud_locked = form.cloud.is_some();
        let endpoint_rows = form
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(150.)).flex_none().child(if cloud_locked {
                        Self::locked_value(node.name.read(cx).text()).into_any_element()
                    } else {
                        node.name.clone().into_any_element()
                    }))
                    .child(div().flex_1().child(if cloud_locked {
                        Self::locked_value(node.endpoint.read(cx).text()).into_any_element()
                    } else {
                        node.endpoint.clone().into_any_element()
                    }))
                    // Explicit native (TCP) port; empty leaves discovery
                    // (advertised port, then the remap offset) in charge.
                    .child(div().w(px(90.)).flex_none().child(if cloud_locked {
                        Self::locked_value(node.native_port.read(cx).text()).into_any_element()
                    } else {
                        node.native_port.clone().into_any_element()
                    }))
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
                        // Same width as Preferences and the Cloud
                        // panel, so the pages line up.
                        .w(px(680.))
                        .max_w_full()
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
                                        .child(if cloud_locked {
                                            // A Cloud service's topology
                                            // comes from the control
                                            // plane: adding a node here
                                            // means adding another
                                            // service, so go back to the
                                            // Cloud panel.
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
                                                .tooltip(|window, cx| {
                                                    gpui_component::tooltip::Tooltip::new(
                                                        "Cloud services manage their own nodes; \
                                                         add another service from the Cloud panel",
                                                    )
                                                    .build(window, cx)
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.cancel_form(cx);
                                                    this.cloud_open(cx);
                                                }))
                                                .into_any_element()
                                        } else {
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
                                                }))
                                                .into_any_element()
                                        }),
                                )
                                .children(endpoint_rows),
                        )
                        .child(Self::field("USER", form.user.clone()))
                        .child(Self::field("DATABASE", form.database.clone()))
                        .child(Self::field("PASSWORD", form.password.clone()))
                        .when_some(form.cloud.clone(), |panel, cloud| {
                            let keyed = self
                                .connection
                                .cloud
                                .org_has_key(&self.preferences, &cloud.org_id);
                            panel
                                .when(keyed, |panel| {
                                    panel.child(match form.provision {
                                    ProvisionStage::Idle => div()
                                        .flex()
                                        .child(
                                            div()
                                                .id("cloud-provision")
                                                // Brand treatment like
                                                // the sign-in button: it
                                                // acts on ClickHouse
                                                // Cloud itself.
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .px_2()
                                                .py_0p5()
                                                .rounded(px(3.))
                                                .border_1()
                                                .border_color(gpui::rgb(0xFFCC01))
                                                .bg(theme::bg_sidebar())
                                                .text_xs()
                                                .text_color(gpui::rgb(0xFFCC01))
                                                .child(
                                                    gpui::svg()
                                                        .path("icons/clickhouse.svg")
                                                        .size(px(11.))
                                                        .text_color(gpui::rgb(0xFFCC01)),
                                                )
                                                .child("Provision password")
                                                .hover(|button| {
                                                    button
                                                        .bg(theme::hover())
                                                        .cursor_pointer()
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    if let Some(form) =
                                                        this.connection.form.as_mut()
                                                    {
                                                        form.provision = ProvisionStage::Confirm;
                                                        cx.notify();
                                                    }
                                                })),
                                        )
                                        .into_any_element(),
                                    ProvisionStage::Confirm => div()
                                        .flex()
                                        .flex_col()
                                        .gap_2()
                                        .p_3()
                                        .rounded(px(4.))
                                        .border_1()
                                        .border_color(theme::warning())
                                        .child(div().text_xs().text_color(theme::text()).child(
                                            "This rotates the service's database password: the \
                                             current one stops working everywhere. The new \
                                             password goes into the field above and, on save, \
                                             the macOS Keychain; it is never displayed.",
                                        ))
                                        .child(
                                            div()
                                                .flex()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .id("cloud-provision-confirm")
                                                        .px_2()
                                                        .py_0p5()
                                                        .rounded(px(3.))
                                                        .bg(theme::primary())
                                                        .text_xs()
                                                        .text_color(theme::primary_foreground())
                                                        .child("Rotate and provision")
                                                        .hover(|button| {
                                                            button
                                                                .bg(theme::primary_hover())
                                                                .cursor_pointer()
                                                        })
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            this.cloud_provision_password(cx)
                                                        })),
                                                )
                                                .child(
                                                    div()
                                                        .id("cloud-provision-cancel")
                                                        .px_2()
                                                        .py_0p5()
                                                        .rounded(px(3.))
                                                        .border_1()
                                                        .border_color(theme::border())
                                                        .text_xs()
                                                        .text_color(theme::text_dim())
                                                        .child("Cancel")
                                                        .hover(|button| {
                                                            button
                                                                .bg(theme::hover())
                                                                .cursor_pointer()
                                                        })
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            if let Some(form) =
                                                                this.connection.form.as_mut()
                                                            {
                                                                form.provision =
                                                                    ProvisionStage::Idle;
                                                                cx.notify();
                                                            }
                                                        })),
                                                ),
                                        )
                                        .into_any_element(),
                                    ProvisionStage::Working => div()
                                        .text_xs()
                                        .text_color(theme::text_dim())
                                        .child("Provisioning a new password\u{2026}")
                                        .into_any_element(),
                                })
                                })
                                .when(!keyed, |panel| {
                                    let console_url = format!(
                                        "https://console.clickhouse.cloud/organizations/{}/keys",
                                        cloud.org_id
                                    );
                                    panel.child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(theme::text_dim())
                                                    .child("ORGANIZATION API KEY \u{b7} OPTIONAL"),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(theme::text_dim())
                                                    .child(
                                                    "The browser sign-in is read-only. Paste an \
                                                     organization API key to let zeDB provision \
                                                     the database password here and wake idle \
                                                     services; it cannot create one for you.",
                                                ),
                                            )
                                            .child(
                                                div()
                                                    .id("cloud-form-console")
                                                    .text_xs()
                                                    .text_color(theme::accent())
                                                    .child(
                                                        "Create one in the Cloud console \
                                                     (Organization \u{2192} API keys)",
                                                    )
                                                    .hover(|link| link.cursor_pointer())
                                                    .on_click(cx.listener(move |_, _, _, cx| {
                                                        cx.open_url(&console_url);
                                                    })),
                                            )
                                            .when_some(form.key_id.clone(), |section, key_id| {
                                                section.child(Self::field("API KEY ID", key_id))
                                            })
                                            .when_some(
                                                form.key_secret.clone(),
                                                |section, key_secret| {
                                                    section.child(Self::field(
                                                        "API KEY SECRET",
                                                        key_secret,
                                                    ))
                                                },
                                            )
                                            .child(
                                                div().flex().child(
                                                    div()
                                                        .id("cloud-form-link")
                                                        .px_2()
                                                        .py_0p5()
                                                        .rounded(px(3.))
                                                        .border_1()
                                                        .border_color(theme::border())
                                                        .text_xs()
                                                        .text_color(theme::text())
                                                        .child(if form.linking_key {
                                                            "Linking\u{2026}"
                                                        } else {
                                                            "Link key"
                                                        })
                                                        .hover(|button| {
                                                            button
                                                                .bg(theme::hover())
                                                                .cursor_pointer()
                                                        })
                                                        .when(!form.linking_key, |button| {
                                                            button.on_click(cx.listener(
                                                                |this, _, _, cx| {
                                                                    this.cloud_link_from_form(cx)
                                                                },
                                                            ))
                                                        }),
                                                ),
                                            ),
                                    )
                                })
                        })
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
                        .when(form.cloud.is_some() && form.read_only, |panel| {
                            // The silent default gates KILL QUERY and
                            // measured codec savings; one sentence
                            // makes it a choice instead of a surprise.
                            panel.child(div().text_xs().text_color(theme::text_dim()).child(
                                "Cloud connections start read-only: no writes, no KILL QUERY, \
                                 and advisors skip measurements that write. Flip the toggle \
                                 here when you want a writable session.",
                            ))
                        })
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
