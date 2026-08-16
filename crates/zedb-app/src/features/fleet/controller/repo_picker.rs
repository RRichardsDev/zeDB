//! The migration-repo picker: local folder via the native directory
//! dialog, or a git host via the user's sign-in (elevated device-flow
//! token, held only in the picker and dropped with it). Empty
//! directories are initialized only after an explicit confirmation;
//! the one exception is a repository the user just created to be one.

use gpui::Context;

use crate::fleet::view::RepoPicker;
use crate::{github, rt, Workspace};

impl Workspace {
    /// The folder button: with a repo path typed, open it as always;
    /// with none, offer the picker.
    pub(crate) fn fleet_repo_button(&mut self, cx: &mut Context<Self>) {
        if self.fleet.repo_path.read(cx).text().trim().is_empty() {
            let path = Self::input("", "/path/to/migration-repo", false, cx);
            self.fleet.repo_picker = Some(RepoPicker::Menu { path });
            self.fleet.repo_picker_generation += 1;
            cx.notify();
        } else {
            self.fleet_open_repo(cx);
        }
    }

    pub(crate) fn repo_picker_close(&mut self, cx: &mut Context<Self>) {
        self.fleet.repo_picker = None;
        self.fleet.repo_picker_generation += 1;
        cx.notify();
    }

    /// The menu's folder button: open the typed path, or browse via
    /// the native directory dialog when nothing is typed.
    pub(crate) fn repo_picker_local(&mut self, cx: &mut Context<Self>) {
        if let Some(RepoPicker::Menu { path }) = &self.fleet.repo_picker {
            let typed = path.read(cx).text().trim().to_string();
            if !typed.is_empty() {
                self.fleet.repo_picker = None;
                self.fleet.repo_picker_generation += 1;
                self.fleet
                    .repo_path
                    .update(cx, |input, cx| input.set_text(typed, cx));
                self.fleet_open_repo(cx);
                cx.notify();
                return;
            }
        }
        self.fleet.repo_picker = None;
        cx.notify();
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |this, cx| {
                let text = path.display().to_string();
                this.fleet
                    .repo_path
                    .update(cx, |input, cx| input.set_text(text, cx));
                this.fleet_open_repo(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Git: elevated device-flow approval, then the user's repo list.
    /// Mirrors the settings-sync bootstrap; the token never leaves the
    /// picker state.
    pub(crate) fn repo_picker_git(&mut self, cx: &mut Context<Self>) {
        let crate::GithubAuth::SignedIn(profile) = &self.github else {
            self.flash_warning("Sign in to a git host first (Preferences)", cx);
            return;
        };
        let provider = profile.provider;
        self.fleet.repo_picker_generation += 1;
        let generation = self.fleet.repo_picker_generation;
        self.fleet.repo_picker = Some(RepoPicker::Loading("Requesting access...".into()));
        cx.notify();
        let handle = rt::tokio().spawn(async move {
            github::start_device_flow_scoped(provider, provider.elevated_scope()).await
        });
        cx.spawn(async move |this, cx| {
            let device = match handle.await {
                Ok(Ok(device)) => device,
                Ok(Err(error)) => {
                    this.update(cx, |this, cx| {
                        if this.fleet.repo_picker_generation == generation {
                            this.fleet.repo_picker = None;
                            this.flash_warning(error, cx);
                        }
                    })
                    .ok();
                    return;
                }
                Err(_) => return,
            };
            let poll_device = device.clone();
            let stale = this
                .update(cx, |this, cx| {
                    if this.fleet.repo_picker_generation != generation {
                        return true;
                    }
                    this.fleet.repo_picker = Some(RepoPicker::Authorizing {
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
                Ok(Ok(token)) => {
                    // Keep the elevated token: zeDB's own git runs on
                    // this host authenticate with it from now on, so
                    // account choice does not hang on SSH keys.
                    let _ = zedb_core::secrets::set_plain(&provider.broker_keychain_key(), &token);
                    token
                }
                Ok(Err(error)) => {
                    this.update(cx, |this, cx| {
                        if this.fleet.repo_picker_generation == generation {
                            this.fleet.repo_picker = None;
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
                    if this.fleet.repo_picker_generation != generation {
                        return true;
                    }
                    this.fleet.repo_picker =
                        Some(RepoPicker::Loading("Listing repositories...".into()));
                    cx.notify();
                    false
                })
                .unwrap_or(true)
            {
                return;
            }
            let list_token = token.clone();
            let listed = rt::tokio()
                .spawn(async move { github::list_repos(provider, &list_token).await })
                .await;
            this.update(cx, |this, cx| {
                if this.fleet.repo_picker_generation != generation {
                    return;
                }
                match listed {
                    Ok(Ok(repos)) => {
                        let create_name = Self::input("", "new repository name", false, cx);
                        this.fleet.repo_picker = Some(RepoPicker::Repos {
                            provider,
                            token,
                            repos,
                            create_name,
                        });
                    }
                    Ok(Err(error)) => {
                        this.fleet.repo_picker = None;
                        this.flash_warning(error, cx);
                    }
                    Err(_) => this.fleet.repo_picker = None,
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// A repo picked from the list: open (clone) it.
    pub(crate) fn repo_picker_choose(&mut self, ssh_url: String, cx: &mut Context<Self>) {
        self.fleet.repo_picker = None;
        self.fleet.repo_picker_generation += 1;
        self.fleet
            .repo_path
            .update(cx, |input, cx| input.set_text(ssh_url, cx));
        self.fleet_open_repo(cx);
        cx.notify();
    }

    /// Create a new private repository and open it. The fresh clone is
    /// empty by construction, so it initializes as a migration repo
    /// without the confirm dialog: creating it was the confirmation.
    pub(crate) fn repo_picker_create(&mut self, cx: &mut Context<Self>) {
        let Some(RepoPicker::Repos {
            provider,
            token,
            create_name,
            ..
        }) = &self.fleet.repo_picker
        else {
            return;
        };
        let provider = *provider;
        let token = token.clone();
        let name = create_name.read(cx).text().trim().to_string();
        if name.is_empty() {
            self.flash_warning("Name the new repository first", cx);
            return;
        }
        self.fleet.repo_picker_generation += 1;
        let generation = self.fleet.repo_picker_generation;
        self.fleet.repo_picker = Some(RepoPicker::Loading(format!("Creating {name}...")));
        cx.notify();
        let handle = rt::tokio().spawn(async move {
            github::create_private_repo(provider, &token, &name, "zeDB migration repo").await
        });
        cx.spawn(async move |this, cx| {
            let result = handle.await;
            this.update(cx, |this, cx| {
                if this.fleet.repo_picker_generation != generation {
                    return;
                }
                match result.map_err(|error| error.to_string()) {
                    Ok(Ok(repo)) => {
                        this.fleet.repo_picker = None;
                        this.fleet.auto_init_once = true;
                        this.fleet.repo_path.update(cx, |input, cx| {
                            input.set_text(
                                if repo.http_url.is_empty() {
                                    repo.ssh_url
                                } else {
                                    repo.http_url
                                },
                                cx,
                            )
                        });
                        this.fleet_open_repo(cx);
                    }
                    Ok(Err(error)) | Err(error) => {
                        this.fleet.repo_picker = None;
                        this.flash_warning(error, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The empty-directory confirmation: initialize and reopen.
    pub(crate) fn repo_picker_confirm_init(&mut self, cx: &mut Context<Self>) {
        let Some(RepoPicker::ConfirmInit { path, source }) = self.fleet.repo_picker.take() else {
            return;
        };
        self.fleet.repo_picker_generation += 1;
        self.fleet.auto_init_once = true;
        let text = source.clone();
        self.fleet
            .repo_path
            .update(cx, |input, cx| input.set_text(text, cx));
        let _ = path;
        self.fleet_open_repo(cx);
        cx.notify();
    }
}
