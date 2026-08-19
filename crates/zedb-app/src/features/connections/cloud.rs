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
    /// The signed-in account (email, name, cached avatar), once the
    /// identity has been fetched.
    pub(crate) account: Option<cloud_oauth::Account>,
    /// The user code awaiting browser approval, while polling.
    pub(crate) authorizing: Option<String>,
    /// Organizations visible through the sign-in (superset of, or
    /// disjoint from, the keyed orgs in preferences).
    pub(crate) oauth_orgs: Vec<CloudOrg>,
    pub(crate) oauth_generation: u64,
    /// Services we asked the control plane to start, with remaining
    /// refresh polls: the plane can report `idle` for a while after
    /// accepting the awake, so polling must outlive the state string.
    pub(crate) waking_watch: HashMap<String, u8>,
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
            waking_watch: HashMap::new(),
        }
    }

    /// Whether this org's Start button can work: waking is a
    /// management write, which only an API key can do.
    pub(crate) fn org_has_key(&self, preferences: &zedb_core::Preferences, org_id: &str) -> bool {
        preferences.cloud_orgs.iter().any(|org| org.id == org_id)
    }
}

/// The provider's mark and a display name for its tooltip; None for a
/// provider we have no mark for (the raw string renders instead).
fn provider_icon(provider: &str) -> Option<(&'static str, &'static str)> {
    match provider {
        "aws" => Some(("icons/provider-aws.svg", "Amazon Web Services")),
        "gcp" => Some(("icons/provider-gcp.svg", "Google Cloud")),
        "azure" => Some(("icons/provider-azure.svg", "Microsoft Azure")),
        _ => None,
    }
}

/// The flag of the country a cloud region sits in, by prefix, across
/// the AWS/GCP/Azure region-naming schemes ClickHouse Cloud offers.
/// None for an unrecognized region (the raw string still renders).
fn region_flag(region: &str) -> Option<&'static str> {
    const FLAGS: &[(&str, &str)] = &[
        ("us-", "\u{1f1fa}\u{1f1f8}"),
        ("us", "\u{1f1fa}\u{1f1f8}"),
        ("eastus", "\u{1f1fa}\u{1f1f8}"),
        ("westus", "\u{1f1fa}\u{1f1f8}"),
        ("centralus", "\u{1f1fa}\u{1f1f8}"),
        ("eu-west-1", "\u{1f1ee}\u{1f1ea}"),
        ("eu-west-2", "\u{1f1ec}\u{1f1e7}"),
        ("eu-west-3", "\u{1f1eb}\u{1f1f7}"),
        ("eu-central", "\u{1f1e9}\u{1f1ea}"),
        ("eu-north", "\u{1f1f8}\u{1f1ea}"),
        ("europe-west1", "\u{1f1e7}\u{1f1ea}"),
        ("europe-west2", "\u{1f1ec}\u{1f1e7}"),
        ("europe-west3", "\u{1f1e9}\u{1f1ea}"),
        ("europe-west4", "\u{1f1f3}\u{1f1f1}"),
        ("europe-west9", "\u{1f1eb}\u{1f1f7}"),
        ("europe-north", "\u{1f1eb}\u{1f1ee}"),
        ("northeurope", "\u{1f1ee}\u{1f1ea}"),
        ("westeurope", "\u{1f1f3}\u{1f1f1}"),
        ("germanywestcentral", "\u{1f1e9}\u{1f1ea}"),
        ("uksouth", "\u{1f1ec}\u{1f1e7}"),
        ("francecentral", "\u{1f1eb}\u{1f1f7}"),
        ("switzerlandnorth", "\u{1f1e8}\u{1f1ed}"),
        ("ap-south", "\u{1f1ee}\u{1f1f3}"),
        ("ap-southeast-1", "\u{1f1f8}\u{1f1ec}"),
        ("ap-southeast-2", "\u{1f1e6}\u{1f1fa}"),
        ("ap-northeast-1", "\u{1f1ef}\u{1f1f5}"),
        ("ap-northeast-2", "\u{1f1f0}\u{1f1f7}"),
        ("asia-south", "\u{1f1ee}\u{1f1f3}"),
        ("asia-southeast", "\u{1f1f8}\u{1f1ec}"),
        ("asia-northeast", "\u{1f1ef}\u{1f1f5}"),
        ("southeastasia", "\u{1f1f8}\u{1f1ec}"),
        ("centralindia", "\u{1f1ee}\u{1f1f3}"),
        ("japaneast", "\u{1f1ef}\u{1f1f5}"),
        ("koreacentral", "\u{1f1f0}\u{1f1f7}"),
        ("australiaeast", "\u{1f1e6}\u{1f1fa}"),
        ("sa-east", "\u{1f1e7}\u{1f1f7}"),
        ("brazilsouth", "\u{1f1e7}\u{1f1f7}"),
        ("ca-central", "\u{1f1e8}\u{1f1e6}"),
        ("canadacentral", "\u{1f1e8}\u{1f1e6}"),
        ("me-central", "\u{1f1e6}\u{1f1ea}"),
        ("af-south", "\u{1f1ff}\u{1f1e6}"),
    ];
    FLAGS
        .iter()
        .find(|(prefix, _)| region.starts_with(prefix))
        .map(|(_, flag)| *flag)
}

/// The default (editable) connection name for a shared warehouse:
/// the same honest relationship label the panel header shows. The
/// API exposes no warehouse name (a standing ask), so the label
/// states what is known instead of guessing from a renamable
/// service.
fn warehouse_label(services: &[(String, CloudService)], warehouse_id: &str) -> String {
    format!(
        "Warehouse \u{b7} {} compute \u{b7} shared data",
        warehouse_size(services, warehouse_id)
    )
}

/// How many visible services share this warehouse.
fn warehouse_size(services: &[(String, CloudService)], warehouse_id: &str) -> usize {
    services
        .iter()
        .filter(|(_, service)| service.warehouse_id.as_deref() == Some(warehouse_id))
        .count()
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
            // The auth server grants no refresh token to this client;
            // the Keychain-stored access token carries the session
            // across relaunches for its roughly one-day life.
            let durable = tokens
                .refresh
                .as_deref()
                .is_some_and(|refresh| cloud_oauth::store_refresh_token(refresh).is_ok());
            cloud_oauth::cache_access_token(&tokens.access);
            let access = tokens.access.clone();
            let identity = tokens.identity.clone();
            let account = rt::tokio()
                .spawn(
                    async move { cloud_oauth::fetch_account(&access, identity.as_deref()).await },
                )
                .await
                .ok()
                .filter(cloud_oauth::Account::known);
            this.update(cx, |this, cx| {
                if this.connection.cloud.oauth_generation != generation {
                    return;
                }
                this.connection.cloud.authorizing = None;
                this.connection.cloud.signed_in = true;
                this.connection.cloud.account = account;
                if durable {
                    this.flash_notice("Signed in to ClickHouse Cloud", cx);
                } else {
                    this.flash_notice(
                        "Signed in to ClickHouse Cloud; this session lasts about a day",
                        cx,
                    );
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
            // Orgs whose listing succeeded: only for these can a
            // missing service honestly be called deleted.
            let mut ok_orgs: Vec<String> = Vec::new();
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
                        ok_orgs.push(org.id.clone());
                        services.extend(list.into_iter().map(|service| (org.id.clone(), service)))
                    }
                    Err(error) => errors.push(format!("{}: {error}", org.name)),
                }
            }
            if signed_in {
                match cloud_oauth::access_token().await {
                    Ok(Some(token)) => {
                        account = Some(cloud_oauth::fetch_account(&token, None).await)
                            .filter(cloud_oauth::Account::known);
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
                                            Ok(list) => {
                                                ok_orgs.push(org.id.clone());
                                                services.extend(
                                                    list.into_iter()
                                                        .map(|service| (org.id.clone(), service)),
                                                )
                                            }
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
            // Warehouse-mates sit together (primary first) so the
            // panel can group them under one header.
            services.sort_by(|a, b| {
                (&a.0, &a.1.warehouse_id, !a.1.is_primary, &a.1.name).cmp(&(
                    &b.0,
                    &b.1.warehouse_id,
                    !b.1.is_primary,
                    &b.1.name,
                ))
            });
            (services, errors, oauth_orgs, account, ok_orgs)
        });
        cx.spawn(async move |this, cx| {
            let Ok((services, errors, oauth_orgs, account, ok_orgs)) = task.await else {
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
                // A linked connection whose service vanished from a
                // successfully listed org is dead, and says so; a
                // failed listing proves nothing and marks nothing.
                let deleted: Vec<String> = this
                    .connection
                    .connections
                    .iter()
                    .filter_map(|connection| connection.cloud.as_ref())
                    .filter(|cloud| ok_orgs.contains(&cloud.org_id))
                    .filter(|cloud| {
                        !this
                            .connection
                            .cloud
                            .services
                            .iter()
                            .any(|(_, service)| service.id == cloud.service_id)
                    })
                    .map(|cloud| cloud.service_id.clone())
                    .collect();
                for service_id in deleted {
                    this.connection
                        .cloud
                        .states
                        .insert(service_id, "deleted".into());
                }
                this.connection.cloud.oauth_orgs = oauth_orgs;
                if account.is_some() {
                    this.connection.cloud.account = account;
                }
                this.connection.cloud.error = (!errors.is_empty()).then(|| errors.join(" \u{b7} "));
                // Services we started stay under watch until they
                // report running (or the watch runs dry): the plane
                // can keep saying `idle` for a while after accepting
                // the awake, so the watch also keeps the row showing
                // as starting instead of bouncing back to idle.
                let fresh_states = this.connection.cloud.states.clone();
                let mut running_now = Vec::new();
                this.connection.cloud.waking_watch.retain(|id, polls| {
                    if fresh_states.get(id).map(String::as_str) == Some("running") {
                        running_now.push(id.clone());
                        return false;
                    }
                    if *polls == 0 {
                        return false;
                    }
                    *polls -= 1;
                    true
                });
                let watched: Vec<String> =
                    this.connection.cloud.waking_watch.keys().cloned().collect();
                for id in watched {
                    if let Some((_, service)) = this
                        .connection
                        .cloud
                        .services
                        .iter_mut()
                        .find(|(_, service)| service.id == id)
                    {
                        if service.is_asleep() {
                            service.state = "starting".into();
                        }
                    }
                    if this
                        .connection
                        .cloud
                        .states
                        .get(&id)
                        .is_some_and(|state| state == "idle" || state == "stopped")
                    {
                        this.connection.cloud.states.insert(id, "starting".into());
                    }
                }
                for id in running_now {
                    if let Some((_, service)) = this
                        .connection
                        .cloud
                        .services
                        .iter()
                        .find(|(_, service)| service.id == id)
                    {
                        this.flash_notice(format!("{} is running", service.name), cx);
                    }
                }
                // A waking service settles on its own schedule: keep
                // polling until nothing is mid-transition, so the
                // sidebar's "waking" clears itself.
                if !this.connection.cloud.waking_watch.is_empty()
                    || this
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
        // The state decides the command (stopped vs idle), so capture
        // it before the optimistic overwrite below.
        let previous_state = self
            .connection
            .cloud
            .services
            .iter()
            .find(|(_, service)| service.id == service_id)
            .map(|(_, service)| service.state.clone())
            .unwrap_or_default();
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
        let watch_id = service_id.clone();
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
            clickhouse_cloud::start_service(
                &key_id,
                &key_secret,
                &org_id,
                &service_id,
                &previous_state,
            )
            .await
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await.unwrap_or_else(|_| Err("Start stopped".into()));
            this.update(cx, |this, cx| match outcome {
                Ok(()) => {
                    // Watch it up to running: 24 polls at the 15s
                    // cadence is about six minutes of wake time.
                    this.connection.cloud.waking_watch.insert(watch_id, 24);
                    this.flash_notice("Waking the service; it can take a few minutes", cx);
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
        // A service in a shared warehouse defaults the connection
        // name to the warehouse relationship label; the compute
        // keeps the service's own name.
        let connection_name = service
            .warehouse_id
            .as_deref()
            .filter(|warehouse| warehouse_size(&self.connection.cloud.services, warehouse) > 1)
            .map(|warehouse| warehouse_label(&self.connection.cloud.services, warehouse))
            .unwrap_or_else(|| name.clone());
        self.connection.cloud.open = false;
        self.connection.pending_delete = None;
        self.connection.form = Some(ConnectionForm {
            editing: None,
            original_name: None,
            name: Self::input(connection_name, "staging", false, cx),
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

    /// The warehouse of the open form's first service, when known:
    /// the only warehouse whose compute may join the form.
    pub(crate) fn form_warehouse_id(&self) -> Option<String> {
        let service_id = self
            .connection
            .form
            .as_ref()
            .and_then(|form| form.cloud.as_ref())
            .map(|cloud| cloud.service_id.clone())?;
        self.connection
            .cloud
            .services
            .iter()
            .find(|(_, service)| service.id == service_id)
            .and_then(|(_, service)| service.warehouse_id.clone())
    }

    /// Append a Cloud service to the open connection form as another
    /// compute (the form's add button leads to the panel and back).
    /// The form keeps its provenance from the first service.
    pub(crate) fn cloud_add_node_to_form(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some((_, service)) = self.connection.cloud.services.get(index) else {
            return;
        };
        // A different warehouse is different data: joining it to
        // this connection would be wrong, not a preference.
        if service.warehouse_id.is_none() || service.warehouse_id != self.form_warehouse_id() {
            self.flash_warning(
                "Different warehouse, different data: only compute sharing this \
                 connection's warehouse can join it",
                cx,
            );
            return;
        }
        let Some(url) = service.https_url() else {
            self.flash_warning("The service reports no HTTPS endpoint", cx);
            return;
        };
        let name = service.name.clone();
        let port = service.native_secure_port();
        let base_name = service
            .warehouse_id
            .as_deref()
            .map(|warehouse| warehouse_label(&self.connection.cloud.services, warehouse))
            .unwrap_or_else(|| "My Cloud Cluster".to_string());
        if self.connection.form.is_none() {
            return;
        }
        let node = NodeForm {
            name: Self::input(name, "Node 1", false, cx),
            endpoint: Self::input(url, "https://host:8443", false, cx),
            native_port: Self::input(
                port.map(|port| port.to_string()).unwrap_or_default(),
                "tcp auto",
                false,
                cx,
            ),
        };
        let (name_input, first_node_name, node_count) = match self.connection.form.as_mut() {
            Some(form) => {
                form.nodes.push(node);
                (
                    form.name.clone(),
                    form.nodes[0].name.clone(),
                    form.nodes.len(),
                )
            }
            None => return,
        };
        // More than one compute makes it the warehouse, not one
        // service: swap a still-default name for the warehouse's.
        // A name the user typed themselves is left alone.
        if node_count > 1 {
            let current = name_input.read(cx).text();
            let default_name = first_node_name.read(cx).text();
            if current == default_name || current.is_empty() {
                let taken: Vec<String> = self
                    .connection
                    .connections
                    .iter()
                    .map(|connection| connection.name.clone())
                    .collect();
                let mut cluster_name = base_name.clone();
                let mut counter = 1;
                while taken.contains(&cluster_name) {
                    counter += 1;
                    cluster_name = format!("{base_name} {counter}");
                }
                name_input.update(cx, |input, cx| input.set_text(cluster_name, cx));
            }
        }
        self.connection.cloud.open = false;
        cx.notify();
    }

    /// Rotate the linked service's database password through the
    /// control plane and drop the result straight into the form's
    /// (masked) password field: saving stores it in the Keychain and
    /// the plaintext is never shown. Needs the org's API key; the
    /// form only offers this behind an explicit rotation confirm.
    pub(crate) fn cloud_provision_password(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.connection.form.as_ref() else {
            return;
        };
        let Some(cloud) = form.cloud.clone() else {
            return;
        };
        // The warehouse shares one set of users, and the control
        // plane only rotates the password on the primary service:
        // aiming at a secondary compute is a 400. Resolve the
        // primary; the provenance id stands in when the panel has no
        // fresh listing to resolve through.
        let services = &self.connection.cloud.services;
        let service_id = services
            .iter()
            .find(|(_, service)| service.id == cloud.service_id)
            .and_then(|(_, service)| service.warehouse_id.as_deref())
            .and_then(|warehouse| {
                services.iter().find(|(_, service)| {
                    service.warehouse_id.as_deref() == Some(warehouse) && service.is_primary
                })
            })
            .map(|(_, service)| service.id.clone())
            .unwrap_or_else(|| cloud.service_id.clone());
        if let Some(form) = self.connection.form.as_mut() {
            form.provision = ProvisionStage::Working;
        }
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
            clickhouse_cloud::provision_password(&key_id, &key_secret, &cloud.org_id, &service_id)
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
        let org_id = cloud.org_id.clone();
        let task = rt::tokio().spawn(async move {
            let stored =
                zedb_core::secrets::get_plain(&clickhouse_cloud::keychain_key(&cloud.org_id))
                    .ok()
                    .flatten();
            let (key_id, key_secret) = stored
                .as_deref()
                .and_then(clickhouse_cloud::split_credentials)?;
            let list = clickhouse_cloud::list_services(&key_id, &key_secret, &cloud.org_id)
                .await
                .ok()?;
            // Some(service): found. None from the ? above: could not
            // check. Some(None) below: listed and definitely gone.
            Some((
                cloud.service_id.clone(),
                list.into_iter()
                    .find(|service| service.id == cloud.service_id),
            ))
        });
        cx.spawn(async move |this, cx| {
            let Ok(Some((service_id, found))) = task.await else {
                return;
            };
            let Some(service) = found else {
                this.update(cx, |this, cx| {
                    this.connection
                        .cloud
                        .states
                        .insert(service_id, "deleted".into());
                    this.flash_warning(
                        format!(
                            "{name} no longer exists in ClickHouse Cloud (the service was \
                             deleted); this connection is dead"
                        ),
                        cx,
                    );
                    cx.notify();
                })
                .ok();
                return;
            };
            this.update(cx, |this, cx| {
                this.connection
                    .cloud
                    .states
                    .insert(service.id.clone(), service.state.clone());
                // Reaching here means the org's API key just worked (the
                // state fetch above used it), so connecting can finish
                // the job: wake the service and connect when it lands.
                // The user's connect click is the explicit wake consent.
                if service.is_asleep() {
                    this.cloud_wake_then_connect(
                        name,
                        org_id.clone(),
                        service.id.clone(),
                        service.state.clone(),
                        true,
                        cx,
                    );
                } else if service.is_waking() {
                    this.cloud_wake_then_connect(
                        name,
                        org_id.clone(),
                        service.id.clone(),
                        service.state.clone(),
                        false,
                        cx,
                    );
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Phase 13 slice 1, wake-before-connect: an asleep linked service
    /// is woken (or a waking one watched) and the connect retried when
    /// the control plane reports it running, instead of leaving the
    /// user to start it by hand and try again. Bounded at 24 polls of
    /// the 15s cadence, about six minutes of wake time. Starting any
    /// other connect abandons the watch (the `connecting` name is the
    /// guard).
    fn cloud_wake_then_connect(
        &mut self,
        name: String,
        org_id: String,
        service_id: String,
        state: String,
        wake: bool,
        cx: &mut Context<Self>,
    ) {
        self.connection.connecting = Some(name.clone());
        self.notice = Some(format!(
            "{name} is {state} in ClickHouse Cloud; waking it, then connecting \
             (takes a few minutes)\u{2026}"
        ));
        self.connection
            .cloud
            .states
            .insert(service_id.clone(), "starting".into());
        // The connection page's dashboard watches the same wake, so its
        // card flips to waking and its own polling keeps it honest.
        self.connection.usage.waking.insert(service_id.clone(), 24);
        self.cloud_usage_refresh(true, cx);
        cx.notify();
        let wake_task = wake.then(|| {
            let org_id = org_id.clone();
            let service_id = service_id.clone();
            rt::tokio().spawn(async move {
                let stored =
                    zedb_core::secrets::get_plain(&clickhouse_cloud::keychain_key(&org_id))
                        .ok()
                        .flatten();
                let Some((key_id, key_secret)) = stored
                    .as_deref()
                    .and_then(clickhouse_cloud::split_credentials)
                else {
                    return Err("No API key in the Keychain for this organization".to_string());
                };
                clickhouse_cloud::start_service(&key_id, &key_secret, &org_id, &service_id, &state)
                    .await
            })
        });
        cx.spawn(async move |this, cx| {
            if let Some(task) = wake_task {
                let outcome = task.await.unwrap_or_else(|_| Err("Wake stopped".into()));
                if let Err(error) = outcome {
                    this.update(cx, |this, cx| {
                        if this.connection.connecting.as_deref() == Some(name.as_str()) {
                            this.connection.connecting = None;
                        }
                        this.flash_warning(format!("Could not wake {name}: {error}"), cx);
                    })
                    .ok();
                    return;
                }
            }
            for _ in 0..24 {
                gpui::Timer::after(std::time::Duration::from_secs(15)).await;
                let still_waiting = this
                    .update(cx, |this, _| {
                        this.connection.connecting.as_deref() == Some(name.as_str())
                    })
                    .unwrap_or(false);
                if !still_waiting {
                    return;
                }
                let org = org_id.clone();
                let id = service_id.clone();
                let fetched = rt::tokio()
                    .spawn(async move {
                        let stored =
                            zedb_core::secrets::get_plain(&clickhouse_cloud::keychain_key(&org))
                                .ok()
                                .flatten();
                        let (key_id, key_secret) = stored
                            .as_deref()
                            .and_then(clickhouse_cloud::split_credentials)?;
                        clickhouse_cloud::list_services(&key_id, &key_secret, &org)
                            .await
                            .ok()?
                            .into_iter()
                            .find(|service| service.id == id)
                            .map(|service| service.state)
                    })
                    .await
                    .ok()
                    .flatten();
                let Some(current) = fetched else { continue };
                let done = this
                    .update(cx, |this, cx| {
                        if current == "running" {
                            this.connection
                                .cloud
                                .states
                                .insert(service_id.clone(), current.clone());
                            let connection = this
                                .connection
                                .connections
                                .iter()
                                .find(|connection| connection.name == name)
                                .cloned();
                            let Some(connection) = connection else {
                                this.connection.connecting = None;
                                cx.notify();
                                return true;
                            };
                            let password = this
                                .connection
                                .password_cache
                                .get(&name)
                                .cloned()
                                .or_else(|| zedb_core::secrets::get_password(&name).ok());
                            let Some(password) = password else {
                                this.connection.connecting = None;
                                this.flash_warning(
                                    format!("{name} is awake, but its password could not be read"),
                                    cx,
                                );
                                return true;
                            };
                            this.flash_notice(format!("{name} is awake; connecting\u{2026}"), cx);
                            this.probe_connection(connection, password, None, cx);
                            true
                        } else {
                            // Keep the sidebar honest through the wait.
                            this.connection
                                .cloud
                                .states
                                .insert(service_id.clone(), "starting".into());
                            false
                        }
                    })
                    .unwrap_or(true);
                if done {
                    return;
                }
            }
            this.update(cx, |this, cx| {
                if this.connection.connecting.as_deref() == Some(name.as_str()) {
                    this.connection.connecting = None;
                    this.flash_warning(
                        format!("{name} did not wake within ~6 minutes; connect again to retry"),
                        cx,
                    );
                    cx.notify();
                }
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

    /// The sidebar's marker for a connection whose Cloud compute is not
    /// running: "idle" or "waking", muted. Judged across the linked
    /// service's whole warehouse: the connection is usable when ANY
    /// member is up, so a stopped secondary must not brand a connection
    /// whose primary is running.
    pub(crate) fn cloud_state_label(&self, connection: &ConnectionConfig) -> Option<&'static str> {
        let cloud = connection.cloud.as_ref()?;
        // The freshest view of one service: the watch map (updated by
        // wakes, stops, and dashboard polls) over the org list.
        let state_of = |id: &str, listed: &str| -> String {
            self.connection
                .cloud
                .states
                .get(id)
                .cloned()
                .unwrap_or_else(|| listed.to_string())
        };
        let services = &self.connection.cloud.services;
        let warehouse = services
            .iter()
            .find(|(_, service)| service.id == cloud.service_id)
            .and_then(|(_, service)| service.warehouse_id.clone());
        let members: Vec<String> = match &warehouse {
            Some(warehouse) => services
                .iter()
                .filter(|(_, service)| service.warehouse_id.as_deref() == Some(warehouse.as_str()))
                .map(|(_, service)| state_of(&service.id, &service.state))
                .collect(),
            None => Vec::new(),
        };
        // Before the org list loads (or for a service outside it), the
        // linked service's own state is all there is.
        let states: Vec<String> = if members.is_empty() {
            vec![self.connection.cloud.states.get(&cloud.service_id)?.clone()]
        } else {
            members
        };
        let judge = |state: &str| CloudService {
            state: state.to_string(),
            ..CloudService::default()
        };
        if states.iter().any(|state| judge(state).is_running()) {
            return None;
        }
        if states.iter().any(|state| judge(state).is_waking()) {
            return Some("waking");
        }
        if self
            .connection
            .cloud
            .states
            .get(&cloud.service_id)
            .is_some_and(|state| state == "deleted")
        {
            return Some("deleted");
        }
        if states.iter().any(|state| judge(state).is_asleep()) {
            return Some("idle");
        }
        None
    }

    /// The Cloud panel: linked orgs with their services, and the key
    /// form to link (another) organization.
    pub(crate) fn cloud_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let orgs = self.preferences.cloud_orgs.clone();
        let loading = self.connection.cloud.loading;
        // With a form in progress, picking a service appends it as a
        // node of that form instead of starting a new connection.
        let form_open = self.connection.form.is_some();
        let form_endpoints: Vec<String> = self
            .connection
            .form
            .as_ref()
            .map(|form| {
                form.nodes
                    .iter()
                    .map(|node| node.endpoint.read(cx).text())
                    .collect()
            })
            .unwrap_or_default();
        let form_warehouse = self.form_warehouse_id();
        // Warehouse-mates (sorted together by the refresh) group
        // under one header; the header slot is per row so headers
        // interleave with the rows below.
        let services = &self.connection.cloud.services;
        let headers: Vec<Option<String>> = services
            .iter()
            .enumerate()
            .map(|(index, (org_id, service))| {
                let warehouse = service.warehouse_id.as_deref()?;
                if warehouse_size(services, warehouse) < 2 {
                    return None;
                }
                let first = index == 0
                    || services[index - 1].0 != *org_id
                    || services[index - 1].1.warehouse_id.as_deref() != Some(warehouse);
                // The API exposes no warehouse name (console-only),
                // so the header states the relationship instead of
                // guessing a name from a renamable service.
                first.then(|| {
                    format!(
                        "{} COMPUTE \u{b7} SHARED DATA",
                        warehouse_size(services, warehouse)
                    )
                })
            })
            .collect();
        let grouped: Vec<bool> = services
            .iter()
            .map(|(_, service)| {
                service
                    .warehouse_id
                    .as_deref()
                    .is_some_and(|warehouse| warehouse_size(services, warehouse) > 1)
            })
            .collect();
        let service_rows = self
            .connection
            .cloud
            .services
            .iter()
            .enumerate()
            .map(|(index, (org_id, service))| {
                let added = if form_open {
                    // The same service cannot land in the form twice.
                    service
                        .https_url()
                        .is_some_and(|url| form_endpoints.contains(&url))
                } else {
                    // Provenance only records a connection's first
                    // service; every further compute lives as a node
                    // endpoint, so both spellings count as added.
                    self.connection.connections.iter().any(|connection| {
                        connection
                            .cloud
                            .as_ref()
                            .is_some_and(|cloud| cloud.service_id == service.id)
                            || service.https_url().is_some_and(|url| {
                                connection.nodes.iter().any(|node| node.endpoint == url)
                            })
                    })
                };
                let state_color = if service.is_running() {
                    theme::success()
                } else if service.is_waking() {
                    theme::warning()
                } else {
                    theme::text_dim()
                };
                let keyed = self.connection.cloud.org_has_key(&self.preferences, org_id);
                let same_warehouse =
                    service.warehouse_id.is_some() && service.warehouse_id == form_warehouse;
                let org_id = org_id.clone();
                let service_id = service.id.clone();
                let asleep = service.is_asleep();
                let running = service.is_running();
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .when(grouped[index], |row| row.pl_4())
                    .rounded(px(3.))
                    .hover(|row| row.bg(theme::bg_sidebar()))
                    .child(div().size(px(7.)).rounded_full().bg(state_color))
                    .child(div().text_color(theme::text()).child(service.name.clone()))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_xs()
                            .text_color(theme::text_dim())
                            .child(format!("{} \u{b7}", service.state))
                            .child(match provider_icon(&service.provider) {
                                Some((icon, tooltip)) => div()
                                    .id(("cloud-provider", index))
                                    .flex()
                                    .items_center()
                                    .child(
                                        gpui::svg()
                                            .path(icon)
                                            .size(px(12.))
                                            .text_color(theme::text_dim()),
                                    )
                                    .tooltip(move |window, cx| {
                                        gpui_component::tooltip::Tooltip::new(tooltip)
                                            .build(window, cx)
                                    })
                                    .into_any_element(),
                                None => div().child(service.provider.clone()).into_any_element(),
                            })
                            .child(format!(
                                "{}{}",
                                region_flag(&service.region)
                                    .map(|flag| format!("{flag} "))
                                    .unwrap_or_default(),
                                service.region
                            )),
                    )
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
                    } else if form_open && !same_warehouse {
                        // Different warehouse, different data: this
                        // compute cannot join the form's connection.
                        div()
                            .id(("cloud-add-foreign", index))
                            .px_2()
                            .py_0p5()
                            .rounded(px(3.))
                            .border_1()
                            .border_color(theme::border())
                            .text_xs()
                            .text_color(theme::text_dim())
                            .child("Add compute")
                            .tooltip(|window, cx| {
                                gpui_component::tooltip::Tooltip::new(
                                    "This service belongs to a different warehouse, so it \
                                     holds different data; only compute sharing the \
                                     connection's warehouse can join it",
                                )
                                .build(window, cx)
                            })
                            .into_any_element()
                    } else if running {
                        div()
                            .id(("cloud-add", index))
                            .px_2()
                            .py_0p5()
                            .rounded(px(3.))
                            .border_1()
                            .border_color(theme::border())
                            .text_xs()
                            .text_color(theme::text())
                            .child(if form_open {
                                "Add compute"
                            } else {
                                "Add connection"
                            })
                            .hover(|button| button.bg(theme::hover()).cursor_pointer())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.connection.form.is_some() {
                                    this.cloud_add_node_to_form(index, cx)
                                } else {
                                    this.cloud_add_service(index, cx)
                                }
                            }))
                            .into_any_element()
                    } else {
                        // Adding wants the probe (and the password
                        // check) to hit a live service: start it
                        // first.
                        div()
                            .id(("cloud-add-disabled", index))
                            .px_2()
                            .py_0p5()
                            .rounded(px(3.))
                            .border_1()
                            .border_color(theme::border())
                            .text_xs()
                            .text_color(theme::text_dim())
                            .child(if form_open {
                                "Add compute"
                            } else {
                                "Add connection"
                            })
                            .tooltip(|window, cx| {
                                gpui_component::tooltip::Tooltip::new(
                                    "Start the service first; connections are added to a \
                                     running service",
                                )
                                .build(window, cx)
                            })
                            .into_any_element()
                    })
            })
            .collect::<Vec<_>>();
        // Interleave warehouse headers ahead of their (indented)
        // member rows.
        let service_rows: Vec<gpui::AnyElement> = headers
            .into_iter()
            .zip(service_rows)
            .flat_map(|(header, row)| {
                let mut out: Vec<gpui::AnyElement> = Vec::new();
                if let Some(label) = header {
                    out.push(
                        div()
                            .pt_1()
                            .text_xs()
                            .text_color(theme::text_dim())
                            .child(format!("WAREHOUSE \u{b7} {label}"))
                            .into_any_element(),
                    );
                }
                out.push(row.into_any_element());
                out
            })
            .collect();
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
        // The key section shows in exactly two states: as the NEXT
        // step under the identity row while a signed-in org lacks a
        // key, or at the bottom as the fallback door when there is
        // neither a sign-in nor any linked key. Any linked key hides
        // it (unlinking the org brings it back).
        let unkeyed_org = self
            .connection
            .cloud
            .oauth_orgs
            .iter()
            .any(|org| !orgs.iter().any(|keyed| keyed.id == org.id));
        let key_next = signed_in && authorizing.is_none() && unkeyed_org;
        let (key_early, key_late) = if key_next {
            (
                Some(self.cloud_key_section("NEXT \u{b7} LINK AN API KEY", cx)),
                None,
            )
        } else if !signed_in && authorizing.is_none() && orgs.is_empty() {
            (
                None,
                Some(self.cloud_key_section("API KEY \u{b7} FOR WAKING AND MANAGING", cx)),
            )
        } else {
            (None, None)
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
                        // Same width as the Preferences page, so the
                        // identity rows line up between the two.
                        .w(px(680.))
                        .max_w_full()
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
                        // Same separator the Preferences page draws
                        // under its identity row.
                        .child(
                            div()
                                .py_3()
                                .border_b_1()
                                .border_color(theme::border())
                                .child(match (&authorizing, signed_in) {
                                    // Same code presentation as the forge
                                    // bootstrap: boxed characters, click to
                                    // copy again.
                                    (Some(user_code), _) => div()
                                        .flex()
                                        .flex_col()
                                        .gap_2()
                                        .child(
                                            div()
                                                .flex()
                                                .items_start()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .text_sm()
                                                        .text_color(theme::text_dim())
                                                        .child(
                                                            "Approve the sign-in at \
                                                     auth.clickhouse.cloud (opened in your \
                                                     browser). The code is on your clipboard; \
                                                     click it to copy it again.",
                                                        ),
                                                )
                                                .child(
                                                    div()
                                                        .id("cloud-signin-cancel")
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
                                                            this.cloud_sign_in_cancel(cx)
                                                        }))
                                                        .child("Cancel"),
                                                ),
                                        )
                                        .child(div().mt_2().child(self.device_code_boxes(
                                            user_code.clone(),
                                            "cloud-signin-code",
                                            cx,
                                        )))
                                        .into_any_element(),
                                    // Same shape as the forge identity row in
                                    // Preferences: avatar, name over a marked
                                    // detail line, bordered Sign out.
                                    (None, true) => {
                                        let name = account
                                            .as_ref()
                                            .and_then(|account| account.name.clone())
                                            .or_else(|| {
                                                account
                                                    .as_ref()
                                                    .and_then(|account| account.email.clone())
                                            })
                                            .unwrap_or_else(|| "Signed in".to_string());
                                        let email = account
                                            .as_ref()
                                            .and_then(|account| account.email.clone())
                                            .unwrap_or_else(|| "ClickHouse Cloud".to_string());
                                        let avatar = account
                                            .as_ref()
                                            .and_then(|account| account.avatar.clone());
                                        div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_3()
                                            // The avatar slot is always
                                            // 36px so the row does not
                                            // shift when the picture
                                            // finishes loading.
                                            .child(match avatar {
                                                Some(avatar) => gpui::img(
                                                    gpui::ImageSource::Resource(
                                                        gpui::Resource::Path(avatar.into()),
                                                    ),
                                                )
                                                .size(px(36.))
                                                .rounded_full()
                                                .into_any_element(),
                                                None => div()
                                                    .size(px(36.))
                                                    .rounded_full()
                                                    .bg(theme::bg_sidebar())
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .child(
                                                        gpui::svg()
                                                            .path("icons/clickhouse.svg")
                                                            .size(px(14.))
                                                            .text_color(theme::text_dim()),
                                                    )
                                                    .into_any_element(),
                                            })
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .gap_0p5()
                                                    .child(
                                                        div().text_color(theme::text()).child(name),
                                                    )
                                                    .child(
                                                        div()
                                                            .flex()
                                                            .items_center()
                                                            .gap_1()
                                                            .text_sm()
                                                            .text_color(theme::text_dim())
                                                            .child(
                                                                gpui::svg()
                                                                    .path("icons/clickhouse.svg")
                                                                    .size(px(11.))
                                                                    .text_color(gpui::rgb(
                                                                        0xFFCC01,
                                                                    )),
                                                            )
                                                            .child(email),
                                                    ),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .id("cloud-sign-out")
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
                                            .on_click(
                                                cx.listener(|this, _, _, cx| {
                                                    this.cloud_sign_out(cx)
                                                }),
                                            )
                                            .child("Sign out"),
                                    )
                                    .into_any_element()
                                    }
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
                                                .bg(theme::bg_sidebar())
                                                .text_color(gpui::rgb(0xFFCC01))
                                                .child(
                                                    gpui::svg()
                                                        .path("icons/clickhouse.svg")
                                                        .size(px(12.))
                                                        .text_color(gpui::rgb(0xFFCC01)),
                                                )
                                                .child("Sign in")
                                                .hover(|button| {
                                                    button.bg(theme::hover()).cursor_pointer()
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
                                }),
                        )
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
                                                .w(px(26.))
                                                .h(px(24.))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .rounded(px(3.))
                                                .child(
                                                    gpui::svg()
                                                        .path("icons/refresh.svg")
                                                        .size(px(13.))
                                                        .text_color(if loading {
                                                            theme::text()
                                                        } else {
                                                            theme::text_dim()
                                                        }),
                                                )
                                                .hover(|button| {
                                                    button.bg(theme::hover()).cursor_pointer()
                                                })
                                                .tooltip(|window, cx| {
                                                    gpui_component::tooltip::Tooltip::new("Refresh")
                                                        .build(window, cx)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn service(name: &str, warehouse: Option<&str>, primary: bool) -> (String, CloudService) {
        (
            "org".into(),
            CloudService {
                id: name.into(),
                name: name.into(),
                state: "running".into(),
                warehouse_id: warehouse.map(str::to_string),
                is_primary: primary,
                ..CloudService::default()
            },
        )
    }

    #[test]
    fn warehouse_label_states_the_relationship() {
        let services = vec![
            service("Side Compute", Some("wh-1"), false),
            service("My Second Service", Some("wh-1"), true),
            service("Elsewhere", Some("wh-2"), true),
        ];
        assert_eq!(
            warehouse_label(&services, "wh-1"),
            "Warehouse \u{b7} 2 compute \u{b7} shared data"
        );
        assert_eq!(warehouse_size(&services, "wh-1"), 2);
        assert_eq!(warehouse_size(&services, "wh-2"), 1);
    }

    #[test]
    fn region_flags_cover_the_known_schemes() {
        assert_eq!(region_flag("eu-west-2"), Some("\u{1f1ec}\u{1f1e7}"));
        assert_eq!(region_flag("europe-west4"), Some("\u{1f1f3}\u{1f1f1}"));
        assert_eq!(
            region_flag("germanywestcentral"),
            Some("\u{1f1e9}\u{1f1ea}")
        );
        assert_eq!(region_flag("mars-central-1"), None);
    }
}
