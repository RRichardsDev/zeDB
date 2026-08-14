use gpui::{div, prelude::*, px, svg, Context};

use super::*;

impl Workspace {
    /// The Settings sync section of the preferences panel.
    pub(crate) fn settings_sync_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let enabled = self.preferences.settings_sync_url.is_some();
        let section = div()
            .py_3()
            .border_b_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap_1()
            .child("Settings sync")
            .child(div().text_sm().text_color(theme::text_dim()).child(
                "Preferences, connections, and custom agents in a git repo you own. \
                 Passwords never sync; they stay in this Mac's Keychain.",
            ));
        let section = match &self.settings_sync.status {
            Some((status, warning)) => section.child(
                div()
                    .text_sm()
                    .text_color(if *warning {
                        theme::danger()
                    } else {
                        theme::text_dim()
                    })
                    .child(status.clone()),
            ),
            None => section,
        };
        if enabled {
            let url = self
                .preferences
                .settings_sync_url
                .clone()
                .unwrap_or_default();
            section.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .mt_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .text_color(theme::text_dim())
                            .font_family("Menlo")
                            .child(url),
                    )
                    .child(
                        div()
                            .id("sync-now")
                            .px_3()
                            .py_1()
                            .rounded(px(3.))
                            .border_1()
                            .border_color(theme::border())
                            .text_color(theme::text())
                            .hover(|button| button.bg(theme::bg_sidebar()).cursor_pointer())
                            .on_click(cx.listener(|this, _, _, cx| this.settings_sync_tick(cx)))
                            .child("Sync now"),
                    )
                    .child(
                        div()
                            .id("sync-disable")
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
                            .on_click(cx.listener(|this, _, _, cx| this.settings_sync_disable(cx)))
                            .child("Disable"),
                    ),
            )
        } else {
            let signed_in_provider = match &self.github {
                crate::GithubAuth::SignedIn(profile) => Some(profile.provider),
                _ => None,
            };
            let section = section.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .mt_2()
                    .child(div().flex_1().child(self.settings_sync.url_input.clone()))
                    .child(
                        div()
                            .id("sync-enable")
                            .px_3()
                            .py_1()
                            .rounded(px(3.))
                            .border_1()
                            .border_color(theme::border())
                            .text_color(theme::text())
                            .hover(|button| button.bg(theme::bg_sidebar()).cursor_pointer())
                            .on_click(cx.listener(|this, _, _, cx| this.settings_sync_enable(cx)))
                            .child("Enable"),
                    )
                    .when_some(
                        signed_in_provider.filter(|_| self.settings_sync.bootstrap.is_none()),
                        |row, provider| {
                            row.child(
                                div()
                                    .id("sync-bootstrap")
                                    .px_3()
                                    .py_1()
                                    .rounded(px(3.))
                                    .border_1()
                                    .border_color(theme::border())
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_color(theme::text_dim())
                                    .hover(|button| {
                                        button
                                            .bg(theme::bg_sidebar())
                                            .text_color(theme::text())
                                            .cursor_pointer()
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.settings_sync_bootstrap(cx)
                                    }))
                                    .child("Create on")
                                    .child(
                                        svg()
                                            .path(provider.icon())
                                            .size(px(14.))
                                            .text_color(theme::text()),
                                    )
                                    .child(provider.name()),
                            )
                        },
                    ),
            );
            let section = match &self.settings_sync.switch_offer {
                Some(candidate) => {
                    let switch_url = candidate.clone();
                    section.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .mt_2()
                            .text_sm()
                            .text_color(theme::text_dim())
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(format!("Found {candidate} under this account.")),
                            )
                            .child(
                                div()
                                    .id("sync-switch-now")
                                    .px_3()
                                    .py_1()
                                    .rounded(px(3.))
                                    .border_1()
                                    .border_color(theme::border())
                                    .text_color(theme::text())
                                    .hover(|button| button.bg(theme::bg_sidebar()).cursor_pointer())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.settings_sync_enable_url(switch_url.clone(), cx);
                                    }))
                                    .child("Switch now"),
                            )
                            .child(
                                div()
                                    .id("sync-switch-dismiss")
                                    .px_2()
                                    .py_0p5()
                                    .rounded(px(3.))
                                    .text_color(theme::text_dim())
                                    .hover(|button| {
                                        button
                                            .bg(theme::bg_sidebar())
                                            .text_color(theme::text())
                                            .cursor_pointer()
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.settings_sync.switch_offer = None;
                                        cx.notify();
                                    }))
                                    .child("Not now"),
                            ),
                    )
                }
                None => section,
            };
            match &self.settings_sync.bootstrap {
                None => section,
                Some(Bootstrap::Authorizing { user_code }) => section
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap_2()
                            .mt_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .text_color(theme::text_dim())
                                    .child(
                                        "Approve the temporary repo access at \
                                     https://github.com/login/device (opened in your browser). \
                                     It's on your clipboard; click the code to copy it again. \
                                     The access is used once to find or create the repo and \
                                     is never kept.",
                                    ),
                            )
                            .child(
                                div()
                                    .id("bootstrap-cancel")
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
                                        this.settings_sync_bootstrap_cancel(cx)
                                    }))
                                    .child("Cancel"),
                            ),
                    )
                    .child(div().mt_2().child(self.device_code_boxes(
                        user_code.clone(),
                        "bootstrap-code",
                        cx,
                    ))),
                Some(Bootstrap::Working(message)) => section.child(
                    div()
                        .mt_2()
                        .text_sm()
                        .text_color(theme::text_dim())
                        .child(message.clone()),
                ),
                Some(Bootstrap::OfferLink { ssh_url }) => {
                    let link_url = ssh_url.clone();
                    section.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .mt_2()
                            .text_sm()
                            .text_color(theme::text_dim())
                            .child(div().flex_1().min_w_0().child(format!(
                                "Looks like you already have zedb-settings ({ssh_url})."
                            )))
                            .child(
                                div()
                                    .id("bootstrap-link")
                                    .px_3()
                                    .py_1()
                                    .rounded(px(3.))
                                    .border_1()
                                    .border_color(theme::border())
                                    .text_color(theme::text())
                                    .hover(|button| button.bg(theme::bg_sidebar()).cursor_pointer())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.settings_sync.bootstrap = None;
                                        this.settings_sync_enable_url(link_url.clone(), cx);
                                    }))
                                    .child("Link it"),
                            )
                            .child(
                                div()
                                    .id("bootstrap-no-link")
                                    .px_2()
                                    .py_0p5()
                                    .rounded(px(3.))
                                    .text_color(theme::text_dim())
                                    .hover(|button| {
                                        button
                                            .bg(theme::bg_sidebar())
                                            .text_color(theme::text())
                                            .cursor_pointer()
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.settings_sync_bootstrap_cancel(cx)
                                    }))
                                    .child("Not now"),
                            ),
                    )
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use zedb_core::sync::{run_sync_tick as run_tick, SyncTickResult as TickResult};
    use zedb_core::{sync, ConnectionConfig, ConnectionNode, EnvTier, Preferences};

    fn git_in(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            status.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&status.stderr)
        );
    }

    fn configure_identity(dir: &Path) {
        git_in(dir, &["config", "user.email", "test@example.com"]);
        git_in(dir, &["config", "user.name", "Test"]);
    }

    fn seed_stamp(preferences: &Preferences, connections: &[ConnectionConfig]) {
        sync::save_state(&sync::SyncState {
            last_synced_hash: Some(sync::content_hash(preferences, connections)),
            last_synced_at: Some(0),
        });
    }

    /// Machine A pushes its settings; a fresh machine B enables sync and
    /// inherits them instead of clobbering the repo with its defaults;
    /// B's later change comes back to A.
    #[test]
    fn two_machine_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let remote = tmp.path().join("remote.git");
        std::fs::create_dir(&remote).unwrap();
        git_in(&remote, &["init", "--bare", "-q", "--initial-branch=main"]);
        let remote_url = remote.to_str().unwrap();

        let connections = vec![ConnectionConfig {
            name: "staging".into(),
            nodes: vec![ConnectionNode {
                name: "Node 1".into(),
                endpoint: "http://ch-1.example:8123".into(),
            }],
            user: "default".into(),
            database: None,
            tier: EnvTier::Staging,
            read_only: true,
            driver: Default::default(),
            cloud: None,
        }];

        // Machine A: vim on, one connection; enable seeds the stamp.
        let config_a = tmp.path().join("config-a");
        unsafe { std::env::set_var("ZEDB_CONFIG_DIR", &config_a) };
        let checkout_a = tmp.path().join("a");
        zedb_core::git::clone_repo(remote_url, &checkout_a).unwrap();
        configure_identity(&checkout_a);
        let preferences_a = Preferences {
            vim_mode: true,
            ..Preferences::default()
        };
        seed_stamp(&preferences_a, &connections);
        match run_tick(&checkout_a, &preferences_a, &connections) {
            TickResult::Pushed { conflicted: false } => {}
            TickResult::Pushed { conflicted: true } => panic!("first push saw a conflict"),
            TickResult::UpToDate => panic!("expected a push, repo was empty"),
            TickResult::Applied(..) => panic!("nothing to apply from an empty repo"),
            TickResult::Failed(error) => panic!("push failed: {error}"),
        }

        // Machine B: fresh defaults; must inherit A's settings.
        let config_b = tmp.path().join("config-b");
        unsafe { std::env::set_var("ZEDB_CONFIG_DIR", &config_b) };
        let checkout_b = tmp.path().join("b");
        zedb_core::git::clone_repo(remote_url, &checkout_b).unwrap();
        configure_identity(&checkout_b);
        let preferences_b = Preferences::default();
        seed_stamp(&preferences_b, &[]);
        let inherited = match run_tick(&checkout_b, &preferences_b, &[]) {
            TickResult::Applied(payload, hash) => {
                sync::save_state(&sync::SyncState {
                    last_synced_hash: Some(hash),
                    last_synced_at: Some(0),
                });
                payload
            }
            other => panic!(
                "fresh machine must inherit, got {}",
                match other {
                    TickResult::UpToDate => "UpToDate",
                    TickResult::Pushed { .. } => "Pushed (defaults clobbered the repo!)",
                    TickResult::Failed(ref error) => error,
                    TickResult::Applied(..) => unreachable!(),
                }
            ),
        };
        assert!(inherited.preferences.vim_mode);
        assert_eq!(inherited.connections.len(), 1);

        // B turns vim off and pushes; A applies the change.
        let mut preferences_b = inherited.preferences.clone();
        preferences_b.vim_mode = false;
        match run_tick(&checkout_b, &preferences_b, &inherited.connections) {
            TickResult::Pushed { conflicted: false } => {}
            other => panic!(
                "B's change should push cleanly, got {}",
                match other {
                    TickResult::Failed(ref error) => error,
                    _ => "unexpected result",
                }
            ),
        }
        unsafe { std::env::set_var("ZEDB_CONFIG_DIR", &config_a) };
        match run_tick(&checkout_a, &preferences_a, &connections) {
            TickResult::Applied(payload, _) => assert!(!payload.preferences.vim_mode),
            _ => panic!("A should apply B's change"),
        }

        unsafe { std::env::remove_var("ZEDB_CONFIG_DIR") };
    }
}
