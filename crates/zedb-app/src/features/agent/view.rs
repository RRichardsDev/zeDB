use super::*;

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

        // Header: agent name, new thread, close.
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

        // The Add More Agents form.
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
            // Existing custom agents, removable.
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

        // Transcript.
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

        // Composer.
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

/// Fenced code blocks in a markdown reply, for insert-into-editor;
/// untagged and sql/clickhouse-tagged fences count.
fn fenced_sql_blocks(text: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut in_fence = false;
    let mut takes_sql = false;
    let mut current = String::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("```") {
            if in_fence {
                if takes_sql && !current.trim().is_empty() {
                    blocks.push(current.trim_end().to_string());
                }
                current.clear();
                in_fence = false;
            } else {
                in_fence = true;
                let language = rest.trim().to_lowercase();
                takes_sql = language.is_empty() || language == "sql" || language == "clickhouse";
            }
        } else if in_fence {
            current.push_str(line);
            current.push('\n');
        }
    }
    blocks
}

fn render_entry(
    index: usize,
    entry: &ThreadEntry,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> gpui::AnyElement {
    match entry {
        // Our messages echo the composer's look: a rounded, thin-bordered
        // box over the panel background, distinct from the borderless AI
        // replies.
        ThreadEntry::User(text) => div()
            .px_3()
            .py_2()
            .rounded(px(4.))
            .border_1()
            .border_color(theme::border())
            .bg(theme::bg())
            .text_color(theme::text())
            .child(
                TextView::markdown(("agent-user", index), text.clone(), window, cx)
                    .selectable(true),
            )
            .into_any_element(),
        ThreadEntry::Assistant(text) => {
            if text.is_empty() {
                div().into_any_element()
            } else if text.trim_start().starts_with("Automatic approval review") {
                // Adapter housekeeping (Codex's auto-approval notices),
                // visually separated from the agent's actual reply.
                div()
                    .p_2()
                    .rounded(px(4.))
                    .border_1()
                    .border_color(theme::warning())
                    .text_xs()
                    .text_color(theme::warning())
                    .child(
                        TextView::markdown(("agent-notice", index), text.clone(), window, cx)
                            .selectable(true),
                    )
                    .into_any_element()
            } else {
                let blocks = fenced_sql_blocks(text);
                let mut body = div()
                    .text_color(theme::text())
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        TextView::markdown(("agent-md", index), text.clone(), window, cx)
                            .selectable(true),
                    );
                for (block_index, block) in blocks.into_iter().enumerate() {
                    let label = if block_index == 0 {
                        "insert into editor".to_string()
                    } else {
                        format!("insert block {} into editor", block_index + 1)
                    };
                    body = body.child(
                        div()
                            .id(("agent-insert-sql", index * 16 + block_index))
                            .px_2()
                            .py_0p5()
                            .rounded(px(3.))
                            .border_1()
                            .border_color(theme::border())
                            .text_xs()
                            .text_color(theme::text_dim())
                            .child(label)
                            .hover(|button| button.bg(theme::hover()).cursor_pointer())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_query_tab_with(&block, window, cx);
                            })),
                    );
                }
                body.into_any_element()
            }
        }
        ThreadEntry::Tool { title, status, .. } => div()
            .flex()
            .items_center()
            .gap_1()
            .text_xs()
            .text_color(theme::text_dim())
            .child(
                svg()
                    .path("icons/check-chain.svg")
                    .size(px(11.))
                    .text_color(if status == "completed" {
                        theme::success()
                    } else {
                        theme::text_dim()
                    }),
            )
            .child(format!("{title} ({status})"))
            .into_any_element(),
        ThreadEntry::Permission {
            title,
            input,
            options,
            answered,
        } => {
            let mut card = div()
                .p_2()
                .rounded(px(4.))
                .border_1()
                .border_color(theme::warning())
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_color(theme::warning())
                        .text_xs()
                        .child(format!("Permission: {title}")),
                )
                .when_some(input.clone(), |card, input| {
                    card.child(
                        div()
                            .text_xs()
                            .font_family("Menlo")
                            .text_color(theme::text_dim())
                            .child(input),
                    )
                });
            match answered {
                Some(choice) => {
                    card = card.child(
                        div()
                            .text_xs()
                            .text_color(theme::text_dim())
                            .child(format!("answered: {choice}")),
                    );
                }
                None => {
                    let mut row = div().flex().items_center().gap_2();
                    for (option_index, option) in options.iter().enumerate() {
                        let option_id = option.option_id.clone();
                        let label = if option.name.is_empty() {
                            option.option_id.clone()
                        } else {
                            option.name.clone()
                        };
                        row = row.child(
                            div()
                                .id(("agent-permission", index * 8 + option_index))
                                .px_2()
                                .py_1()
                                .rounded(px(3.))
                                .border_1()
                                .border_color(theme::border())
                                .text_color(theme::text())
                                .text_xs()
                                .child(label)
                                .hover(|button| button.bg(theme::hover()).cursor_pointer())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.agent_answer_permission(Some(option_id.clone()), cx);
                                })),
                        );
                    }
                    card = card.child(row);
                }
            }
            card.into_any_element()
        }
        ThreadEntry::Info(text) => div()
            .text_xs()
            .text_color(theme::text_dim())
            .child(text.clone())
            .into_any_element(),
    }
}
