use crate::*;

use gpui::prelude::*;

impl Workspace {
    pub(crate) fn title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(36.))
            .flex_none()
            .w_full()
            .bg(theme::bg_sidebar())
            .border_b_1()
            .border_color(theme::border())
            .flex()
            .items_center()
            .pl(px(90.))
            .pr_3()
            .text_sm()
            .text_color(theme::text())
            .child("zeDB")
            .child(div().flex_1())
            .when_some(self.update_available.clone(), |bar, update| {
                let phase = self.update_phase;
                let (prefix, version) = match phase {
                    UpdatePhase::Available => {
                        ("Update available:", Some(format!("v{}", update.version)))
                    }
                    UpdatePhase::Installing => {
                        ("Downloading", Some(format!("v{}...", update.version)))
                    }
                    UpdatePhase::Ready => ("Restart to update", None),
                };
                bar.child(
                    div()
                        .id("update-available")
                        .px_2()
                        .py_0p5()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(theme::text_dim())
                        .text_xs()
                        .text_color(theme::text_dim())
                        .flex()
                        .items_center()
                        .gap_1()
                        .when(phase != UpdatePhase::Installing, |pill| {
                            pill.hover(|pill| {
                                pill.bg(theme::bg())
                                    .text_color(theme::text())
                                    .cursor_pointer()
                            })
                            .on_click(cx.listener(
                                move |this, _, _, cx| match this.update_phase {
                                    UpdatePhase::Available => this.start_update_install(cx),
                                    UpdatePhase::Ready => this.relaunch_updated(cx),
                                    UpdatePhase::Installing => {}
                                },
                            ))
                        })
                        .child(prefix)
                        .when_some(version, |pill, version| {
                            pill.child(div().text_color(theme::text()).child(version))
                        }),
                )
            })
            .when_some(
                match &self.github {
                    GithubAuth::SignedIn(profile) => Some(profile.clone()),
                    _ => None,
                },
                |toolbar, profile| {
                    let label = format!("@{} on GitHub", profile.login);
                    toolbar.child(
                        div()
                            .id("toolbar-profile")
                            .ml_2()
                            .mt(px(3.))
                            .size(px(28.))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .map(|button| match profile.avatar.clone() {
                                Some(avatar) => button.child(
                                    gpui::img(gpui::ImageSource::Resource(gpui::Resource::Path(
                                        avatar.into(),
                                    )))
                                    .size(px(24.))
                                    .rounded_full(),
                                ),
                                None => button.child(
                                    svg()
                                        .path("icons/github.svg")
                                        .size(px(18.))
                                        .text_color(theme::text_dim()),
                                ),
                            })
                            .tooltip(move |window, cx| {
                                gpui_component::tooltip::Tooltip::new(label.clone())
                                    .build(window, cx)
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.open_preferences(cx))),
                    )
                },
            )
    }

    pub(crate) fn start_update_install(&mut self, cx: &mut Context<Self>) {
        let Some(update) = self.update_available.clone() else {
            return;
        };
        // Bare-binary runs (cargo run) have nothing to swap; hand over to the
        // release page instead.
        if updates::current_bundle().is_none() || update.asset.is_none() {
            cx.open_url(&update.url);
            return;
        }
        self.update_phase = UpdatePhase::Installing;
        cx.notify();
        let task = rt::tokio().spawn(async move { updates::download_and_install(&update).await });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|error| Err(format!("update task failed: {error}")));
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => this.update_phase = UpdatePhase::Ready,
                    Err(error) => {
                        this.update_phase = UpdatePhase::Available;
                        this.flash_warning(format!("Update failed: {error}"), cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn session_snapshot(&self, cx: &Context<Self>) -> zedb_core::SavedSession {
        zedb_core::SavedSession {
            tabs: self
                .query
                .tabs
                .iter()
                .map(|tab| zedb_core::SavedQueryTab {
                    id: tab.persistent_id.clone(),
                    saved_tab_id: tab.saved_tab_id.clone(),
                    name: tab_display_name(tab),
                    sql: tab.editor.read(cx).value().to_string(),
                    connection: tab.connection.clone(),
                })
                .collect(),
            active_tab: self.query.active_tab,
        }
    }

    pub(crate) fn relaunch_updated(&mut self, cx: &mut Context<Self>) {
        // The quit hook would save too, but saving up front means a failure
        // can stop the restart instead of losing tabs.
        let session = self.session_snapshot(cx);
        if let Err(error) = zedb_core::save_session(&session) {
            // Losing open tabs is worse than delaying the restart.
            self.flash_warning(format!("Could not save open tabs: {error}"), cx);
            return;
        }
        if let Some(bundle) = updates::current_bundle() {
            let _ = std::process::Command::new("open")
                .arg("-n")
                .arg(bundle)
                .spawn();
        }
        cx.quit();
    }

    pub(crate) fn open_preferences(&mut self, cx: &mut Context<Self>) {
        self.connection.form = None;
        self.show_preferences = true;
        self.settings_sync_probe_existing(cx);
        cx.notify();
    }

    pub(crate) fn close_preferences(&mut self, cx: &mut Context<Self>) {
        self.show_preferences = false;
        self.settings_sync_tick(cx);
        cx.notify();
    }

    pub(crate) fn toggle_fleet(&mut self, cx: &mut Context<Self>) {
        self.show_fleet = !self.show_fleet;
        if self.show_fleet {
            self.show_query_editor = false;
            self.show_ops = false;
            if self.fleet.repo.is_none() && !self.fleet.repo_path.read(cx).text().trim().is_empty()
            {
                self.fleet_open_repo(cx);
            } else if self.fleet.rows.is_empty() {
                self.fleet_refresh(cx);
            }
        }
        cx.notify();
    }

    pub(crate) fn set_theme_preference(
        &mut self,
        preference: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preferences.theme = Some(preference.to_string());
        if let Err(error) = save_preferences(&self.preferences) {
            self.notice = Some(format!("Could not save preferences: {error}"));
        }
        apply_theme_preference(Some(preference), Some(window), cx);
        self.settings_sync_tick(cx);
        cx.notify();
    }

    /// System-mode follow-up: re-resolve on window refocus so a macOS
    /// appearance change is picked up promptly.
    pub(crate) fn theme_recheck(&mut self, cx: &mut Context<Self>) {
        if self.preferences.theme.as_deref() == Some("system") {
            let mode = resolve_theme_mode(Some("system"), cx);
            if mode.is_dark() != Theme::global(cx).is_dark() {
                apply_theme_preference(Some("system"), None, cx);
                cx.notify();
            }
        }
    }

    pub(crate) fn toggle_vim_mode(&mut self, cx: &mut Context<Self>) {
        self.preferences.vim_mode = !self.preferences.vim_mode;
        if self.preferences.vim_mode {
            for tab in &mut self.query.tabs {
                let editor = tab.editor.read(cx);
                let cursor = editor.cursor_position();
                tab.vim.reset(
                    editor.value().as_ref(),
                    cursor.line as usize,
                    cursor.character as usize,
                );
                tab.vim_command_line = None;
                tab.vim_recording = None;
            }
        }
        if let Err(error) = save_preferences(&self.preferences) {
            self.notice = Some(format!("Could not save preferences: {error}"));
        }
        cx.notify();
    }

    pub(crate) fn toggle_experimental_streaming_queries(&mut self, cx: &mut Context<Self>) {
        self.preferences.experimental_streaming_queries =
            !self.preferences.experimental_streaming_queries;
        if let Err(error) = save_preferences(&self.preferences) {
            self.notice = Some(format!("Could not save preferences: {error}"));
            self.notice_warning = true;
        } else {
            self.settings_sync_tick(cx);
        }
        cx.notify();
    }

    /// Start the GitHub device flow: browser opens, code shows in the
    /// panel, and we poll until approved.
    pub(crate) fn github_sign_in(&mut self, provider: github::Provider, cx: &mut Context<Self>) {
        if !provider.configured() {
            self.flash_warning(
                format!(
                    "{} sign-in isn't wired up in this build yet",
                    provider.name()
                ),
                cx,
            );
            return;
        }
        self.github_generation += 1;
        let generation = self.github_generation;
        let handle = rt::tokio().spawn(github::start_device_flow(provider));
        cx.spawn(async move |this, cx| {
            let device = match handle.await {
                Ok(Ok(device)) => device,
                Ok(Err(error)) => {
                    this.update(cx, |this, cx| this.flash_warning(error, cx))
                        .ok();
                    return;
                }
                Err(_) => return,
            };
            let poll_device = device.clone();
            let stale = this
                .update(cx, |this, cx| {
                    if this.github_generation != generation {
                        return true;
                    }
                    this.github = GithubAuth::Authorizing {
                        user_code: device.user_code.clone(),
                        verification_uri: device.verification_uri.clone(),
                    };
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                        device.user_code.clone(),
                    ));
                    cx.open_url(&device.verification_uri);
                    cx.notify();
                    false
                })
                .unwrap_or(true);
            if stale {
                return;
            }
            let token = rt::tokio()
                .spawn(async move { github::poll_for_token(provider, &poll_device).await })
                .await;
            let token = match token {
                Ok(Ok(token)) => token,
                Ok(Err(error)) => {
                    this.update(cx, |this, cx| {
                        if this.github_generation == generation {
                            this.github = GithubAuth::SignedOut;
                            this.flash_warning(error, cx);
                        }
                    })
                    .ok();
                    return;
                }
                Err(_) => return,
            };
            if let Err(error) = github::store_token(provider, &token) {
                this.update(cx, |this, cx| {
                    this.flash_warning(format!("Could not store the token: {error}"), cx)
                })
                .ok();
            }
            let profile = rt::tokio()
                .spawn(async move { github::fetch_profile(provider, &token).await })
                .await;
            this.update(cx, |this, cx| {
                if this.github_generation != generation {
                    return;
                }
                match profile {
                    Ok(Ok(profile)) => {
                        this.notice = Some(format!(
                            "Signed in to {} as {}",
                            provider.name(),
                            profile.login
                        ));
                        this.notice_warning = false;
                        this.github = GithubAuth::SignedIn(profile);
                        this.settings_sync_identity_changed(cx);
                    }
                    Ok(Err(error)) => {
                        this.github = GithubAuth::SignedOut;
                        this.flash_warning(error, cx);
                    }
                    Err(_) => this.github = GithubAuth::SignedOut,
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn github_sign_out(&mut self, cx: &mut Context<Self>) {
        let provider = match &self.github {
            GithubAuth::SignedIn(profile) => profile.provider,
            _ => github::Provider::GitHub,
        };
        github::clear_token(provider);
        self.github_generation += 1;
        self.github = GithubAuth::SignedOut;
        self.settings_sync_identity_changed(cx);
        self.notice = Some(format!(
            "Signed out of {} on this Mac; revoke zeDB under the {} application \
             settings if you want the grant gone too",
            provider.name(),
            provider.name(),
        ));
        self.notice_warning = false;
        cx.notify();
    }

    /// GitHub-style per-character boxes for a device code; clicking
    /// re-copies the code to the clipboard.
    pub(crate) fn device_code_boxes(
        &self,
        user_code: String,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .gap_1()
            .cursor_pointer()
            .hover(|boxes| boxes.opacity(0.85))
            .on_click({
                let code = user_code.clone();
                cx.listener(move |this, _, _, cx| {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(code.clone()));
                    this.notice = Some("Code copied to the clipboard".into());
                    this.notice_warning = false;
                    cx.notify();
                })
            })
            .children(user_code.chars().map(|character| {
                if character == '-' {
                    div()
                        .px_1()
                        .text_color(theme::text_dim())
                        .child("-")
                        .into_any_element()
                } else {
                    div()
                        .w(px(38.))
                        .h(px(46.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(theme::bg_sidebar())
                        .text_xl()
                        .font_family("Menlo")
                        .text_color(theme::text())
                        .child(String::from(character))
                        .into_any_element()
                }
            }))
    }

    /// The Account section of the preferences panel.
    pub(crate) fn account_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let row = div().py_3().border_b_1().border_color(theme::border());
        match &self.github {
            GithubAuth::SignedOut => {
                row.flex()
                    .flex_col()
                    .gap_1()
                    .child("Account")
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::text_dim())
                            .child("Optional: shows your profile and enables settings sync later"),
                    )
                    .child(div().flex().items_center().gap_2().mt_2().children(
                        github::Provider::ALL.into_iter().map(|provider| {
                            div()
                                .id(provider.name())
                                .px_3()
                                .py_1()
                                .rounded(px(3.))
                                .border_1()
                                .border_color(theme::border())
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_color(if provider.configured() {
                                    theme::text()
                                } else {
                                    theme::text_dim()
                                })
                                .hover(|button| button.bg(theme::bg_sidebar()).cursor_pointer())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.github_sign_in(provider, cx)
                                }))
                                .child(
                                    svg()
                                        .path(provider.icon())
                                        .size(px(16.))
                                        .text_color(theme::text()),
                                )
                                .child(format!("Sign in with {}", provider.name()))
                        }),
                    ))
            }
            GithubAuth::Authorizing {
                user_code,
                verification_uri,
            } => {
                let code_boxes = self.device_code_boxes(user_code.clone(), "github-code", cx);
                row.flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child("Account")
                            .child(
                                div()
                                    .id("github-cancel")
                                    .px_2()
                                    .py_1()
                                    .rounded(px(3.))
                                    .text_color(theme::text_dim())
                                    .hover(|button| {
                                        button
                                            .bg(theme::bg_sidebar())
                                            .text_color(theme::text())
                                            .cursor_pointer()
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.github_generation += 1;
                                        this.github = GithubAuth::SignedOut;
                                        cx.notify();
                                    }))
                                    .child("Cancel"),
                            ),
                    )
                    .child(div().text_sm().text_color(theme::text_dim()).child(format!(
                        "Enter this code at {verification_uri} (opened in your browser). \
                         It's on your clipboard; click the code to copy it again."
                    )))
                    .child(code_boxes)
            }
            GithubAuth::SignedIn(profile) => {
                let display = profile
                    .name
                    .clone()
                    .unwrap_or_else(|| profile.login.clone());
                row.flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .when_some(profile.avatar.clone(), |account, avatar| {
                                account.child(
                                    gpui::img(gpui::ImageSource::Resource(gpui::Resource::Path(
                                        avatar.into(),
                                    )))
                                    .size(px(36.))
                                    .rounded_full(),
                                )
                            })
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_0p5()
                                    .child(div().text_color(theme::text()).child(display))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .text_sm()
                                            .text_color(theme::text_dim())
                                            .child(
                                                svg()
                                                    .path(profile.provider.icon())
                                                    .size(px(13.))
                                                    .text_color(theme::text_dim()),
                                            )
                                            .child(format!("@{}", profile.login)),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .id("github-sign-out")
                            .px_3()
                            .py_1()
                            .rounded(px(3.))
                            .border_1()
                            .border_color(theme::border())
                            .text_color(theme::text_dim())
                            .hover(|button| {
                                button
                                    .bg(theme::bg_sidebar())
                                    .text_color(theme::text())
                                    .cursor_pointer()
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.github_sign_out(cx)))
                            .child("Sign out"),
                    )
            }
        }
    }

    pub(crate) fn preferences_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().flex().justify_center().child(
            div()
                .w(px(680.))
                .max_w_full()
                .p_6()
                .flex()
                .flex_col()
                .gap_5()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(div().text_xl().child("Preferences"))
                        .child(
                            div()
                                .id("close-preferences")
                                .px_3()
                                .py_1()
                                .rounded(px(3.))
                                .border_1()
                                .border_color(theme::border())
                                .text_color(theme::text_dim())
                                .hover(|button| {
                                    button
                                        .bg(theme::bg_sidebar())
                                        .text_color(theme::text())
                                        .cursor_pointer()
                                })
                                .on_click(cx.listener(|this, _, _, cx| this.close_preferences(cx)))
                                .child("Done"),
                        ),
                )
                .child(self.account_section(cx))
                .child(self.settings_sync_section(cx))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_4()
                        .py_3()
                        .border_b_1()
                        .border_color(theme::border())
                        .child(
                            div().flex().flex_col().gap_1().child("Theme").child(
                                div()
                                    .text_sm()
                                    .text_color(theme::text_dim())
                                    .child("System follows macOS appearance."),
                            ),
                        )
                        .child(div().flex().items_center().gap_2().children(
                            [("dark", "Dark"), ("light", "Light"), ("system", "System")].map(
                                |(value, label)| {
                                    let active =
                                        self.preferences.theme.as_deref().unwrap_or("dark")
                                            == value;
                                    div()
                                        .id(label)
                                        .px_3()
                                        .py_1()
                                        .rounded(px(3.))
                                        .border_1()
                                        .border_color(if active {
                                            theme::accent()
                                        } else {
                                            theme::border()
                                        })
                                        .text_color(if active {
                                            theme::text()
                                        } else {
                                            theme::text_dim()
                                        })
                                        .hover(|button| button.bg(theme::hover()).cursor_pointer())
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.set_theme_preference(value, window, cx)
                                        }))
                                        .child(label)
                                },
                            ),
                        )),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_4()
                        .py_3()
                        .border_b_1()
                        .border_color(theme::border())
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .gap_1()
                                .child("Vim mode")
                                .child(
                                    div()
                                        .text_sm()
                                        .whitespace_normal()
                                        .text_color(theme::text_dim())
                                        .child("Use Vim keybindings in query editors."),
                                ),
                        )
                        .child(
                            div()
                                .id("toggle-vim-mode")
                                .w(px(54.))
                                .h(px(28.))
                                .flex_none()
                                .px_1()
                                .rounded_full()
                                .flex()
                                .items_center()
                                .when(self.preferences.vim_mode, |toggle| {
                                    toggle.justify_end().bg(theme::toggle_on())
                                })
                                .when(!self.preferences.vim_mode, |toggle| {
                                    toggle.justify_start().bg(theme::toggle_off())
                                })
                                .hover(|toggle| toggle.cursor_pointer())
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_vim_mode(cx)))
                                .child(div().size(px(20.)).rounded_full().bg(
                                    if self.preferences.vim_mode {
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
                        .items_center()
                        .justify_between()
                        .gap_4()
                        .py_3()
                        .border_b_1()
                        .border_color(theme::border())
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .gap_1()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child("Experimental STREAM tails")
                                        .child(
                                            svg()
                                                .path("icons/experimental.svg")
                                                .size(px(14.))
                                                .text_color(theme::warning()),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .whitespace_normal()
                                        .text_color(theme::text_dim())
                                        .child(
                                            "Prefer ClickHouse 26.6 STREAM CURSOR for compatible instant tails. Falls back to WATCH and polling.",
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .id("toggle-experimental-streaming-queries")
                                .w(px(54.))
                                .h(px(28.))
                                .flex_none()
                                .px_1()
                                .rounded_full()
                                .flex()
                                .items_center()
                                .when(
                                    self.preferences.experimental_streaming_queries,
                                    |toggle| toggle.justify_end().bg(theme::toggle_on()),
                                )
                                .when(
                                    !self.preferences.experimental_streaming_queries,
                                    |toggle| toggle.justify_start().bg(theme::toggle_off()),
                                )
                                .hover(|toggle| toggle.cursor_pointer())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_experimental_streaming_queries(cx)
                                }))
                                .child(div().size(px(20.)).rounded_full().bg(
                                    if self.preferences.experimental_streaming_queries {
                                        theme::toggle_knob_on()
                                    } else {
                                        theme::toggle_knob_off()
                                    },
                                )),
                        ),
                ),
        )
    }

    pub(crate) fn tier_colors(tier: EnvTier) -> (u32, u32) {
        if theme::is_dark() {
            match tier {
                EnvTier::Dev => (0x28384b, 0x8ab4d4),
                EnvTier::Staging => (0x463b28, 0xc7a969),
                EnvTier::Production => (0x472d31, 0xd4868d),
            }
        } else {
            match tier {
                EnvTier::Dev => (0xd6e6f5, 0x2f5f8a),
                EnvTier::Staging => (0xf3e9d2, 0x8a6d1f),
                EnvTier::Production => (0xf5dcdf, 0xa03744),
            }
        }
    }

    pub(crate) fn tier_badge(tier: EnvTier) -> impl IntoElement {
        let (background, foreground) = Self::tier_colors(tier);
        div()
            .px_2()
            .py(px(2.))
            .rounded(px(3.))
            .border_1()
            .border_color(rgb(foreground))
            .bg(rgb(background))
            .text_color(rgb(foreground))
            .text_xs()
            .child(tier.label().to_uppercase())
    }

    /// Half-scale badge for the dense connections list.
    pub(crate) fn tier_badge_small(tier: EnvTier) -> impl IntoElement {
        let (background, foreground) = Self::tier_colors(tier);
        div()
            .px(px(4.5))
            .py(px(1.))
            .rounded(px(2.))
            .border_1()
            .border_color(rgb(foreground))
            .bg(rgb(background))
            .text_color(rgb(foreground))
            .text_size(px(8.))
            .child(tier.label().to_uppercase())
    }

    /// The connection's write posture, worn next to the tier: quiet
    /// when read-only (the safe default), loud when writes are open.
    /// This is the half-scale form for the dense connections list; use
    /// `write_badge_sized(read_only, false)` for a full-size one.
    pub(crate) fn write_badge_small(read_only: bool) -> impl IntoElement {
        Self::write_badge_sized(read_only, true)
    }

    pub(crate) fn write_badge_sized(read_only: bool, small: bool) -> impl IntoElement {
        div()
            .map(|badge| {
                if small {
                    badge
                        .px(px(4.5))
                        .py(px(1.))
                        .rounded(px(2.))
                        .text_size(px(8.))
                } else {
                    badge.px_2().py(px(2.)).rounded(px(3.)).text_xs()
                }
            })
            .border_1()
            .map(|badge| {
                if read_only {
                    badge
                        .bg(theme::row_hover())
                        .border_color(theme::text_dim())
                        .text_color(theme::text_dim())
                        .child("READ-ONLY")
                } else {
                    badge
                        .bg(rgb(if theme::is_dark() { 0x4d2c2c } else { 0xf7dfd9 }))
                        .border_color(theme::alert())
                        .text_color(theme::alert())
                        .child("WRITE")
                }
            })
    }

    /// The at-rest environment mark: a small triangle in the tier's
    /// accent color, shown when a connection row is not hovered.
    pub(crate) fn tier_glyph(tier: EnvTier) -> impl IntoElement {
        let (_, foreground) = Self::tier_colors(tier);
        div()
            .text_size(px(9.))
            .text_color(rgb(foreground))
            .child("\u{25B2}")
    }

    /// The at-rest write-posture mark: a small square, dim when
    /// read-only, alert-colored when writes are open.
    pub(crate) fn write_glyph(read_only: bool) -> impl IntoElement {
        let color = if read_only {
            theme::text_dim()
        } else {
            theme::alert()
        };
        div().text_size(px(8.)).text_color(color).child("\u{25A0}")
    }
}
