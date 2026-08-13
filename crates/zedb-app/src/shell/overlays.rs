use crate::*;

use gpui::prelude::*;

impl Workspace {
    /// Check for updates on demand (the menu item); the periodic loop
    /// stays quiet when nothing is newer, this says so out loud.
    pub(crate) fn check_for_updates_now(&mut self, cx: &mut Context<Self>) {
        self.notice = Some("Checking for updates...".into());
        self.notice_warning = false;
        cx.notify();
        let handle = rt::tokio().spawn(updates::check());
        cx.spawn(async move |this, cx| {
            let update = handle.await.ok().flatten();
            this.update(cx, |this, cx| {
                match update {
                    Some(update) => {
                        this.notice = Some(format!(
                            "v{} is available; click the update pill in the title bar to install",
                            update.version
                        ));
                        if this.update_phase == UpdatePhase::Available {
                            this.update_available = Some(update);
                        }
                    }
                    None => {
                        this.notice = Some(format!(
                            "No newer release found; you are on v{}",
                            env!("CARGO_PKG_VERSION")
                        ));
                    }
                }
                this.notice_warning = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn about_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let version = env!("CARGO_PKG_VERSION");
        let commit = option_env!("ZEDB_BUILD_COMMIT").unwrap_or("dev");
        let build = option_env!("ZEDB_BUILD_NUMBER").unwrap_or("0");
        let full_version = format!("{version}+build.{build}.{commit}");
        let copy_text = full_version.clone();
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
                    .w(px(560.))
                    .p_5()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::bg_sidebar())
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(
                        // Explicit Embedded: a bare filename parses as
                        // a relative URI, so From<&str> routes it to
                        // the HTTP client and it never loads.
                        img(gpui::ImageSource::Resource(gpui::Resource::Embedded(
                            "about-logo.png".into(),
                        )))
                        .size(px(96.)),
                    )
                    .child(
                        div()
                            .text_xl()
                            .text_color(theme::text())
                            .child(format!("zeDB {version}")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::text_dim())
                            .mt_2()
                            .child("Commit"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_family("Menlo")
                            .text_color(theme::text())
                            .child(commit),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::text_dim())
                            .mt_2()
                            .child("Version"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_family("Menlo")
                            .text_color(theme::text())
                            .child(full_version),
                    )
                    .child(
                        div()
                            .mt_4()
                            .w_full()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .id("about-ok")
                                    .flex_1()
                                    .py_1()
                                    .rounded(px(4.))
                                    .border_1()
                                    .border_color(theme::border())
                                    .text_center()
                                    .text_color(theme::text())
                                    .child("OK")
                                    .hover(|button| button.bg(theme::hover()).cursor_pointer())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.show_about = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("about-copy")
                                    .flex_1()
                                    .py_1()
                                    .rounded(px(4.))
                                    .border_1()
                                    .border_color(theme::border())
                                    .text_center()
                                    .text_color(theme::text())
                                    .child("Copy")
                                    .hover(|button| button.bg(theme::hover()).cursor_pointer())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                            copy_text.clone(),
                                        ));
                                        this.notice = Some("Version copied".into());
                                        this.notice_warning = false;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
    }
}
