use crate::export::{ExportFormat, ExportStep};
use crate::*;

use gpui::prelude::*;

impl Workspace {
    pub(crate) fn export_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(export) = self.export.as_ref() else {
            return div().into_any_element();
        };
        let running = export.running;
        let max_rows_label = self
            .query
            .tabs
            .get(self.query.active_tab)
            .map(|tab| tab.max_rows)
            .map(|max_rows| (max_rows.label().to_string(), max_rows.limit()))
            .unwrap_or_else(|| ("100k".into(), Some(100_000)));

        let button = |id: &'static str, primary: bool| {
            div()
                .id(id)
                .px_3()
                .py_1()
                .rounded(px(3.))
                .map(move |button| {
                    if primary {
                        button
                            .bg(theme::primary())
                            .text_color(theme::primary_foreground())
                    } else {
                        button
                            .border_1()
                            .border_color(theme::border())
                            .text_color(theme::text())
                    }
                })
        };

        let body: gpui::AnyElement = match export.step {
            ExportStep::Scope => {
                let (cap_label, cap) = max_rows_label;
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::text_dim())
                            .child("How much of the result?"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                button("export-scope-capped", true)
                                    .hover(|button| {
                                        button.bg(theme::primary_hover()).cursor_pointer()
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(export) = this.export.as_mut() {
                                            export.row_cap = cap;
                                            export.step = ExportStep::Configure;
                                        }
                                        cx.notify();
                                    }))
                                    .child(format!("Current max rows ({cap_label})")),
                            )
                            .child(
                                button("export-scope-all", false)
                                    .hover(|button| button.bg(theme::hover()).cursor_pointer())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(export) = this.export.as_mut() {
                                            export.row_cap = None;
                                            export.step = ExportStep::Configure;
                                        }
                                        cx.notify();
                                    }))
                                    .child("All rows"),
                            ),
                    )
                    .into_any_element()
            }
            ExportStep::Configure => {
                let format = export.format;
                let editing = export.editing_path;
                let path_text = export.path_input.read(cx).text();
                let path_display = path_text.replace(
                    &dirs::home_dir()
                        .map(|home| home.to_string_lossy().to_string())
                        .unwrap_or_default(),
                    "~",
                );
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().flex().items_center().gap_2().children(
                        ExportFormat::ALL.into_iter().map(|option| {
                            let selected = option == format;
                            div()
                                .id(option.label())
                                .px_2()
                                .py_0p5()
                                .rounded(px(3.))
                                .border_1()
                                .map(|chip| {
                                    if selected {
                                        chip.border_color(theme::accent()).text_color(theme::text())
                                    } else {
                                        chip.border_color(theme::border())
                                            .text_color(theme::text_dim())
                                    }
                                })
                                .when(!running, |chip| {
                                    chip.hover(|chip| chip.bg(theme::hover()).cursor_pointer())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.export_set_format(option, cx)
                                        }))
                                })
                                .when(running, |chip| chip.opacity(0.5))
                                .child(option.label())
                        }),
                    ))
                    .map(|panel| {
                        if editing {
                            panel.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(div().w(px(400.)).child(export.path_input.clone()))
                                    .child(
                                        div()
                                            .id("export-browse")
                                            .flex_none()
                                            .size(px(24.))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded(px(3.))
                                            .child(
                                                gpui::svg()
                                                    .path("icons/folder-open.svg")
                                                    .size(px(13.))
                                                    .text_color(theme::text_dim()),
                                            )
                                            .hover(|button| {
                                                button.bg(theme::hover()).cursor_pointer()
                                            })
                                            .tooltip(|window, cx| {
                                                gpui_component::tooltip::Tooltip::new(
                                                    "Choose location",
                                                )
                                                .build(window, cx)
                                            })
                                            .on_click(
                                                cx.listener(|this, _, _, cx| {
                                                    this.export_browse(cx)
                                                }),
                                            ),
                                    ),
                            )
                        } else {
                            // The quiet default: readable, editable on
                            // request, never a demand.
                            panel.child(
                                div()
                                    .id("export-path")
                                    .text_xs()
                                    .text_color(theme::text_dim())
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(if running {
                                        format!("to {path_display}")
                                    } else {
                                        format!("to {path_display} (click to change)")
                                    })
                                    .when(!running, |path| {
                                        path.hover(|path| {
                                            path.text_color(theme::text()).cursor_pointer()
                                        })
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                if let Some(export) = this.export.as_mut() {
                                                    export.editing_path = true;
                                                }
                                                cx.notify();
                                            }),
                                        )
                                    }),
                            )
                        }
                    })
                    .when(running, |panel| {
                        let rate = export
                            .started_at
                            .map(|started| started.elapsed().as_secs_f64())
                            .filter(|secs| *secs > 0.2)
                            .map(|secs| export.progress_bytes as f64 / secs)
                            .map(|rate| format!(" ({}/s)", Self::format_bytes(rate as u64)))
                            .unwrap_or_default();
                        panel.child(div().text_xs().text_color(theme::text_dim()).child(format!(
                            "exporting\u{2026} {}{rate}",
                            Self::format_bytes(export.progress_bytes)
                        )))
                    })
                    .when_some(export.error.clone(), |panel, error| {
                        panel.child(div().text_xs().text_color(theme::danger()).child(error))
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .pt_1()
                            .child(
                                button("export-cancel", false)
                                    .hover(|button| button.bg(theme::hover()).cursor_pointer())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.export_cancel(cx);
                                    }))
                                    .child("Cancel"),
                            )
                            .child(
                                button("export-go", true)
                                    .when(!running, |button| {
                                        button
                                            .hover(|button| {
                                                button.bg(theme::primary_hover()).cursor_pointer()
                                            })
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.export_start(cx)),
                                            )
                                    })
                                    .child(if running {
                                        "Exporting\u{2026}"
                                    } else {
                                        "Export"
                                    }),
                            ),
                    )
                    .into_any_element()
            }
        };

        gpui::deferred(
            div()
                .id("export-backdrop")
                .occlude()
                .absolute()
                .inset_0()
                .bg(gpui::rgba(0x00000088))
                .flex()
                .items_center()
                .justify_center()
                .on_click(cx.listener(|this, _, _, cx| {
                    let running = this
                        .export
                        .as_ref()
                        .map(|export| export.running)
                        .unwrap_or(false);
                    if !running {
                        this.export = None;
                        cx.notify();
                    }
                }))
                .child(
                    div()
                        .id("export-dialog")
                        .occlude()
                        .w(px(460.))
                        .p_3()
                        .rounded(px(6.))
                        .bg(theme::bg_sidebar())
                        .border_1()
                        .border_color(theme::border())
                        .flex()
                        .flex_col()
                        .gap_2()
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme::text())
                                .child("Export current query results"),
                        )
                        .child(body),
                ),
        )
        .into_any_element()
    }
}
