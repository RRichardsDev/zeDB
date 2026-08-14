use gpui::{div, prelude::*, px, Context};
use gpui_component::input::{Input, InputState};

use super::*;
use crate::author::RollbackChoice;
use crate::theme;

impl Workspace {
    pub(crate) fn author_panel(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let author = self.author.as_ref()?;
        let number = author.number;
        let editing_existing = author.existing.is_some();
        let readonly = author.applied_on > 0;
        let applied_on = author.applied_on;
        let status_known = author.status_known;
        let checking = author.checking;
        let harness_phase = author.harness_phase;
        let saveable = author.saveable(cx) && !readonly;
        let targeted = author.targeted;
        let choice = author.rollback_choice;
        let with_rollback = choice != RollbackChoice::NoFile;
        let check_errors = author.check_errors.clone();
        let stale_check = author.check_errors.is_some() && !saveable && !checking && !readonly;

        let mut classes = div().flex().items_center().gap_1();
        for option in RollbackChoice::ALL {
            let selected = option == choice;
            classes = classes.child(
                div()
                    .id(gpui::SharedString::from(format!(
                        "author-class-{}",
                        option.label()
                    )))
                    .px_2()
                    .py_1()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(if selected {
                        theme::text()
                    } else {
                        theme::border()
                    })
                    .text_color(if selected {
                        theme::text()
                    } else {
                        theme::text_dim()
                    })
                    .child(option.label())
                    .when(!readonly, |button| {
                        button
                            .hover(|button| button.bg(theme::hover()).cursor_pointer())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.author_set_rollback_choice(option, window, cx);
                            }))
                    }),
            );
        }

        let editor = move |label: &'static str, state: &Entity<InputState>, height: f32| {
            div()
                .flex_none()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_color(theme::text_dim()).text_xs().child(label))
                .child(
                    div()
                        .h(px(height))
                        .rounded(px(3.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(theme::bg())
                        .child(
                            Input::new(state)
                                .appearance(false)
                                .bordered(false)
                                .focus_bordered(false)
                                .disabled(readonly)
                                .pl(px(4.))
                                .h_full(),
                        ),
                )
        };

        let mut body = div()
            .id("author-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(div().text_color(theme::text_dim()).child("rollback:"))
                    .child(classes)
                    .child(div().w(px(1.)).h(px(18.)).bg(theme::border()).mx_2())
                    .child(div().text_color(theme::text_dim()).child("scope:"))
                    .child(
                        div()
                            .id("author-targeted")
                            .px_2()
                            .py_1()
                            .rounded(px(3.))
                            .border_1()
                            .border_color(if targeted {
                                theme::text()
                            } else {
                                theme::border()
                            })
                            .text_color(if targeted {
                                theme::text()
                            } else {
                                theme::text_dim()
                            })
                            .child(if targeted {
                                "targeted (opt-in)"
                            } else {
                                "chain migration"
                            })
                            .when(!readonly, |button| {
                                button
                                    .hover(|button| button.bg(theme::hover()).cursor_pointer())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(author) = &mut this.author {
                                            author.targeted = !author.targeted;
                                        }
                                        cx.notify();
                                    }))
                            }),
                    ),
            )
            .when(editing_existing && !readonly && !status_known, |body| {
                body.child(div().text_color(theme::warning()).text_xs().child(
                    "No fleet status loaded; confirm this migration has never been \
                     applied anywhere before editing it.",
                ))
            })
            .child(editor("upgrade.sql", &author.upgrade, 200.));
        if with_rollback {
            body = body.child(editor("rollback.sql", &author.rollback, 140.));
        }
        if let Some(errors) = &check_errors {
            if errors.is_empty() {
                body = body.child(div().text_color(theme::success()).child(if stale_check {
                    "Checks passed, but the draft changed since; check again."
                } else {
                    "Checks passed against the pinned server."
                }));
            } else {
                let mut list = div()
                    .p_2()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(theme::danger())
                    .text_color(theme::danger())
                    .text_xs()
                    .font_family("Menlo")
                    .flex()
                    .flex_col()
                    .gap_1();
                for error in errors {
                    list = list.child(error.clone());
                }
                body = body.child(list);
            }
        }

        let footer = div()
            .flex_none()
            .px_3()
            .py_2()
            .border_t_1()
            .border_color(theme::border())
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .id("author-check")
                    .px_3()
                    .py_1()
                    // One width fits every state (Check, 43%, Verifying,
                    // Checking), so the footer never jitters as the
                    // button moves through them.
                    .w(px(118.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_1p5()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(theme::border())
                    .text_color(if checking { theme::text_dim() } else { theme::text() })
                    .map(|button| match harness_phase {
                        // Fetching the harness: the button's background
                        // fills left to right with the download.
                        Some(zedb_ch::pin::PinPhase::Downloading { received, total }) => {
                            let fraction = total
                                .filter(|total| *total > 0)
                                .map(|total| received as f32 / total as f32)
                                .unwrap_or(0.0);
                            button
                                .relative()
                                .child(
                                    div()
                                        .absolute()
                                        .left_0()
                                        .top_0()
                                        .bottom_0()
                                        .w(gpui::relative(fraction.clamp(0.02, 1.0)))
                                        .rounded(px(3.))
                                        // Green, but quiet enough that the
                                        // label stays readable on top.
                                        .bg(theme::success().opacity(0.35)),
                                )
                                .child(div().relative().child(match total {
                                    // The label is the number; the tooltip
                                    // keeps the words and the bytes.
                                    Some(_) => format!("{:.0}%", fraction * 100.0),
                                    None => "Downloading\u{2026}".to_string(),
                                }))
                                .tooltip(move |window, cx| {
                                    gpui_component::tooltip::Tooltip::new(match total {
                                        Some(total) => format!(
                                            "Getting a ClickHouse harness to check against \
                                             \u{b7} {} of {}",
                                            Workspace::format_bytes(received),
                                            Workspace::format_bytes(total),
                                        ),
                                        None => {
                                            "Getting a ClickHouse harness to check against".into()
                                        }
                                    })
                                    .build(window, cx)
                                })
                        }
                        // The stall between download and first use:
                        // macOS assessing the fresh binary.
                        Some(zedb_ch::pin::PinPhase::Verifying) => button
                            .child(
                                gpui::svg()
                                    .path("icons/lock.svg")
                                    .size(px(13.))
                                    .text_color(theme::text_dim()),
                            )
                            .child("Verifying\u{2026}")
                            .tooltip(|window, cx| {
                                gpui_component::tooltip::Tooltip::new(
                                    "macOS is verifying the ClickHouse harness \
                                     (first run only)",
                                )
                                .build(window, cx)
                            }),
                        None if checking => {
                            use gpui::{percentage, Animation, AnimationExt as _, Transformation};
                            use gpui_component::Sizable as _;
                            button.child(
                                gpui_component::Icon::empty()
                                    .path("icons/hourglass.svg")
                                    .with_size(gpui_component::Size::Small)
                                    .text_color(theme::text_dim())
                                    .with_animation(
                                        "author-check-spin",
                                        Animation::new(std::time::Duration::from_secs(1)).repeat(),
                                        |icon, delta| {
                                            icon.transform(Transformation::rotate(percentage(
                                                delta,
                                            )))
                                        },
                                    ),
                            )
                            .child("Checking...")
                        }
                        None => button.child("Check"),
                    })
                    .hover(|button| button.bg(theme::hover()).cursor_pointer())
                    .on_click(cx.listener(|this, _, _, cx| this.author_check(cx))),
            )
            .when(!readonly, |footer| {
                footer.child(
                    div()
                        .id("author-save")
                        .px_3()
                        .py_1()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(if saveable { theme::success() } else { theme::border() })
                        .text_color(if saveable { theme::success() } else { theme::text_dim() })
                        .child("Save to repo")
                        .when(saveable, |button| {
                            button
                                .hover(|button| button.bg(theme::hover()).cursor_pointer())
                                .on_click(cx.listener(|this, _, _, cx| this.author_save(cx)))
                        })
                        .when(!saveable, |button| {
                            button.tooltip(|window, cx| {
                                gpui_component::tooltip::Tooltip::new(
                                    "Run Check on the current draft first; saving requires a clean check",
                                )
                                .build(window, cx)
                            })
                        }),
                )
            })
            .child(div().flex_1())
            .child(
                div()
                    .id("author-cancel")
                    .px_3()
                    .py_1()
                    .rounded(px(3.))
                    .text_color(theme::text_dim())
                    .child(if readonly {
                        "Close"
                    } else if editing_existing {
                        "Discard changes"
                    } else {
                        "Discard draft"
                    })
                    .hover(|button| button.bg(theme::hover()).cursor_pointer())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.author = None;
                        cx.notify();
                    })),
            );

        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::rgba(0x00000088))
                .occlude()
                .child(
                    div()
                        .w(px(760.))
                        .max_h(px(620.))
                        .flex()
                        .flex_col()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(theme::bg_sidebar())
                        .child(
                            div()
                                .flex_none()
                                .px_3()
                                .py_2()
                                .border_b_1()
                                .border_color(theme::border())
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(if readonly {
                                    format!("Migration {number:05}")
                                } else if editing_existing {
                                    format!("Edit migration {number:05}")
                                } else {
                                    format!("New migration {number:05}")
                                })
                                .child(div().text_color(theme::warning()).text_xs().child(if readonly {
                                    format!(
                                        "Applied on {applied_on} database(s); history is \
                                         immutable, so this is read-only"
                                    )
                                } else {
                                    "Draft only: nothing is written until checks pass and you save"
                                        .to_string()
                                })),
                        )
                        .child(body)
                        .child(footer),
                )
                .into_any_element(),
        )
    }
}
