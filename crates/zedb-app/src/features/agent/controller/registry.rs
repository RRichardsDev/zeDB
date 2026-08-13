use super::*;

impl Workspace {
    pub(crate) fn agent_open_add_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.agent.open = true;
        self.agent.add_form = Some(AddAgentForm {
            name: cx.new(|cx| InputState::new(window, cx).placeholder("Name")),
            command: cx.new(|cx| {
                InputState::new(window, cx).placeholder("command and args, e.g. my-agent --acp")
            }),
            error: None,
        });
        cx.notify();
    }

    pub(crate) fn agent_save_custom(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &self.agent.add_form else {
            return;
        };
        let name = form.name.read(cx).value().trim().to_string();
        let command_line = form.command.read(cx).value().trim().to_string();
        let mut parts = command_line.split_whitespace().map(str::to_string);
        let command = parts.next().unwrap_or_default();
        if name.is_empty() || command.is_empty() {
            if let Some(form) = &mut self.agent.add_form {
                form.error = Some("both a name and a command are needed".into());
            }
            cx.notify();
            return;
        }
        self.preferences.custom_agents.push(zedb_core::CustomAgent {
            name,
            command,
            args: parts.collect(),
        });
        if let Err(error) = zedb_core::save_preferences(&self.preferences) {
            self.notice = Some(format!("Could not save preferences: {error}"));
            self.notice_warning = true;
            self.notice_flash_id += 1;
        }
        self.agent.add_form = None;
        self.agent_refresh_registry();
        cx.notify();
    }

    pub(crate) fn agent_remove_custom(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.preferences.custom_agents.len() {
            self.preferences.custom_agents.remove(index);
            if let Err(error) = zedb_core::save_preferences(&self.preferences) {
                self.notice = Some(format!("Could not save preferences: {error}"));
                self.notice_warning = true;
                self.notice_flash_id += 1;
            }
            self.agent_refresh_registry();
        }
        cx.notify();
    }

    /// Poll the open repo's file signature while `generation` is the
    /// live thread; on change, reopen the repo and refresh git state.
    pub(crate) fn agent_watch_repo(&mut self, generation: u64, cx: &mut Context<Self>) {
        fn signature(root: &std::path::Path) -> u64 {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            let stamp = |path: &std::path::Path, hasher: &mut _| {
                if let Ok(meta) = std::fs::metadata(path) {
                    path.hash(hasher);
                    meta.len().hash(hasher);
                    if let Ok(modified) = meta.modified() {
                        modified.hash(hasher);
                    }
                }
            };
            stamp(&root.join("zedb.toml"), &mut hasher);
            stamp(&root.join("exclusions.toml"), &mut hasher);
            let mut stack = vec![root.join("migrations")];
            while let Some(dir) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        stack.push(path);
                    } else {
                        stamp(&path, &mut hasher);
                    }
                }
            }
            hasher.finish()
        }

        let Some(root) = self.fleet.repo.as_ref().map(|repo| repo.root.clone()) else {
            return;
        };
        let mut last = signature(&root);
        cx.spawn(async move |this, cx| loop {
            gpui::Timer::after(std::time::Duration::from_secs(2)).await;
            let stale = this
                .update(cx, |this, cx| {
                    let live = this
                        .agent
                        .thread
                        .as_ref()
                        .is_some_and(|thread| thread.generation == generation);
                    if !live {
                        return true;
                    }
                    let current = signature(&root);
                    if current != last {
                        last = current;
                        if let Ok(reopened) = zedb_core::repo::MigrationRepo::open_root(&root) {
                            this.fleet.repo = Some(Arc::new(reopened));
                        }
                        this.fleet.git = zedb_core::git::read_git_status(&root);
                        if let Some(thread) = this.agent.thread.as_mut() {
                            thread
                                .entries
                                .push(ThreadEntry::Info("repo files changed on disk".into()));
                        }
                        cx.notify();
                    }
                    false
                })
                .unwrap_or(true);
            if stale {
                break;
            }
        })
        .detach();
    }
}
