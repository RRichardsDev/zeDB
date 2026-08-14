use super::*;

#[path = "view/transcript.rs"]
mod transcript;

use transcript::render_entry;

impl Workspace {
    pub(crate) fn agent_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut panel = div()
            .w(px(self.agent.width))
            .flex_none()
            .h_full()
            .relative()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(theme::border())
            .bg(theme::bg_sidebar())
            .child(gpui::deferred(
                div()
                    .id("agent-pane-resize")
                    .absolute()
                    .left(px(-6.))
                    .top_0()
                    .bottom_0()
                    .w(px(13.))
                    .cursor_col_resize()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| {
                            this.agent.resizing = true;
                            cx.notify();
                        }),
                    ),
            ));

        let title = self
            .agent
            .thread
            .as_ref()
            .map(|thread| {
                if thread.running {
                    format!("{} Thread · working", thread.agent_name)
                } else {
                    format!("{} Thread", thread.agent_name)
                }
            })
            .unwrap_or_else(|| "New Thread".into());
        panel = panel.child(
            div()
                .flex_none()
                .px_3()
                .py_2()
                .flex()
                .items_center()
                .justify_between()
                .border_b_1()
                .border_color(theme::border())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .when_some(
                            self.agent
                                .thread
                                .as_ref()
                                .map(|thread| thread.agent_icon.clone()),
                            |header, icon| {
                                header.child(
                                    svg().path(icon).size(px(14.)).text_color(theme::text_dim()),
                                )
                            },
                        )
                        .child(div().text_color(theme::text()).child(title)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            Button::new("agent-new-thread")
                                .label("+")
                                .compact()
                                .outline()
                                .dropdown_menu({
                                    // Snapshot the registry for the
                                    // closure: (name, icon, hint, missing).
                                    let rows: Vec<(String, String, Option<String>, bool)> = self
                                        .agent
                                        .agents
                                        .iter()
                                        .map(|agent| {
                                            use zedb_acp::discovery::Availability;
                                            let (hint, missing) = match &agent.availability {
                                                Availability::Ready => (None, false),
                                                Availability::NeedsLogin { hint } => {
                                                    (Some(hint.clone()), false)
                                                }
                                                Availability::Missing { hint } => {
                                                    (Some(hint.clone()), true)
                                                }
                                            };
                                            (
                                                agent.name.clone(),
                                                icon_for(&agent.id).to_string(),
                                                hint,
                                                missing,
                                            )
                                        })
                                        .collect();
                                    move |menu: PopupMenu, _, _| {
                                        // A Zed-style section header: small
                                        // and dim, unmistakably not an item.
                                        let mut menu = menu.menu_element_with_disabled(
                                            Box::new(StartAgentThread { index: usize::MAX }),
                                            true,
                                            |_, _| {
                                                div()
                                                    .text_xs()
                                                    .text_color(theme::text_dim())
                                                    .child("External Agents")
                                            },
                                        );
                                        for (index, (name, icon_path, hint, missing)) in
                                            rows.clone().into_iter().enumerate()
                                        {
                                            menu = menu.menu_element_with_disabled(
                                                Box::new(StartAgentThread { index }),
                                                missing,
                                                move |_, _| {
                                                    div()
                                                        .w_full()
                                                        .py_0p5()
                                                        .flex()
                                                        .flex_col()
                                                        .gap_0p5()
                                                        .when(!missing, |row| row.cursor_pointer())
                                                        .child(
                                                            div()
                                                                .flex()
                                                                .items_center()
                                                                .gap_2()
                                                                .child(
                                                                    svg()
                                                                        .path(icon_path.clone())
                                                                        .size(px(19.))
                                                                        .text_color(
                                                                            theme::text_dim(),
                                                                        ),
                                                                )
                                                                .child(name.clone()),
                                                        )
                                                        .when_some(hint.clone(), |row, hint| {
                                                            row.child(
                                                                div()
                                                                    .text_xs()
                                                                    .text_color(theme::text_dim())
                                                                    .child(hint),
                                                            )
                                                        })
                                                },
                                            );
                                        }
                                        menu.separator()
                                            .menu("Add More Agents", Box::new(OpenAddAgent))
                                    }
                                }),
                        )
                        .child(
                            div()
                                .id("agent-close")
                                .px_2()
                                .py_1()
                                .rounded(px(3.))
                                .text_color(theme::text_dim())
                                .child("x")
                                .hover(|button| button.bg(theme::hover()).cursor_pointer())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.agent.open = false;
                                    cx.notify();
                                })),
                        ),
                ),
        );

        if let Some(form) = &self.agent.add_form {
            let mut card = div()
                .flex_none()
                .p_2()
                .border_b_1()
                .border_color(theme::border())
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::text_dim())
                        .child("Add an ACP-speaking agent (name + command line):"),
                )
                .child(
                    div()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(theme::bg())
                        .child(
                            Input::new(&form.name)
                                .appearance(false)
                                .bordered(false)
                                .focus_bordered(false)
                                .pl(px(4.)),
                        ),
                )
                .child(
                    div()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(theme::bg())
                        .child(
                            Input::new(&form.command)
                                .appearance(false)
                                .bordered(false)
                                .focus_bordered(false)
                                .pl(px(4.)),
                        ),
                );
            if let Some(error) = &form.error {
                card = card.child(
                    div()
                        .text_xs()
                        .text_color(theme::danger())
                        .child(error.clone()),
                );
            }
            for (index, custom) in self.preferences.custom_agents.iter().enumerate() {
                card = card.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .text_xs()
                        .text_color(theme::text_dim())
                        .child(format!("{} ({})", custom.name, custom.command))
                        .child(
                            div()
                                .id(("agent-custom-remove", index))
                                .px_2()
                                .rounded(px(3.))
                                .child("remove")
                                .hover(|button| button.bg(theme::hover()).cursor_pointer())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.agent_remove_custom(index, cx);
                                })),
                        ),
                );
            }
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("agent-add-save")
                            .px_3()
                            .py_1()
                            .rounded(px(3.))
                            .border_1()
                            .border_color(theme::border())
                            .text_color(theme::text())
                            .child("Add")
                            .hover(|button| button.bg(theme::hover()).cursor_pointer())
                            .on_click(cx.listener(|this, _, _, cx| this.agent_save_custom(cx))),
                    )
                    .child(
                        div()
                            .id("agent-add-cancel")
                            .px_3()
                            .py_1()
                            .rounded(px(3.))
                            .text_color(theme::text_dim())
                            .child("Cancel")
                            .hover(|button| button.bg(theme::hover()).cursor_pointer())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.agent.add_form = None;
                                cx.notify();
                            })),
                    ),
            );
            panel = panel.child(card);
        }

        let mut transcript = div()
            .id("agent-transcript")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_3()
            .flex()
            .flex_col()
            .gap_2();
        if let Some(thread) = self.agent.thread.as_ref() {
            transcript = transcript.track_scroll(&thread.scroll);
            if thread.stick_to_bottom {
                thread.scroll.scroll_to_bottom();
            }
            let scroll = thread.scroll.clone();
            transcript = transcript.on_scroll_wheel(cx.listener(
                move |this, event: &gpui::ScrollWheelEvent, _, cx| {
                    let Some(thread) = this.agent.thread.as_mut() else {
                        return;
                    };
                    let upward = match event.delta {
                        gpui::ScrollDelta::Pixels(delta) => delta.y > gpui::px(0.),
                        gpui::ScrollDelta::Lines(delta) => delta.y > 0.,
                    };
                    if upward {
                        thread.stick_to_bottom = false;
                    } else {
                        // Re-stick when the wheel brings us near the end.
                        let max = scroll.max_offset().height;
                        let position = -scroll.offset().y;
                        if max - position < gpui::px(40.) {
                            thread.stick_to_bottom = true;
                        }
                    }
                    cx.notify();
                },
            ));
        }
        if let Some(thread) = self.agent.thread.as_ref() {
            for (index, entry) in thread.entries.iter().enumerate() {
                transcript = transcript.child(render_entry(index, entry, window, cx));
            }
            if thread.running {
                transcript = transcript.child(
                    div()
                        .text_color(theme::text_dim())
                        .text_xs()
                        .child("working..."),
                );
            }
        } else if let Some((agent_name, entries)) = self.agent.restored.as_ref() {
            transcript = transcript.child(
                div()
                    .text_xs()
                    .text_color(theme::text_dim())
                    .child(format!("{agent_name} thread (read-only)")),
            );
            for (index, (kind, text)) in entries.iter().enumerate() {
                let element = match kind.as_str() {
                    "user" => div()
                        .p_2()
                        .rounded(px(4.))
                        .bg(theme::selected())
                        .text_color(theme::text_dim())
                        .child(
                            TextView::markdown(("agent-restored", index), text.clone(), window, cx)
                                .selectable(true),
                        )
                        .into_any_element(),
                    "assistant" => div()
                        .text_color(theme::text_dim())
                        .child(
                            TextView::markdown(("agent-restored", index), text.clone(), window, cx)
                                .selectable(true),
                        )
                        .into_any_element(),
                    _ => div()
                        .text_xs()
                        .text_color(theme::text_dim())
                        .child(text.clone())
                        .into_any_element(),
                };
                transcript = transcript.child(element);
            }
        } else {
            let has_saved = transcript_path()
                .map(|path| path.is_file())
                .unwrap_or(false);
            transcript = transcript.child(
                div()
                    .w_full()
                    .py_8()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .text_color(theme::text_dim())
                    .child("Start a thread with the + menu above")
                    .when_some(self.preferences.last_agent.clone(), |hint, last| {
                        let icon = self
                            .agent
                            .agents
                            .iter()
                            .find(|agent| agent.name == last)
                            .map(|agent| icon_for(&agent.id))
                            .unwrap_or("icons/sparkle.svg");
                        hint.child(
                            div()
                                .id("agent-new-last")
                                .px_3()
                                .py_1()
                                .rounded(px(3.))
                                .border_1()
                                .border_color(theme::border())
                                .text_xs()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(svg().path(icon).size(px(13.)).text_color(theme::text()))
                                .child(format!("New thread with {last} (cmd-N)"))
                                .hover(|button| button.bg(theme::hover()).cursor_pointer())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.agent_start_last_thread(window, cx)
                                })),
                        )
                    })
                    .when(has_saved, |hint| {
                        hint.child(
                            div()
                                .id("agent-reopen-last")
                                .px_3()
                                .py_1()
                                .rounded(px(3.))
                                .border_1()
                                .border_color(theme::border())
                                .text_xs()
                                .child("Reopen last thread (read-only)")
                                .hover(|button| button.bg(theme::hover()).cursor_pointer())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.agent.restored = load_transcript();
                                    cx.notify();
                                })),
                        )
                    }),
            );
        }
        panel = panel.child(transcript);

        // Status line (auth hints, stop errors).
        if let Some(status) = self
            .agent
            .thread
            .as_ref()
            .and_then(|thread| thread.status.clone())
        {
            panel = panel.child(
                div()
                    .flex_none()
                    .px_3()
                    .py_1()
                    .bg(theme::bg_status())
                    .border_t_1()
                    .border_color(theme::border())
                    .text_xs()
                    .text_color(theme::danger())
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(status),
            );
        }

        if let Some(thread) = self.agent.thread.as_ref() {
            let running = thread.running;
            let ready = thread.session_id.is_some();
            panel = panel.child(
                div()
                    .flex_none()
                    .p_2()
                    .border_t_1()
                    .border_color(theme::border())
                    .flex()
                    .items_end()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .rounded(px(3.))
                            .border_1()
                            .border_color(theme::border())
                            .bg(theme::bg())
                            .child(
                                Input::new(&thread.input)
                                    .appearance(false)
                                    .bordered(false)
                                    .focus_bordered(false)
                                    .pl(px(4.)),
                            ),
                    )
                    .map(|composer| {
                        if running {
                            composer.child(
                                div()
                                    .id("agent-cancel")
                                    .size(px(28.))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(3.))
                                    .border_1()
                                    .border_color(theme::danger())
                                    .child(
                                        svg()
                                            .path("icons/stop.svg")
                                            .size(px(12.))
                                            .text_color(theme::danger()),
                                    )
                                    .hover(|button| {
                                        button.bg(theme::danger_hover()).cursor_pointer()
                                    })
                                    .tooltip(|window, cx| {
                                        gpui_component::tooltip::Tooltip::new("Stop the turn")
                                            .build(window, cx)
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| this.agent_cancel(cx))),
                            )
                        } else {
                            composer.child(
                                div()
                                    .id("agent-send")
                                    .group("agent-send")
                                    .size(px(28.))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(3.))
                                    .border_1()
                                    .border_color(theme::border())
                                    .child(
                                        svg()
                                            .path("icons/send.svg")
                                            .size(px(14.))
                                            .text_color(if ready {
                                                theme::text_dim()
                                            } else {
                                                theme::disabled()
                                            })
                                            .when(ready, |icon| {
                                                icon.group_hover("agent-send", |icon| {
                                                    icon.text_color(theme::text())
                                                })
                                            }),
                                    )
                                    .when(ready, |button| {
                                        button
                                            .hover(|button| {
                                                button.bg(theme::hover()).cursor_pointer()
                                            })
                                            .tooltip(|window, cx| {
                                                gpui_component::tooltip::Tooltip::new(
                                                    "Send (enter)",
                                                )
                                                .build(window, cx)
                                            })
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.agent_send(window, cx)
                                            }))
                                    }),
                            )
                        }
                    }),
            );
        }

        panel
    }
}

/// Strip ANSI escape sequences and control characters, and clamp the
/// length: adapters log freely (codex-acp dumps whole model configs in
/// one colored line) and the status line is one line, not a firehose.
pub(super) fn clean_log_line(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for n in chars.by_ref() {
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        if !c.is_control() {
            out.push(c);
        }
        if out.len() >= 200 {
            out.push_str("...");
            break;
        }
    }
    out.trim().to_string()
}
