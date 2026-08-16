//! ClickHouse Cloud linking: paste an organization API key once, see
//! the org's services with live state, and add any of them as a
//! connection prefilled from the control plane (host, port, TLS). The
//! key goes to the Keychain; only the org id and name persist. The
//! panel fills the same connection form the user always edits, so the
//! hands-on path stays the front door.

use std::collections::HashMap;

use gpui::prelude::*;

use crate::clickhouse_cloud::{self, CloudOrg, CloudService};
use crate::*;

pub(crate) struct CloudLinkState {
    pub(crate) open: bool,
    pub(crate) key_id: Option<Entity<TextInput>>,
    pub(crate) key_secret: Option<Entity<TextInput>>,
    pub(crate) linking: bool,
    pub(crate) loading: bool,
    pub(crate) error: Option<String>,
    /// (org id, service) pairs from the last refresh, in org order.
    pub(crate) services: Vec<(String, CloudService)>,
    /// service id -> last seen state, for the sidebar's idle marker.
    pub(crate) states: HashMap<String, String>,
    pub(crate) generation: u64,
    /// A refresh token sits in the Keychain (browser sign-in done).
    pub(crate) signed_in: bool,
    /// The signed-in account email, once a token has been decoded.
    pub(crate) account: Option<String>,
    /// The user code awaiting browser approval, while polling.
    pub(crate) authorizing: Option<String>,
    /// Organizations visible through the sign-in (superset of, or
    /// disjoint from, the keyed orgs in preferences).
    pub(crate) oauth_orgs: Vec<CloudOrg>,
    pub(crate) oauth_generation: u64,
}

impl CloudLinkState {
    pub(crate) fn new() -> Self {
        Self {
            open: false,
            key_id: None,
            key_secret: None,
            linking: false,
            loading: false,
            error: None,
            services: Vec::new(),
            states: HashMap::new(),
            generation: 0,
            signed_in: false,
            account: None,
            authorizing: None,
            oauth_orgs: Vec::new(),
            oauth_generation: 0,
        }
    }

    /// Whether this org's Start button can work: waking is a
    /// management write, which only an API key can do.
    pub(crate) fn org_has_key(&self, preferences: &zedb_core::Preferences, org_id: &str) -> bool {
        preferences.cloud_orgs.iter().any(|org| org.id == org_id)
    }
}

impl Workspace {
    pub(crate) fn cloud_open(&mut self, cx: &mut Context<Self>) {
        self.connection.cloud.open = true;
        self.connection.cloud.error = None;
        self.connection.cloud.signed_in = cloud_oauth::signed_in();
        if self.connection.cloud.key_id.is_none() {
            self.connection.cloud.key_id = Some(Self::input("", "API key id", false, cx));
            self.connection.cloud.key_secret = Some(Self::input("", "API key secret", true, cx));
        }
        self.cloud_refresh(cx);
        cx.notify();
    }

    /// Browser sign-in via the OAuth device flow: show the code, open
    /// the browser, poll until approved, keep only the refresh token
    /// (Keychain) and an in-memory access token.
    pub(crate) fn cloud_sign_in(&mut self, cx: &mut Context<Self>) {
        self.connection.cloud.oauth_generation += 1;
        let generation = self.connection.cloud.oauth_generation;
        self.connection.cloud.error = None;
        cx.notify();
        let handle = rt::tokio().spawn(cloud_oauth::start_device_flow());
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
            let stale = this
                .update(cx, |this, cx| {
                    if this.connection.cloud.oauth_generation != generation {
                        return true;
                    }
                    this.connection.cloud.authorizing = Some(device.user_code.clone());
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                        device.user_code.clone(),
                    ));
                    cx.open_url(device.open_url());
                    cx.notify();
                    false
                })
                .unwrap_or(true);
            if stale {
                return;
            }
            let poll_device = device.clone();
            let tokens = match rt::tokio()
                .spawn(async move { cloud_oauth::poll_for_tokens(&poll_device).await })
                .await
            {
                Ok(Ok(tokens)) => tokens,
                Ok(Err(error)) => {
                    this.update(cx, |this, cx| {
                        if this.connection.cloud.oauth_generation == generation {
                            this.connection.cloud.authorizing = None;
                            this.flash_warning(error, cx);
                            cx.notify();
                        }
                    })
                    .ok();
                    return;
                }
                Err(_) => return,
            };
            let stored = match tokens.refresh.as_deref() {
                Some(refresh) => cloud_oauth::store_refresh_token(refresh),
                None => Err("no offline access granted; sign-in will not survive a restart".into()),
            };
            cloud_oauth::cache_access_token(&tokens.access);
            let account = cloud_oauth::email(&tokens.access);
            this.update(cx, |this, cx| {
                if this.connection.cloud.oauth_generation != generation {
                    return;
                }
                this.connection.cloud.authorizing = None;
                this.connection.cloud.signed_in = true;
                this.connection.cloud.account = account;
                match stored {
                    Ok(()) => this.flash_notice("Signed in to ClickHouse Cloud", cx),
                    Err(error) => this.flash_warning(error, cx),
                }
                this.cloud_refresh(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Stop waiting for the browser approval. The abandoned poll keeps
    /// running against a code the user never approves; its result is
    /// discarded by the generation check.
    pub(crate) fn cloud_sign_in_cancel(&mut self, cx: &mut Context<Self>) {
        self.connection.cloud.oauth_generation += 1;
        self.connection.cloud.authorizing = None;
        cx.notify();
    }

    /// Clear the Keychain refresh token and every OAuth-derived row;
    /// keyed organizations stay linked.
    pub(crate) fn cloud_sign_out(&mut self, cx: &mut Context<Self>) {
        cloud_oauth::sign_out();
        self.connection.cloud.signed_in = false;
        self.connection.cloud.account = None;
        self.connection.cloud.oauth_orgs.clear();
        let keyed: Vec<String> = self
            .preferences
            .cloud_orgs
            .iter()
            .map(|org| org.id.clone())
            .collect();
        self.connection
            .cloud
            .services
            .retain(|(owner, _)| keyed.contains(owner));
        cx.notify();
    }

    pub(crate) fn cloud_close(&mut self, cx: &mut Context<Self>) {
        self.connection.cloud.open = false;
        cx.notify();
    }

    /// Validate a pasted key against the API, then store: secret in the
    /// Keychain, org id and name in preferences (those sync; the secret
    /// never leaves this machine).
    pub(crate) fn cloud_link(&mut self, cx: &mut Context<Self>) {
        let (Some(key_id), Some(key_secret)) = (
            self.connection.cloud.key_id.as_ref(),
            self.connection.cloud.key_secret.as_ref(),
        ) else {
            return;
        };
        let key_id = key_id.read(cx).text().trim().to_string();
        let key_secret = key_secret.read(cx).text().trim().to_string();
        if key_id.is_empty() || key_secret.is_empty() {
            self.connection.cloud.error = Some("Paste the API key id and secret".into());
            cx.notify();
            return;
        }
        self.connection.cloud.linking = true;
        self.connection.cloud.error = None;
        cx.notify();
        let task = rt::tokio().spawn(async move {
            let orgs = clickhouse_cloud::list_organizations(&key_id, &key_secret).await?;
            if orgs.is_empty() {
                return Err("The API key can see no organizations".to_string());
            }
            for org in &orgs {
                zedb_core::secrets::set_plain(
                    &clickhouse_cloud::keychain_key(&org.id),
                    &format!("{key_id}:{key_secret}"),
                )
                .map_err(|error| error.to_string())?;
            }
            Ok(orgs)
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await.unwrap_or_else(|_| Err("Linking stopped".into()));
            this.update(cx, |this, cx| {
                this.connection.cloud.linking = false;
                match outcome {
                    Ok(orgs) => {
                        for org in orgs {
                            let known = this
                                .preferences
                                .cloud_orgs
                                .iter_mut()
                                .find(|existing| existing.id == org.id);
                            match known {
                                Some(existing) => existing.name = org.name,
                                None => this.preferences.cloud_orgs.push(zedb_core::CloudOrgRef {
                                    id: org.id,
                                    name: org.name,
                                }),
                            }
                        }
                        let _ = zedb_core::save_preferences(&this.preferences);
                        if let Some(input) = this.connection.cloud.key_id.as_ref() {
                            input.update(cx, |input, cx| input.set_text("", cx));
                        }
                        if let Some(input) = this.connection.cloud.key_secret.as_ref() {
                            input.update(cx, |input, cx| input.set_text("", cx));
                        }
                        this.cloud_refresh(cx);
                    }
                    Err(error) => this.connection.cloud.error = Some(error),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Forget one linked organization: Keychain key and preference
    /// entry. Connections created from it stay; they are ordinary
    /// connections.
    pub(crate) fn cloud_unlink(&mut self, org_id: &str, cx: &mut Context<Self>) {
        let _ = zedb_core::secrets::delete_plain(&clickhouse_cloud::keychain_key(org_id));
        self.preferences.cloud_orgs.retain(|org| org.id != org_id);
        let _ = zedb_core::save_preferences(&self.preferences);
        self.connection
            .cloud
            .services
            .retain(|(owner, _)| owner != org_id);
        cx.notify();
    }

    /// Re-list every visible org's services: keyed orgs through their
    /// API keys, then any further orgs the browser sign-in can see
    /// (read-only Bearer). State, endpoints, and anything added or
    /// removed in the console since last look.
    pub(crate) fn cloud_refresh(&mut self, cx: &mut Context<Self>) {
        let orgs = self.preferences.cloud_orgs.clone();
        let signed_in = self.connection.cloud.signed_in;
        if orgs.is_empty() && !signed_in {
            return;
        }
        self.connection.cloud.loading = true;
        self.connection.cloud.generation += 1;
        let generation = self.connection.cloud.generation;
        cx.notify();
        let task = rt::tokio().spawn(async move {
            let mut services = Vec::new();
            let mut errors = Vec::new();
            let mut oauth_orgs = Vec::new();
            let mut account = None;
            for org in &orgs {
                let stored =
                    zedb_core::secrets::get_plain(&clickhouse_cloud::keychain_key(&org.id))
                        .ok()
                        .flatten();
                let Some((key_id, key_secret)) = stored
                    .as_deref()
                    .and_then(clickhouse_cloud::split_credentials)
                else {
                    errors.push(format!("{}: no API key in the Keychain", org.name));
                    continue;
                };
                match clickhouse_cloud::list_services(&key_id, &key_secret, &org.id).await {
                    Ok(list) => {
                        services.extend(list.into_iter().map(|service| (org.id.clone(), service)))
                    }
                    Err(error) => errors.push(format!("{}: {error}", org.name)),
                }
            }
            if signed_in {
                match cloud_oauth::access_token().await {
                    Ok(Some(token)) => {
                        account = cloud_oauth::email(&token);
                        match clickhouse_cloud::list_organizations_bearer(&token).await {
                            Ok(list) => {
                                for org in list {
                                    // Keyed orgs are already listed above with
                                    // the stronger credential.
                                    if !orgs.iter().any(|keyed| keyed.id == org.id) {
                                        match clickhouse_cloud::list_services_bearer(
                                            &token, &org.id,
                                        )
                                        .await
                                        {
                                            Ok(list) => services.extend(
                                                list.into_iter()
                                                    .map(|service| (org.id.clone(), service)),
                                            ),
                                            Err(error) => {
                                                errors.push(format!("{}: {error}", org.name))
                                            }
                                        }
                                    }
                                    oauth_orgs.push(org);
                                }
                            }
                            Err(error) => errors.push(error),
                        }
                    }
                    Ok(None) => {}
                    Err(error) => errors.push(error),
                }
            }
            (services, errors, oauth_orgs, account)
        });
        cx.spawn(async move |this, cx| {
            let Ok((services, errors, oauth_orgs, account)) = task.await else {
                return;
            };
            this.update(cx, |this, cx| {
                if this.connection.cloud.generation != generation {
                    return;
                }
                this.connection.cloud.loading = false;
                this.connection.cloud.states = services
                    .iter()
                    .map(|(_, service)| (service.id.clone(), service.state.clone()))
                    .collect();
                this.connection.cloud.services = services;
                this.connection.cloud.oauth_orgs = oauth_orgs;
                if account.is_some() {
                    this.connection.cloud.account = account;
                }
                this.connection.cloud.error = (!errors.is_empty()).then(|| errors.join(" \u{b7} "));
                // A waking service settles on its own schedule: keep
                // polling until nothing is mid-transition, so the
                // sidebar's "waking" clears itself.
                if this
                    .connection
                    .cloud
                    .services
                    .iter()
                    .any(|(_, service)| service.is_waking())
                {
                    this.cloud_schedule_refresh(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// One delayed refresh, cancelled by any newer refresh (the
    /// generation moves on every fetch).
    fn cloud_schedule_refresh(&mut self, cx: &mut Context<Self>) {
        let generation = self.connection.cloud.generation;
        cx.spawn(async move |this, cx| {
            gpui::Timer::after(std::time::Duration::from_secs(15)).await;
            this.update(cx, |this, cx| {
                if this.connection.cloud.generation == generation {
                    this.cloud_refresh(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Ask the control plane to start an idled service, then poll the
    /// list until the state settles.
    pub(crate) fn cloud_start_service(
        &mut self,
        org_id: String,
        service_id: String,
        cx: &mut Context<Self>,
    ) {
        // Show it as waking immediately; the refresh confirms.
        if let Some((_, service)) = self
            .connection
            .cloud
            .services
            .iter_mut()
            .find(|(_, service)| service.id == service_id)
        {
            service.state = "starting".into();
        }
        self.connection
            .cloud
            .states
            .insert(service_id.clone(), "starting".into());
        cx.notify();
        let task = rt::tokio().spawn(async move {
            let stored = zedb_core::secrets::get_plain(&clickhouse_cloud::keychain_key(&org_id))
                .ok()
                .flatten();
            let Some((key_id, key_secret)) = stored
                .as_deref()
                .and_then(clickhouse_cloud::split_credentials)
            else {
                return Err("No API key in the Keychain for this organization".to_string());
            };
            clickhouse_cloud::start_service(&key_id, &key_secret, &org_id, &service_id).await
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await.unwrap_or_else(|_| Err("Start stopped".into()));
            this.update(cx, |this, cx| match outcome {
                Ok(()) => {
                    this.flash_notice("Waking the service; it can take a minute", cx);
                    this.cloud_refresh(cx);
                }
                Err(error) => this.flash_warning(format!("Could not start: {error}"), cx),
            })
            .ok();
        })
        .detach();
    }

    /// Open the ordinary add-connection form prefilled from a Cloud
    /// service: only the database password is left to type.
    pub(crate) fn cloud_add_service(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some((org_id, service)) = self.connection.cloud.services.get(index) else {
            return;
        };
        let Some(url) = service.https_url() else {
            self.flash_warning("The service reports no HTTPS endpoint", cx);
            return;
        };
        let cloud = zedb_core::CloudProvenance {
            org_id: org_id.clone(),
            service_id: service.id.clone(),
        };
        let name = service.name.clone();
        self.connection.cloud.open = false;
        self.connection.pending_delete = None;
        self.connection.form = Some(ConnectionForm {
            editing: None,
            original_name: None,
            name: Self::input(name.clone(), "staging", false, cx),
            nodes: vec![NodeForm {
                name: Self::input(name, "Node 1", false, cx),
                endpoint: Self::input(url, "https://host:8443", false, cx),
                native_port: Self::input(
                    service
                        .native_secure_port()
                        .map(|port| port.to_string())
                        .unwrap_or_default(),
                    "tcp auto",
                    false,
                    cx,
                ),
            }],
            user: Self::input("default", "default", false, cx),
            database: Self::input("", "optional", false, cx),
            password: Self::input(
                "",
                "the service password, stored in macOS Keychain",
                true,
                cx,
            ),
            // Cloud services default to the loud tier: a paid, shared
            // service should never look like a local dev box.
            tier: EnvTier::Production,
            read_only: true,
            driver_settings: Self::seeded_driver_settings(&[], cx),
            cloud: Some(cloud),
            provision: ProvisionStage::Idle,
            key_id: Some(Self::input("", "API key id", false, cx)),
            key_secret: Some(Self::input("", "API key secret", true, cx)),
            linking_key: false,
        });
        self.notice = None;
        cx.notify();
    }

    /// Link an API key pasted inline in the connection form: validate
    /// it against the control plane, require it to see this
    /// connection's organization, then store it exactly like the
    /// Cloud panel's link (secret in the Keychain, org id and name in
    /// preferences). Unlocks provisioning here and waking in the
    /// panel.
    pub(crate) fn cloud_link_from_form(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.connection.form.as_mut() else {
            return;
        };
        let Some(cloud) = form.cloud.clone() else {
            return;
        };
        let (Some(key_id), Some(key_secret)) = (form.key_id.as_ref(), form.key_secret.as_ref())
        else {
            return;
        };
        let key_id = key_id.read(cx).text().trim().to_string();
        let key_secret = key_secret.read(cx).text().trim().to_string();
        if key_id.is_empty() || key_secret.is_empty() {
            self.flash_warning("Paste the API key id and secret", cx);
            return;
        }
        form.linking_key = true;
        cx.notify();
        let org_id = cloud.org_id.clone();
        let task = rt::tokio().spawn(async move {
            let orgs = clickhouse_cloud::list_organizations(&key_id, &key_secret).await?;
            let Some(org) = orgs.into_iter().find(|org| org.id == org_id) else {
                return Err("The API key cannot see this connection's organization".to_string());
            };
            zedb_core::secrets::set_plain(
                &clickhouse_cloud::keychain_key(&org.id),
                &format!("{key_id}:{key_secret}"),
            )
            .map_err(|error| error.to_string())?;
            Ok(org)
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await.unwrap_or_else(|_| Err("Linking stopped".into()));
            this.update(cx, |this, cx| {
                if let Some(form) = this.connection.form.as_mut() {
                    form.linking_key = false;
                }
                match outcome {
                    Ok(org) => {
                        let known = this
                            .preferences
                            .cloud_orgs
                            .iter_mut()
                            .find(|existing| existing.id == org.id);
                        match known {
                            Some(existing) => existing.name = org.name.clone(),
                            None => this.preferences.cloud_orgs.push(zedb_core::CloudOrgRef {
                                id: org.id.clone(),
                                name: org.name.clone(),
                            }),
                        }
                        let _ = zedb_core::save_preferences(&this.preferences);
                        if let Some(form) = this.connection.form.as_ref() {
                            if let Some(input) = form.key_id.as_ref() {
                                input.update(cx, |input, cx| input.set_text("", cx));
                            }
                            if let Some(input) = form.key_secret.as_ref() {
                                input.update(cx, |input, cx| input.set_text("", cx));
                            }
                        }
                        this.flash_notice(
                            format!(
                                "API key linked for {}; provisioning and waking unlocked",
                                org.name
                            ),
                            cx,
                        );
                    }
                    Err(error) => this.flash_warning(format!("Could not link: {error}"), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Rotate the linked service's database password through the
    /// control plane and drop the result straight into the form's
    /// (masked) password field: saving stores it in the Keychain and
    /// the plaintext is never shown. Needs the org's API key; the
    /// form only offers this behind an explicit rotation confirm.
    pub(crate) fn cloud_provision_password(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.connection.form.as_mut() else {
            return;
        };
        let Some(cloud) = form.cloud.clone() else {
            return;
        };
        form.provision = ProvisionStage::Working;
        cx.notify();
        let task = rt::tokio().spawn(async move {
            let stored =
                zedb_core::secrets::get_plain(&clickhouse_cloud::keychain_key(&cloud.org_id))
                    .ok()
                    .flatten();
            let Some((key_id, key_secret)) = stored
                .as_deref()
                .and_then(clickhouse_cloud::split_credentials)
            else {
                return Err("No API key in the Keychain for this organization".to_string());
            };
            clickhouse_cloud::provision_password(
                &key_id,
                &key_secret,
                &cloud.org_id,
                &cloud.service_id,
            )
            .await
        });
        cx.spawn(async move |this, cx| {
            let outcome = task
                .await
                .unwrap_or_else(|_| Err("Provisioning stopped".into()));
            this.update(cx, |this, cx| {
                let Some(form) = this.connection.form.as_mut() else {
                    return;
                };
                form.provision = ProvisionStage::Idle;
                let password_input = form.password.clone();
                match outcome {
                    Ok(password) => {
                        password_input.update(cx, |input, cx| input.set_text(password, cx));
                        this.flash_notice(
                            "New password provisioned; save the connection to store it in the \
                             Keychain",
                            cx,
                        );
                    }
                    Err(error) => this.flash_warning(format!("Could not provision: {error}"), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// A probe found no reachable node: when the connection came from
    /// a linked Cloud service, name the real cause (asleep or waking)
    /// instead of leaving a bare timeout.
    pub(crate) fn cloud_explain_unreachable(
        &mut self,
        connection: &ConnectionConfig,
        cx: &mut Context<Self>,
    ) {
        let Some(cloud) = connection.cloud.clone() else {
            return;
        };
        if !self
            .preferences
            .cloud_orgs
            .iter()
            .any(|org| org.id == cloud.org_id)
        {
            return;
        }
        let name = connection.name.clone();
        let task = rt::tokio().spawn(async move {
            let stored =
                zedb_core::secrets::get_plain(&clickhouse_cloud::keychain_key(&cloud.org_id))
                    .ok()
                    .flatten();
            let (key_id, key_secret) = stored
                .as_deref()
                .and_then(clickhouse_cloud::split_credentials)?;
            clickhouse_cloud::list_services(&key_id, &key_secret, &cloud.org_id)
                .await
                .ok()?
                .into_iter()
                .find(|service| service.id == cloud.service_id)
        });
        cx.spawn(async move |this, cx| {
            let Ok(Some(service)) = task.await else {
                return;
            };
            this.update(cx, |this, cx| {
                this.connection
                    .cloud
                    .states
                    .insert(service.id.clone(), service.state.clone());
                if service.is_asleep() {
                    this.flash_warning(
                        format!(
                            "{name} is {} in ClickHouse Cloud; start it from the Cloud panel",
                            service.state
                        ),
                        cx,
                    );
                } else if service.is_waking() {
                    this.flash_notice(
                        format!("{name} is waking in ClickHouse Cloud; try again shortly"),
                        cx,
                    );
                    // Clear the sidebar's "waking" once it lands.
                    this.cloud_schedule_refresh(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Whether the active connection is linked to a ClickHouse Cloud
    /// service. Linkage is the stored service id from setup, never a
    /// URL heuristic; this drives the editor area's yellow border.
    pub(crate) fn active_connection_is_cloud(&self) -> bool {
        let Some(connected) = self.connection.connected.as_ref() else {
            return false;
        };
        self.connection
            .connections
            .iter()
            .any(|connection| connection.name == connected.name && connection.cloud.is_some())
    }

    /// The sidebar's marker for a connection whose Cloud service is not
    /// running: "idle" or "waking", muted.
    pub(crate) fn cloud_state_label(&self, connection: &ConnectionConfig) -> Option<&'static str> {
        let cloud = connection.cloud.as_ref()?;
        let state = self.connection.cloud.states.get(&cloud.service_id)?;
        let service = CloudService {
            id: String::new(),
            name: String::new(),
            state: state.clone(),
            endpoints: Vec::new(),
            provider: String::new(),
            region: String::new(),
        };
        if service.is_asleep() {
            Some("idle")
        } else if service.is_waking() {
            Some("waking")
        } else {
            None
        }
    }

    /// The Cloud panel: linked orgs with their services, and the key
    /// form to link (another) organization.
    pub(crate) fn cloud_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let orgs = self.preferences.cloud_orgs.clone();
        let loading = self.connection.cloud.loading;
        let service_rows =
            self.connection
                .cloud
                .services
                .iter()
                .enumerate()
                .map(|(index, (org_id, service))| {
                    let added = self.connection.connections.iter().any(|connection| {
                        connection
                            .cloud
                            .as_ref()
                            .is_some_and(|cloud| cloud.service_id == service.id)
                    });
                    let state_color = if service.is_running() {
                        theme::success()
                    } else if service.is_waking() {
                        theme::warning()
                    } else {
                        theme::text_dim()
                    };
                    let keyed = self.connection.cloud.org_has_key(&self.preferences, org_id);
                    let org_id = org_id.clone();
                    let service_id = service.id.clone();
                    let asleep = service.is_asleep();
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .rounded(px(3.))
                        .hover(|row| row.bg(theme::bg_sidebar()))
                        .child(div().size(px(7.)).rounded_full().bg(state_color))
                        .child(div().text_color(theme::text()).child(service.name.clone()))
                        .child(div().text_xs().text_color(theme::text_dim()).child(format!(
                            "{} \u{b7} {} {}",
                            service.state, service.provider, service.region
                        )))
                        .child(div().flex_1())
                        .when(asleep && keyed, |row| {
                            row.child(
                                div()
                                    .id(("cloud-start", index))
                                    .px_2()
                                    .py_0p5()
                                    .rounded(px(3.))
                                    .border_1()
                                    .border_color(theme::accent())
                                    .text_xs()
                                    .text_color(theme::accent())
                                    .child("Start")
                                    .hover(|button| button.bg(theme::hover()).cursor_pointer())
                                    .on_click(cx.listener({
                                        let org_id = org_id.clone();
                                        let service_id = service_id.clone();
                                        move |this, _, _, cx| {
                                            this.cloud_start_service(
                                                org_id.clone(),
                                                service_id.clone(),
                                                cx,
                                            )
                                        }
                                    })),
                            )
                        })
                        .when(asleep && !keyed, |row| {
                            // Sign-in tokens are read-only on the
                            // management API: an honest disabled
                            // button beats a failing one.
                            row.child(
                                div()
                                    .id(("cloud-start-disabled", index))
                                    .px_2()
                                    .py_0p5()
                                    .rounded(px(3.))
                                    .border_1()
                                    .border_color(theme::border())
                                    .text_xs()
                                    .text_color(theme::text_dim())
                                    .child("Start")
                                    .tooltip(|window, cx| {
                                        gpui_component::tooltip::Tooltip::new(
                                            "Starting from here needs an organization API key \
                                             (the browser sign-in is read-only). Connecting to \
                                             the service still wakes it; the first query takes \
                                             a minute.",
                                        )
                                        .build(window, cx)
                                    }),
                            )
                        })
                        .child(if added {
                            div()
                                .text_xs()
                                .text_color(theme::text_dim())
                                .child("Added")
                                .into_any_element()
                        } else {
                            div()
                                .id(("cloud-add", index))
                                .px_2()
                                .py_0p5()
                                .rounded(px(3.))
                                .border_1()
                                .border_color(theme::border())
                                .text_xs()
                                .text_color(theme::text())
                                .child("Add connection")
                                .hover(|button| button.bg(theme::hover()).cursor_pointer())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.cloud_add_service(index, cx)
                                }))
                                .into_any_element()
                        })
                })
                .collect::<Vec<_>>();
        let mut org_names: Vec<String> = orgs.iter().map(|org| org.name.clone()).collect();
        for org in &self.connection.cloud.oauth_orgs {
            if !orgs.iter().any(|keyed| keyed.id == org.id) {
                org_names.push(org.name.clone());
            }
        }
        let any_org = !org_names.is_empty();
        let org_line = org_names.join(", ");
        let signed_in = self.connection.cloud.signed_in;
        let account = self.connection.cloud.account.clone();
        let authorizing = self.connection.cloud.authorizing.clone();
        // Once signed in, a key is the natural next step while any
        // visible org lacks one: the section renders right under the
        // identity row, ahead of the service list, and sinks to the
        // bottom once every org is keyed.
        let unkeyed_org = self
            .connection
            .cloud
            .oauth_orgs
            .iter()
            .any(|org| !orgs.iter().any(|keyed| keyed.id == org.id));
        let key_next = signed_in && authorizing.is_none() && unkeyed_org;
        let key_section = self.cloud_key_section(
            if key_next {
                "NEXT \u{b7} LINK AN API KEY"
            } else if orgs.is_empty() {
                "API KEY \u{b7} FOR WAKING AND MANAGING"
            } else {
                "API KEY \u{b7} ANOTHER ORGANIZATION"
            },
            cx,
        );
        let (key_early, key_late) = if key_next {
            (Some(key_section), None)
        } else {
            (None, Some(key_section))
        };

        div()
            .id("cloud-panel-scroll")
            .size_full()
            .overflow_y_scroll()
            .bg(theme::bg())
            .p_6()
            .child(
                div().flex().justify_center().w_full().child(
                    div()
                        .w(px(520.))
                        .flex()
                        .flex_col()
                        .gap_4()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_lg()
                                        .text_color(theme::text())
                                        .child("ClickHouse Cloud"),
                                )
                                .child(
                                    div()
                                        .id("cloud-close")
                                        .px_1()
                                        .rounded(px(3.))
                                        .text_color(theme::text_dim())
                                        .child("\u{00d7}")
                                        .hover(|close| {
                                            close
                                                .bg(theme::hover())
                                                .text_color(theme::text())
                                                .cursor_pointer()
                                        })
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.cloud_close(cx)),
                                        ),
                                ),
                        )
                        .child(match (&authorizing, signed_in) {
                            (Some(user_code), _) => div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .p_3()
                                .rounded(px(4.))
                                .border_1()
                                .border_color(theme::border())
                                .child(div().text_xs().text_color(theme::text_dim()).child(
                                    "Approve the sign-in in your browser with this code (copied):",
                                ))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_3()
                                        .child(
                                            div()
                                                .text_lg()
                                                .font_family("Menlo")
                                                .text_color(theme::text())
                                                .child(user_code.clone()),
                                        )
                                        .child(div().flex_1())
                                        .child(
                                            div()
                                                .id("cloud-signin-cancel")
                                                .px_2()
                                                .py_0p5()
                                                .rounded(px(3.))
                                                .border_1()
                                                .border_color(theme::border())
                                                .text_xs()
                                                .text_color(theme::text_dim())
                                                .child("Cancel")
                                                .hover(|button| {
                                                    button.bg(theme::hover()).cursor_pointer()
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.cloud_sign_in_cancel(cx)
                                                })),
                                        ),
                                )
                                .into_any_element(),
                            (None, true) => div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().size(px(7.)).rounded_full().bg(theme::success()))
                                .child(div().text_sm().text_color(theme::text()).child(
                                    match account {
                                        Some(email) => format!("Signed in as {email}"),
                                        None => "Signed in to ClickHouse Cloud".to_string(),
                                    },
                                ))
                                .child(div().flex_1())
                                .child(
                                    div()
                                        .id("cloud-sign-out")
                                        .text_xs()
                                        .text_color(theme::text_dim())
                                        .child("Sign out")
                                        .hover(|button| {
                                            button.text_color(theme::danger()).cursor_pointer()
                                        })
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.cloud_sign_out(cx)),
                                        ),
                                )
                                .into_any_element(),
                            (None, false) => {
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(
                                        div().flex().child(
                                            // Brand button: the ClickHouse
                                            // mark and yellow on a dark
                                            // ground, like the console's
                                            // own sign-in.
                                            div()
                                                .id("cloud-sign-in")
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .px_3()
                                                .py_1()
                                                .rounded(px(3.))
                                                .border_1()
                                                .border_color(gpui::rgb(0xFFCC01))
                                                .bg(gpui::rgb(0x1A1710))
                                                .text_color(gpui::rgb(0xFFCC01))
                                                .child(
                                                    gpui::svg()
                                                        .path("icons/clickhouse.svg")
                                                        .size(px(12.))
                                                        .text_color(gpui::rgb(0xFFCC01)),
                                                )
                                                .child("Sign in")
                                                .hover(|button| {
                                                    button.bg(gpui::rgb(0x2A250F)).cursor_pointer()
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.cloud_sign_in(cx)
                                                })),
                                        ),
                                    )
                                    .child(div().text_xs().text_color(theme::text_dim()).child(
                                        "Approve in the browser; your organizations and services \
                                     appear here with live state. No API key needed to look.",
                                    ))
                                    .into_any_element()
                            }
                        })
                        .children(key_early)
                        .when(any_org, |panel| {
                            panel
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme::text_dim())
                                                .child(format!("LINKED \u{b7} {org_line}")),
                                        )
                                        .child(
                                            div()
                                                .id("cloud-refresh")
                                                .px_2()
                                                .py_0p5()
                                                .rounded(px(3.))
                                                .border_1()
                                                .border_color(theme::border())
                                                .text_xs()
                                                .child(if loading {
                                                    "Refreshing\u{2026}"
                                                } else {
                                                    "Refresh"
                                                })
                                                .hover(|button| {
                                                    button.bg(theme::hover()).cursor_pointer()
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.cloud_refresh(cx)
                                                })),
                                        ),
                                )
                                .child(div().flex().flex_col().gap_1().children(service_rows))
                                .children(orgs.iter().map(|org| {
                                    let org_id = org.id.clone();
                                    div()
                                        .id(gpui::SharedString::from(format!(
                                            "cloud-unlink-{}",
                                            org.id
                                        )))
                                        .text_xs()
                                        .text_color(theme::text_dim())
                                        .child(format!("Unlink {}", org.name))
                                        .hover(|button| {
                                            button.text_color(theme::danger()).cursor_pointer()
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.cloud_unlink(&org_id, cx)
                                        }))
                                }))
                        })
                        .children(key_late),
                ),
            )
    }

    /// The API-key entry block: heading, what a key adds, console
    /// deep links for unkeyed orgs, id/secret fields, and the Link
    /// button. Rendered directly under the identity row as the next
    /// step while a signed-in org lacks a key; at the bottom of the
    /// panel otherwise.
    fn cloud_key_section(&self, heading: &'static str, cx: &mut Context<Self>) -> gpui::AnyElement {
        let orgs = self.preferences.cloud_orgs.clone();
        let error = self.connection.cloud.error.clone();
        let linking = self.connection.cloud.linking;
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_xs().text_color(theme::text_dim()).child(heading))
            .child(div().text_xs().text_color(theme::text_dim()).child(
                "The sign-in is read-only; an organization API key (Admin role) also lets \
                 zeDB start idle services and provision database passwords. The key is \
                 stored in the macOS Keychain.",
            ))
            // Console links straight to each unkeyed org's API-keys
            // page: after the browser sign-in the console session
            // already exists, so this is one click and a paste back.
            .children(
                self.connection
                    .cloud
                    .oauth_orgs
                    .iter()
                    .filter(|org| !orgs.iter().any(|keyed| keyed.id == org.id))
                    .map(|org| {
                        let url = format!(
                            "https://console.clickhouse.cloud/organizations/{}/keys",
                            org.id
                        );
                        div()
                            .id(gpui::SharedString::from(format!(
                                "cloud-console-keys-{}",
                                org.id
                            )))
                            .text_xs()
                            .text_color(theme::accent())
                            .child(format!(
                                "Create or manage keys for {} in the console",
                                org.name
                            ))
                            .hover(|link| link.cursor_pointer())
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.open_url(&url);
                            }))
                    }),
            )
            .when(self.connection.cloud.oauth_orgs.is_empty(), |section| {
                section.child(
                    div()
                        .id("cloud-console-keys")
                        .text_xs()
                        .text_color(theme::accent())
                        .child("Create one in the Cloud console (Organization \u{2192} API keys)")
                        .hover(|link| link.cursor_pointer())
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.open_url("https://console.clickhouse.cloud");
                        })),
                )
            })
            .when_some(self.connection.cloud.key_id.clone(), |section, key_id| {
                section.child(Self::field("API KEY ID", key_id))
            })
            .when_some(
                self.connection.cloud.key_secret.clone(),
                |section, key_secret| section.child(Self::field("API KEY SECRET", key_secret)),
            )
            .when_some(error, |section, error| {
                section.child(div().text_xs().text_color(theme::danger()).child(error))
            })
            .child(
                div().flex().justify_end().child(
                    div()
                        .id("cloud-link")
                        .px_3()
                        .py_1()
                        .rounded(px(3.))
                        .bg(theme::primary())
                        .text_color(theme::primary_foreground())
                        .child(if linking { "Linking\u{2026}" } else { "Link" })
                        .hover(|button| button.bg(theme::primary_hover()).cursor_pointer())
                        .when(!linking, |button| {
                            button.on_click(cx.listener(|this, _, _, cx| this.cloud_link(cx)))
                        }),
                ),
            )
            .into_any_element()
    }
}
