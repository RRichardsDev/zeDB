/// Fenced code blocks in a markdown reply, for insert-into-editor;
/// untagged and sql/clickhouse-tagged fences count.
use super::*;

pub(super) fn fenced_sql_blocks(text: &str) -> Vec<String> {
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

pub(super) fn render_entry(
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
