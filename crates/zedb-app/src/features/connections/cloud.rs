//! ClickHouse Cloud linking: paste an organization API key once, see
//! the org's services with live state, and add any of them as a
//! connection prefilled from the control plane (host, port, TLS). The
//! key goes to the Keychain; only the org id and name persist. The
//! panel fills the same connection form the user always edits, so the
//! hands-on path stays the front door.

use std::collections::HashMap;

use gpui::prelude::*;

use crate::clickhouse_cloud::{self, CloudService};
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
        }
    }
}

impl Workspace {
    pub(crate) fn cloud_open(&mut self, cx: &mut Context<Self>) {
        self.connection.cloud.open = true;
        self.connection.cloud.error = None;
        if self.connection.cloud.key_id.is_none() {
            self.connection.cloud.key_id = Some(Self::input("", "API key id", false, cx));
            self.connection.cloud.key_secret = Some(Self::input("", "API key secret", true, cx));
        }
        self.cloud_refresh(cx);
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

    /// Re-list every linked org's services: state, endpoints, and
    /// anything added or removed in the console since last look.
    pub(crate) fn cloud_refresh(&mut self, cx: &mut Context<Self>) {
        let orgs = self.preferences.cloud_orgs.clone();
        if orgs.is_empty() {
            return;
        }
        self.connection.cloud.loading = true;
        self.connection.cloud.generation += 1;
        let generation = self.connection.cloud.generation;
        cx.notify();
        let task = rt::tokio().spawn(async move {
            let mut services = Vec::new();
            let mut errors = Vec::new();
            for org in orgs {
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
            (services, errors)
        });
        cx.spawn(async move |this, cx| {
            let Ok((services, errors)) = task.await else {
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
        });
        self.notice = None;
        cx.notify();
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
        let error = self.connection.cloud.error.clone();
        let linking = self.connection.cloud.linking;
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
                        .when(asleep, |row| {
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
        let org_line = orgs
            .iter()
            .map(|org| org.name.clone())
            .collect::<Vec<_>>()
            .join(", ");

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
                        .when(!orgs.is_empty(), |panel| {
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
                        .child(div().text_xs().text_color(theme::text_dim()).child(
                            if orgs.is_empty() {
                                "LINK AN ORGANIZATION"
                            } else {
                                "LINK ANOTHER ORGANIZATION"
                            },
                        ))
                        .child(div().text_xs().text_color(theme::text_dim()).child(
                            "Create an API key in the Cloud console (Organization \u{2192} API \
                             keys); services, hosts, and ports fill themselves in. The key is \
                             stored in the macOS Keychain.",
                        ))
                        .when_some(self.connection.cloud.key_id.clone(), |panel, key_id| {
                            panel.child(Self::field("API KEY ID", key_id))
                        })
                        .when_some(
                            self.connection.cloud.key_secret.clone(),
                            |panel, key_secret| {
                                panel.child(Self::field("API KEY SECRET", key_secret))
                            },
                        )
                        .when_some(error, |panel, error| {
                            panel.child(div().text_xs().text_color(theme::danger()).child(error))
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
                                    .hover(|button| {
                                        button.bg(theme::primary_hover()).cursor_pointer()
                                    })
                                    .when(!linking, |button| {
                                        button.on_click(
                                            cx.listener(|this, _, _, cx| this.cloud_link(cx)),
                                        )
                                    }),
                            ),
                        ),
                ),
            )
    }
}
