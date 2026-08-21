use crate::*;

use gpui::prelude::*;

impl Workspace {
    pub(crate) fn password_for_draft(
        &self,
        draft: &ConnectionDraft,
    ) -> Result<Option<String>, String> {
        if !draft.password.is_empty() {
            return Ok(Some(draft.password.clone()));
        }
        let key_name = draft.original_name.as_deref().unwrap_or(&draft.config.name);
        zedb_core::secrets::get_password(key_name)
            .map_err(|error| format!("Could not read macOS Keychain: {error}"))
    }

    pub(crate) fn persist_draft(
        &mut self,
        draft: &ConnectionDraft,
        unlocked_previous_password: Option<&Option<String>>,
    ) -> Result<usize, String> {
        let name = &draft.config.name;
        let previous_connections = self.connection.connections.clone();
        let previous_password = match draft.original_name.as_deref() {
            None => None,
            Some(_) if unlocked_previous_password.is_some() => {
                unlocked_previous_password.cloned().flatten()
            }
            Some(old_name) => zedb_core::secrets::get_password(old_name)
                .map_err(|error| format!("Could not read macOS Keychain: {error}"))?,
        };
        let mut updated = previous_connections.clone();
        let index = match draft.editing {
            Some(index) => {
                updated[index] = draft.config.clone();
                index
            }
            None => {
                updated.push(draft.config.clone());
                updated.len() - 1
            }
        };
        save_connections(&updated)
            .map_err(|error| format!("Could not save connections: {error}"))?;

        let secret_result = if draft.password.is_empty() {
            match draft.original_name.as_deref() {
                Some(old) if old != name => zedb_core::secrets::rename(old, name),
                _ => Ok(()),
            }
        } else {
            zedb_core::secrets::set_password(name, &draft.password).and_then(|_| {
                if let Some(old) = draft.original_name.as_deref().filter(|old| *old != name) {
                    zedb_core::secrets::delete_password(old)?;
                }
                Ok(())
            })
        };
        if let Err(error) = secret_result {
            let rollback_error = save_connections(&previous_connections).err();
            if let Some(old_name) = draft.original_name.as_deref() {
                if let Some(password) = previous_password.as_deref() {
                    let _ = zedb_core::secrets::set_password(old_name, password);
                }
            }
            if draft.original_name.as_deref() != Some(name.as_str()) {
                let _ = zedb_core::secrets::delete_password(name);
            }
            return Err(match rollback_error {
                Some(rollback_error) => format!(
                    "Could not update macOS Keychain: {error}. Could not roll back connection config: {rollback_error}"
                ),
                None => format!("Could not update macOS Keychain: {error}"),
            });
        }

        if let Some(old_name) = draft.original_name.as_deref() {
            self.connection.endpoint_health.remove(old_name);
            if self
                .connection
                .connected
                .as_ref()
                .map(|connected| connected.name.as_str())
                == Some(old_name)
            {
                self.connection.connected = None;
                self.fleet.write_unlocked = false;
                self.fleet.write_unlocked = false;
                self.clear_schema();
            }
        }
        self.connection.connections = updated;
        self.connection.selected = Some(index);
        Ok(index)
    }

    pub(crate) fn save_form(&mut self, cx: &mut Context<Self>) {
        if self
            .connection
            .form
            .as_ref()
            .is_some_and(|form| form.provision == ProvisionStage::Working)
        {
            self.flash_warning("Wait for password provisioning to finish", cx);
            return;
        }
        let result = self
            .draft_from_form(cx)
            .and_then(|draft| self.persist_draft(&draft, None).map(|_| draft.config.name));
        match result {
            Ok(name) => {
                self.connection.form = None;
                self.notice = Some(format!("Saved {name} without testing"));
                self.settings_sync_tick(cx);
            }
            Err(error) => self.notice = Some(error),
        }
        cx.notify();
    }

    pub(crate) fn save_and_connect(&mut self, cx: &mut Context<Self>) {
        if self
            .connection
            .form
            .as_ref()
            .is_some_and(|form| form.provision == ProvisionStage::Working)
        {
            self.flash_warning("Wait for password provisioning to finish", cx);
            return;
        }
        let draft = match self.draft_from_form(cx) {
            Ok(draft) => draft,
            Err(error) => {
                self.notice = Some(error);
                cx.notify();
                return;
            }
        };
        let password = match self.password_for_draft(&draft) {
            Ok(password) => password,
            Err(error) => {
                self.notice = Some(error);
                cx.notify();
                return;
            }
        };
        self.probe_connection(draft.config.clone(), password, Some(draft), cx);
    }

    pub(crate) fn connect_selected(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.connection.selected else {
            return;
        };
        let connection = self.connection.connections[index].clone();
        let password = match self
            .connection
            .password_cache
            .get(&connection.name)
            .cloned()
        {
            Some(password) => password,
            None => match zedb_core::secrets::get_password(&connection.name) {
                Ok(password) => password,
                Err(error) => {
                    self.notice = Some(format!("Could not read macOS Keychain: {error}"));
                    cx.notify();
                    return;
                }
            },
        };
        self.probe_connection(connection, password, None, cx);
    }
}
