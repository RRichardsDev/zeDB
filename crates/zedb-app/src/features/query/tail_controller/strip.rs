use crate::*;

use gpui::prelude::*;

impl Workspace {
    /// The live-tail status strip above the editor: what's tailing, the
    /// retained row count, and Pause / Stop.
    pub(crate) fn tail_strip(
        &self,
        info: TailStripInfo,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let TailStripInfo {
            tab_id,
            key,
            paused,
            error,
            rows,
            native_available,
            push,
            experimental_streaming_enabled,
            dirty,
        } = info;
        let icon_button = |id: &'static str, icon: &'static str, color: gpui::Hsla| {
            div()
                .id(id)
                .size(px(22.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(3.))
                .hover(|button| button.bg(theme::hover()).cursor_pointer())
                .child(svg().path(icon).size(px(13.)).text_color(color))
        };
        div()
            .flex_none()
            .h(px(30.))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .bg(theme::bg_sidebar())
            // An orange outline on the whole strip while the query is edited
            // (unapplied), alongside the green Update Tail button.
            .when(dirty, |strip| {
                strip.border_1().border_color(theme::warning())
            })
            .child(
                // A live dot: accent when following, dim when paused.
                div().size(px(7.)).rounded_full().bg(if paused {
                    theme::text_dim()
                } else {
                    theme::accent()
                }),
            )
            .child(div().text_xs().text_color(theme::text()).child(if paused {
                format!("Tail paused · advancing on {key}")
            } else {
                format!("Tailing · advancing on {key}")
            }))
            .child(
                div()
                    .text_xs()
                    .text_color(theme::text_dim())
                    .child(format!("· {rows} rows")),
            )
            .when(push != TailPush::Poll, |row| {
                // Instant updates active: name the mechanism.
                row.child(
                    div()
                        .text_xs()
                        .text_color(theme::accent())
                        .child(match push {
                            TailPush::Stream => "· instant (STREAM)",
                            TailPush::Watch => "· instant (WATCH)",
                            _ => "· instant (native)",
                        }),
                )
            })
            .when_some(error, |row, error| {
                row.child(
                    div()
                        .text_xs()
                        .text_color(theme::danger())
                        .child(format!("· {error}")),
                )
            })
            .child(div().flex_1())
            .when(dirty, |row| {
                // The editor query was edited; a green-outlined text button
                // (left of "Get instant updates") that reads as "apply your
                // changes".
                row.child(
                    div()
                        .id("tail-update")
                        .px_2()
                        .py_0p5()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(theme::success())
                        .text_xs()
                        .text_color(theme::success())
                        .child("Update Tail")
                        .hover(|button| button.bg(theme::hover()).cursor_pointer())
                        .tooltip(|window, cx| {
                            gpui_component::tooltip::Tooltip::new(
                                "Apply the edited query to the tail",
                            )
                            .build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.update_tail_from_editor(tab_id, cx)
                        })),
                )
            })
            .when(native_available && push == TailPush::Poll, |row| {
                row.child(
                    icon_button(
                        "tail-experimental-settings",
                        "icons/experimental.svg",
                        if experimental_streaming_enabled {
                            theme::warning()
                        } else {
                            theme::text_dim()
                        },
                    )
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(if experimental_streaming_enabled {
                            "Experimental STREAM tails enabled. Open Preferences"
                        } else {
                            "Experimental STREAM tails disabled. Open Preferences"
                        })
                        .build(window, cx)
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.open_preferences(cx))),
                )
            })
            .when(native_available && push == TailPush::Poll, |row| {
                // Discovery found a native port: offer the server-push
                // upgrade, accent-tinted so it reads as an offer.
                row.child(
                    div()
                        .id("tail-instant")
                        .px_2()
                        .py_0p5()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(theme::accent())
                        .text_xs()
                        .text_color(theme::accent())
                        .child("Get instant updates")
                        .hover(|button| button.bg(theme::hover()).cursor_pointer())
                        .tooltip(|window, cx| {
                            gpui_component::tooltip::Tooltip::new(
                                "Switch to the native (TCP) connection for instant updates",
                            )
                            .build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.upgrade_tail_instant(tab_id, cx);
                        })),
                )
            })
            .child(
                // Paused shows green Play (resume); running shows orange
                // Pause. Stop is always red.
                if paused {
                    icon_button("tail-play", "icons/play.svg", theme::success()).tooltip(
                        |window, cx| {
                            gpui_component::tooltip::Tooltip::new("Resume").build(window, cx)
                        },
                    )
                } else {
                    icon_button("tail-pause", "icons/pause.svg", theme::warning()).tooltip(
                        |window, cx| {
                            gpui_component::tooltip::Tooltip::new("Pause").build(window, cx)
                        },
                    )
                }
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_tail_pause(tab_id, cx))),
            )
            .child(
                icon_button("tail-stop", "icons/stop.svg", theme::danger())
                    .tooltip(|window, cx| {
                        gpui_component::tooltip::Tooltip::new("Stop").build(window, cx)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.stop_tail(tab_id, cx))),
            )
    }
}
