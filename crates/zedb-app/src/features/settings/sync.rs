//! Settings sync over a plain git repo (docs/PHASE-3.4.md M1).
//!
//! A sync tick runs on launch, window refocus, closing Preferences, and
//! after connection edits: pull, compare the local settings against the
//! repo payload, then apply the repo's version or push ours. Both sides
//! changing since the last sync is last-writer-wins (local pushes; the
//! repo's version stays in git history).

#[path = "sync/view.rs"]
mod view;

use std::path::PathBuf;

use gpui::{prelude::*, Context, Entity};
use zedb_core::{git, sync};

use crate::components::text_input::TextInput;
use crate::github;
use crate::rt;
use crate::theme;
use crate::Workspace;

pub struct SettingsSyncState {
    pub url_input: Entity<TextInput>,
    pub busy: bool,
    pub generation: u64,
    /// The quiet status line: (text, is_warning).
    pub status: Option<(String, bool)>,
    /// The one-click create/link flow for a GitHub zedb-settings repo.
    pub bootstrap: Option<Bootstrap>,
    pub bootstrap_generation: u64,
    /// Whether the one-shot probe for an existing zedb-settings repo ran.
    pub probed: bool,
    /// The URL the probe prefilled, so an identity change can clear it
    /// without touching anything the user typed themselves.
    pub probed_url: Option<String>,
    /// A zedb-settings repo found under the newly signed-in provider
    /// while the input still holds the old provider's URL.
    pub switch_offer: Option<String>,
}

/// Where the repo bootstrap is. The elevated `repo`-scoped token lives
/// only inside the driver's future, never in this state and never on
/// disk.
pub enum Bootstrap {
    /// Waiting for the user to approve the elevated access.
    Authorizing { user_code: String },
    /// Talking to the GitHub API.
    Working(String),
    /// A zedb-settings repo already exists; offer to link it.
    OfferLink { ssh_url: String },
}

impl SettingsSyncState {
    pub fn new(cx: &mut Context<Workspace>) -> Self {
        let url_input =
            cx.new(|cx| TextInput::new("", "git@github.com:you/zedb-settings.git", false, cx));
        Self {
            url_input,
            busy: false,
            generation: 0,
            status: None,
            bootstrap: None,
            bootstrap_generation: 0,
            probed: false,
            probed_url: None,
            switch_offer: None,
        }
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

impl Workspace {
    fn settings_sync_root(&self) -> Option<PathBuf> {
        self.preferences
            .settings_sync_repo
            .as_deref()
            .map(PathBuf::from)
            .filter(|path| path.join(".git").exists())
    }

    /// One reconcile pass; quiet unless something happened.
    pub(crate) fn settings_sync_tick(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.settings_sync_root() else {
            return;
        };
        if self.settings_sync.busy {
            return;
        }
        self.settings_sync.busy = true;
        self.settings_sync.generation += 1;
        let generation = self.settings_sync.generation;
        let preferences = self.preferences.clone();
        let connections = self.connection.connections.clone();
        let handle = rt::tokio()
            .spawn(async move { sync::run_sync_tick(&root, &preferences, &connections) });
        cx.spawn(async move |this, cx| {
            let result = handle.await;
            this.update(cx, |this, cx| {
                this.settings_sync.busy = false;
                if this.settings_sync.generation != generation {
                    return;
                }
                match result {
                    Ok(sync::SyncTickResult::UpToDate) => {
                        this.settings_sync.status = Some(("Synced".into(), false));
                    }
                    Ok(sync::SyncTickResult::Applied(payload, hash)) => {
                        this.settings_sync_apply(*payload, hash, cx);
                    }
                    Ok(sync::SyncTickResult::Pushed { conflicted }) => {
                        this.settings_sync.status = Some((
                            if conflicted {
                                "Pushed local settings; the repo's version stays in git history"
                                    .into()
                            } else {
                                "Pushed local settings".into()
                            },
                            false,
                        ));
                    }
                    Ok(sync::SyncTickResult::Failed(error)) => {
                        this.settings_sync.status = Some((error, true));
                    }
                    Err(_) => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn settings_sync_apply(
        &mut self,
        payload: sync::Payload,
        hash: String,
        cx: &mut Context<Self>,
    ) {
        let vim_was_on = self.preferences.vim_mode;
        self.preferences = sync::apply_preferences(&self.preferences, &payload.preferences);
        if self.preferences.vim_mode && !vim_was_on {
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
        if let Err(error) = zedb_core::save_preferences(&self.preferences) {
            self.settings_sync.status = Some((format!("could not save: {error}"), true));
            return;
        }
        self.connection.connections = payload.connections.clone();
        if let Err(error) = zedb_core::save_connections(&self.connection.connections) {
            self.settings_sync.status = Some((format!("could not save: {error}"), true));
            return;
        }
        if self
            .connection
            .selected
            .is_some_and(|index| index >= self.connection.connections.len())
        {
            self.connection.selected = None;
        }
        sync::save_state(&sync::SyncState {
            last_synced_hash: Some(hash),
            last_synced_at: Some(unix_now()),
        });
        self.settings_sync.status = Some(("Applied settings from the sync repo".into(), false));
        self.notice = Some("Settings updated from the sync repo".into());
        self.notice_warning = false;
        cx.notify();
    }

    /// Stamp the current local settings as "already synced". Run at
    /// enable time so a fresh machine's first tick reads as "only the
    /// repo changed" and inherits the repo's settings instead of
    /// pushing its own defaults over them.
    fn settings_sync_seed_stamp(&self) {
        sync::save_state(&sync::SyncState {
            last_synced_hash: Some(sync::content_hash(
                &self.preferences,
                &self.connection.connections,
            )),
            last_synced_at: Some(unix_now()),
        });
    }

    fn settings_sync_enable(&mut self, cx: &mut Context<Self>) {
        let url = self
            .settings_sync
            .url_input
            .read(cx)
            .text()
            .trim()
            .to_string();
        if url.is_empty() {
            self.flash_warning("Paste a git URL (or a local repo path) first", cx);
            return;
        }
        self.settings_sync_enable_url(url, cx);
    }

    fn settings_sync_enable_url(&mut self, url: String, cx: &mut Context<Self>) {
        self.settings_sync.switch_offer = None;
        // A local path that is already a checkout needs no clone.
        let local = PathBuf::from(&url);
        if local.join(".git").exists() {
            self.preferences.settings_sync_url = Some(url);
            self.preferences.settings_sync_repo = Some(local.display().to_string());
            let _ = zedb_core::save_preferences(&self.preferences);
            self.settings_sync_seed_stamp();
            self.settings_sync_tick(cx);
            return;
        }
        // A bare local repo (a directory with HEAD but no .git) clones
        // like any remote; everything else must look like a git URL.
        let local_bare = local.join("HEAD").exists();
        if !local_bare && !git::is_remote_url(&url) {
            self.flash_warning(
                "That doesn't look like a git URL or an existing local repo",
                cx,
            );
            return;
        }

        let Some(dest) = dirs::data_local_dir().map(|dir| {
            dir.join("zedb")
                .join("settings-sync")
                .join(git::clone_directory_name(&url))
        }) else {
            self.flash_warning("Could not determine a local data directory", cx);
            return;
        };
        if dest.join(".git").exists() {
            // Cloned on an earlier attempt; reuse it.
            self.preferences.settings_sync_url = Some(url);
            self.preferences.settings_sync_repo = Some(dest.display().to_string());
            let _ = zedb_core::save_preferences(&self.preferences);
            self.settings_sync_seed_stamp();
            self.settings_sync_tick(cx);
            return;
        }

        self.settings_sync.busy = true;
        self.settings_sync.status = Some(("Cloning...".into(), false));
        cx.notify();
        let clone_url = url.clone();
        let clone_dest = dest.clone();
        let handle = rt::tokio().spawn(async move { git::clone_repo(&clone_url, &clone_dest) });
        cx.spawn(async move |this, cx| {
            let result = handle.await;
            this.update(cx, |this, cx| {
                this.settings_sync.busy = false;
                match result {
                    Ok(Ok(())) => {
                        this.preferences.settings_sync_url = Some(url);
                        this.preferences.settings_sync_repo = Some(dest.display().to_string());
                        let _ = zedb_core::save_preferences(&this.preferences);
                        this.settings_sync_seed_stamp();
                        this.settings_sync.status = None;
                        this.settings_sync_tick(cx);
                    }
                    Ok(Err(error)) => {
                        this.settings_sync.status = Some((error, true));
                    }
                    Err(_) => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Quiet, once-per-launch: if the user is signed in and sync is
    /// unconfigured, ask their own git (ssh) whether a zedb-settings
    /// repo exists under their account. Found one: prefill its real URL.
    /// No OAuth scope involved; a private repo is visible to their key.
    pub(crate) fn settings_sync_probe_existing(&mut self, cx: &mut Context<Self>) {
        if self.settings_sync.probed || self.preferences.settings_sync_url.is_some() {
            return;
        }
        let crate::GithubAuth::SignedIn(profile) = &self.github else {
            return;
        };
        if !self
            .settings_sync
            .url_input
            .read(cx)
            .text()
            .trim()
            .is_empty()
        {
            return;
        }
        self.settings_sync.probed = true;
        let url = profile.provider.ssh_url(&profile.login, "zedb-settings");
        let probe_url = url.clone();
        let handle = rt::tokio().spawn(async move { git::remote_exists(&probe_url) });
        cx.spawn(async move |this, cx| {
            if !matches!(handle.await, Ok(true)) {
                return;
            }
            this.update(cx, |this, cx| {
                if this.preferences.settings_sync_url.is_some() {
                    return;
                }
                let input = this.settings_sync.url_input.clone();
                let still_empty = input.read(cx).text().trim().is_empty();
                if still_empty {
                    input.update(cx, |input, cx| input.set_text(url.clone(), cx));
                    this.settings_sync.status =
                        Some((format!("Found {url}; press Enable to link it"), false));
                    this.settings_sync.probed_url = Some(url.clone());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// The signed-in identity changed (sign-in, sign-out, provider
    /// switch): forget the old probe, clear a URL only if the probe put
    /// it there, and probe again for the new identity.
    pub(crate) fn settings_sync_identity_changed(&mut self, cx: &mut Context<Self>) {
        self.settings_sync.switch_offer = None;
        // Signed in with one forge while sync is linked to the other:
        // unlink into the text box, old URL prefilled, so one Enable
        // relinks it (or clearing the field adopts the new provider).
        // A sign-out alone never unlinks; sync is BYO and works
        // without any identity.
        if let crate::GithubAuth::SignedIn(profile) = &self.github {
            if let Some(url) = self.preferences.settings_sync_url.clone() {
                let other_host = match profile.provider {
                    github::Provider::GitHub => "gitlab.com",
                    github::Provider::GitLab => "github.com",
                };
                if url.contains(other_host) {
                    self.preferences.settings_sync_url = None;
                    self.preferences.settings_sync_repo = None;
                    let _ = zedb_core::save_preferences(&self.preferences);
                    self.settings_sync.generation += 1;
                    self.settings_sync
                        .url_input
                        .clone()
                        .update(cx, |input, cx| input.set_text(url, cx));
                    self.settings_sync.status = Some((
                        format!(
                            "Unlinked the {other_host} repo after the account switch; \
                             press Enable to relink it, or clear the field to use {}",
                            profile.provider.name()
                        ),
                        false,
                    ));
                    // Look for a counterpart under the new provider and
                    // offer a one-click switch if it exists.
                    let candidate = profile.provider.ssh_url(&profile.login, "zedb-settings");
                    let probe = candidate.clone();
                    let handle = rt::tokio().spawn(async move { git::remote_exists(&probe) });
                    cx.spawn(async move |this, cx| {
                        if !matches!(handle.await, Ok(true)) {
                            return;
                        }
                        this.update(cx, |this, cx| {
                            if this.preferences.settings_sync_url.is_none() {
                                this.settings_sync.switch_offer = Some(candidate);
                                cx.notify();
                            }
                        })
                        .ok();
                    })
                    .detach();
                }
            }
        }
        if let Some(prefilled) = self.settings_sync.probed_url.take() {
            let input = self.settings_sync.url_input.clone();
            if input.read(cx).text().trim() == prefilled {
                input.update(cx, |input, cx| input.set_text("", cx));
                self.settings_sync.status = None;
            }
        }
        // The example URL follows the signed-in provider.
        let placeholder = match &self.github {
            crate::GithubAuth::SignedIn(profile) => {
                profile.provider.ssh_url("you", "zedb-settings")
            }
            _ => github::Provider::GitHub.ssh_url("you", "zedb-settings"),
        };
        self.settings_sync
            .url_input
            .clone()
            .update(cx, |input, cx| input.set_placeholder(placeholder, cx));
        self.settings_sync.probed = false;
        self.settings_sync_probe_existing(cx);
    }

    /// One-click GitHub bootstrap: a second device-flow approval grants
    /// a `repo`-scoped token that lives only inside this future. It
    /// checks for an existing zedb-settings repo (offering to link it)
    /// or creates a private one, then the token is dropped: the stored
    /// sign-in keeps its read-only scope.
    pub(crate) fn settings_sync_bootstrap(&mut self, cx: &mut Context<Self>) {
        let crate::GithubAuth::SignedIn(profile) = &self.github else {
            self.flash_warning("Sign in first", cx);
            return;
        };
        let provider = profile.provider;
        let login = profile.login.clone();
        self.settings_sync.bootstrap_generation += 1;
        let generation = self.settings_sync.bootstrap_generation;
        let handle = rt::tokio().spawn(async move {
            github::start_device_flow_scoped(provider, provider.elevated_scope()).await
        });
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
                    if this.settings_sync.bootstrap_generation != generation {
                        return true;
                    }
                    this.settings_sync.bootstrap = Some(Bootstrap::Authorizing {
                        user_code: device.user_code.clone(),
                    });
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
            let token = match rt::tokio()
                .spawn(async move { github::poll_for_token(provider, &poll_device).await })
                .await
            {
                Ok(Ok(token)) => token,
                Ok(Err(error)) => {
                    this.update(cx, |this, cx| {
                        if this.settings_sync.bootstrap_generation == generation {
                            this.settings_sync.bootstrap = None;
                            this.flash_warning(error, cx);
                        }
                    })
                    .ok();
                    return;
                }
                Err(_) => return,
            };
            if this
                .update(cx, |this, cx| {
                    if this.settings_sync.bootstrap_generation != generation {
                        return true;
                    }
                    this.settings_sync.bootstrap =
                        Some(Bootstrap::Working("Looking for zedb-settings...".into()));
                    cx.notify();
                    false
                })
                .unwrap_or(true)
            {
                return;
            }
            let lookup_token = token.clone();
            let lookup_login = login.clone();
            let existing = rt::tokio()
                .spawn(async move {
                    github::get_repo(provider, &lookup_token, &lookup_login, "zedb-settings").await
                })
                .await;
            match existing {
                Ok(Ok(Some(repo))) => {
                    this.update(cx, |this, cx| {
                        if this.settings_sync.bootstrap_generation == generation {
                            this.settings_sync.bootstrap = Some(Bootstrap::OfferLink {
                                ssh_url: repo.ssh_url,
                            });
                            cx.notify();
                        }
                    })
                    .ok();
                    // The elevated token drops here; linking uses the
                    // user's own git auth.
                }
                Ok(Ok(None)) => {
                    if this
                        .update(cx, |this, cx| {
                            if this.settings_sync.bootstrap_generation != generation {
                                return true;
                            }
                            this.settings_sync.bootstrap = Some(Bootstrap::Working(
                                "Creating a private zedb-settings repo...".into(),
                            ));
                            cx.notify();
                            false
                        })
                        .unwrap_or(true)
                    {
                        return;
                    }
                    let created = rt::tokio()
                        .spawn(async move {
                            github::create_private_repo(
                                provider,
                                &token,
                                "zedb-settings",
                                "zeDB settings sync (created by zeDB)",
                            )
                            .await
                        })
                        .await;
                    this.update(cx, |this, cx| {
                        if this.settings_sync.bootstrap_generation != generation {
                            return;
                        }
                        this.settings_sync.bootstrap = None;
                        match created {
                            Ok(Ok(repo)) => {
                                this.notice = Some(format!(
                                    "Created {}; the elevated access was not kept",
                                    repo.full_name
                                ));
                                this.notice_warning = false;
                                this.settings_sync_enable_url(repo.ssh_url, cx);
                            }
                            Ok(Err(error)) => this.flash_warning(error, cx),
                            Err(_) => {}
                        }
                        cx.notify();
                    })
                    .ok();
                }
                Ok(Err(error)) => {
                    this.update(cx, |this, cx| {
                        if this.settings_sync.bootstrap_generation == generation {
                            this.settings_sync.bootstrap = None;
                            this.flash_warning(error, cx);
                        }
                    })
                    .ok();
                }
                Err(_) => {}
            }
        })
        .detach();
    }

    fn settings_sync_bootstrap_cancel(&mut self, cx: &mut Context<Self>) {
        self.settings_sync.bootstrap_generation += 1;
        self.settings_sync.bootstrap = None;
        cx.notify();
    }

    fn settings_sync_disable(&mut self, cx: &mut Context<Self>) {
        // The checkout stays on disk; disabling only stops the syncing.
        self.preferences.settings_sync_url = None;
        self.preferences.settings_sync_repo = None;
        let _ = zedb_core::save_preferences(&self.preferences);
        self.settings_sync.generation += 1;
        self.settings_sync.status = None;
        // Offer the way back: probe again so the repo that was just
        // unlinked (or any other existing one) prefills for re-enable.
        self.settings_sync.probed = false;
        self.settings_sync.probed_url = None;
        self.settings_sync_probe_existing(cx);
        self.notice = Some("Settings sync disabled; the repo and its history are untouched".into());
        self.notice_warning = false;
        cx.notify();
    }
}
