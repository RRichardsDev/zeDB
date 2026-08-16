//! The Cloud dashboard on the connection summary page: everything the
//! control plane knows about the objects behind a Cloud-linked
//! connection, in one place. Overview (per-compute facts), Cost (the
//! last 30 days of credits, daily), Backups, and Metrics (the
//! service's filtered Prometheus set). Read-only; fetched with the
//! org's API key when linked, the sign-in token otherwise.

use gpui::prelude::*;

use crate::clickhouse_cloud::{self, CloudAuth, CloudBackup, CloudService, CostReport};
use crate::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum UsageTab {
    Overview,
    Cost,
    Backups,
    Metrics,
}

impl UsageTab {
    pub(crate) const ALL: [UsageTab; 4] = [
        UsageTab::Overview,
        UsageTab::Cost,
        UsageTab::Backups,
        UsageTab::Metrics,
    ];

    fn label(self) -> &'static str {
        match self {
            UsageTab::Overview => "Overview",
            UsageTab::Cost => "Cost",
            UsageTab::Backups => "Backups",
            UsageTab::Metrics => "Metrics",
        }
    }
}

#[derive(Default)]
pub(crate) struct CloudUsageState {
    pub(crate) tab: Option<UsageTab>,
    pub(crate) loading: bool,
    pub(crate) error: Option<String>,
    /// The connection the loaded data belongs to.
    pub(crate) fetched_for: Option<String>,
    /// The warehouse's services (or the one linked service).
    pub(crate) services: Vec<CloudService>,
    pub(crate) cost: Option<CostReport>,
    pub(crate) backups: Vec<(String, Vec<CloudBackup>)>,
    pub(crate) metrics: Vec<(String, Vec<(String, f64)>)>,
    pub(crate) generation: u64,
}

impl CloudUsageState {
    fn tab(&self) -> UsageTab {
        self.tab.unwrap_or(UsageTab::Overview)
    }
}

impl Workspace {
    /// Load (or reload) the dashboard for the selected connection.
    /// No-op for non-Cloud connections and when the data is already
    /// this connection's.
    pub(crate) fn cloud_usage_refresh(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(connection) = self
            .connection
            .selected
            .and_then(|index| self.connection.connections.get(index))
        else {
            return;
        };
        let Some(cloud) = connection.cloud.clone() else {
            self.connection.usage = CloudUsageState::default();
            return;
        };
        if !force && self.connection.usage.fetched_for.as_deref() == Some(&connection.name) {
            return;
        }
        let name = connection.name.clone();
        let signed_in = cloud_oauth::signed_in();
        self.connection.usage.loading = true;
        self.connection.usage.error = None;
        self.connection.usage.fetched_for = Some(name.clone());
        self.connection.usage.generation += 1;
        let generation = self.connection.usage.generation;
        cx.notify();

        let task = rt::tokio().spawn(async move {
            let auth =
                match zedb_core::secrets::get_plain(&clickhouse_cloud::keychain_key(&cloud.org_id))
                    .ok()
                    .flatten()
                    .as_deref()
                    .and_then(clickhouse_cloud::split_credentials)
                {
                    Some((id, secret)) => CloudAuth::Basic { id, secret },
                    None if signed_in => match cloud_oauth::access_token().await {
                        Ok(Some(token)) => CloudAuth::Bearer(token),
                        _ => {
                            return Err("Sign in or link an API key to see Cloud usage".to_string())
                        }
                    },
                    None => return Err("Sign in or link an API key to see Cloud usage".to_string()),
                };

            let all = clickhouse_cloud::list_services_authed(&auth, &cloud.org_id).await?;
            let warehouse = all
                .iter()
                .find(|service| service.id == cloud.service_id)
                .and_then(|service| service.warehouse_id.clone());
            let services: Vec<CloudService> = all
                .into_iter()
                .filter(|service| match &warehouse {
                    Some(warehouse) => service.warehouse_id.as_deref() == Some(warehouse),
                    None => service.id == cloud.service_id,
                })
                .collect();

            let today = chrono::Local::now().date_naive();
            let from = today - chrono::Days::new(29);
            let cost = clickhouse_cloud::usage_cost(
                &auth,
                &cloud.org_id,
                &from.format("%Y-%m-%d").to_string(),
                &today.format("%Y-%m-%d").to_string(),
            )
            .await;

            let mut backups = Vec::new();
            let mut metrics = Vec::new();
            let mut errors: Vec<String> = Vec::new();
            if let Err(error) = &cost {
                errors.push(format!("cost: {error}"));
            }
            for service in &services {
                match clickhouse_cloud::list_backups(&auth, &cloud.org_id, &service.id).await {
                    Ok(list) => backups.push((service.name.clone(), list)),
                    Err(error) => errors.push(format!("{} backups: {error}", service.name)),
                }
                match clickhouse_cloud::service_metrics(&auth, &cloud.org_id, &service.id).await {
                    Ok(list) => metrics.push((service.name.clone(), list)),
                    Err(error) => errors.push(format!("{} metrics: {error}", service.name)),
                }
            }
            Ok((services, cost.ok(), backups, metrics, errors))
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await.unwrap_or_else(|_| Err("Fetch stopped".into()));
            this.update(cx, |this, cx| {
                if this.connection.usage.generation != generation {
                    return;
                }
                this.connection.usage.loading = false;
                match outcome {
                    Ok((services, cost, backups, metrics, errors)) => {
                        this.connection.usage.services = services;
                        this.connection.usage.cost = cost;
                        this.connection.usage.backups = backups;
                        this.connection.usage.metrics = metrics;
                        this.connection.usage.error =
                            (!errors.is_empty()).then(|| errors.join(" \u{b7} "));
                        this.connection.usage.fetched_for = Some(name);
                    }
                    Err(error) => {
                        this.connection.usage.error = Some(error);
                        this.connection.usage.fetched_for = None;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The dashboard's static part (brand row, tabs, errors); the tab
    /// bodies scroll separately via `cloud_usage_body`. None for
    /// connections without Cloud linkage.
    pub(crate) fn cloud_usage_header(
        &self,
        connection: &ConnectionConfig,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        connection.cloud.as_ref()?;
        let usage = &self.connection.usage;
        let active = usage.tab();

        let tabs = div()
            .flex()
            .items_center()
            .gap_1()
            .children(UsageTab::ALL.into_iter().map(|tab| {
                div()
                    .id(gpui::SharedString::from(format!(
                        "cloud-usage-tab-{}",
                        tab.label()
                    )))
                    .px_2()
                    .py_0p5()
                    .rounded(px(3.))
                    .text_xs()
                    .when(tab == active, |button| {
                        button.bg(theme::hover()).text_color(theme::text())
                    })
                    .when(tab != active, |button| button.text_color(theme::text_dim()))
                    .hover(|button| button.bg(theme::hover()).cursor_pointer())
                    .child(tab.label())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.connection.usage.tab = Some(tab);
                        cx.notify();
                    }))
            }))
            .child(div().flex_1())
            .child(
                div()
                    .id("cloud-usage-refresh")
                    .w(px(26.))
                    .h(px(22.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.))
                    .child(
                        gpui::svg()
                            .path("icons/refresh.svg")
                            .size(px(12.))
                            .text_color(if usage.loading {
                                theme::text()
                            } else {
                                theme::text_dim()
                            }),
                    )
                    .hover(|button| button.bg(theme::hover()).cursor_pointer())
                    .tooltip(|window, cx| {
                        gpui_component::tooltip::Tooltip::new("Refresh Cloud data")
                            .build(window, cx)
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.cloud_usage_refresh(true, cx))),
            );

        Some(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            gpui::svg()
                                .path("icons/clickhouse.svg")
                                .size(px(11.))
                                .text_color(gpui::rgb(0xFFCC01)),
                        )
                        .child(
                            div()
                                .text_color(theme::text_dim())
                                .child("ClickHouse Cloud"),
                        ),
                )
                .child(tabs)
                .when_some(usage.error.clone(), |section, error| {
                    section.child(div().text_xs().text_color(theme::danger()).child(error))
                })
                .into_any_element(),
        )
    }

    /// The active tab's body, in its own scroll region below the
    /// static header.
    pub(crate) fn cloud_usage_body(
        &self,
        connection: &ConnectionConfig,
    ) -> Option<gpui::AnyElement> {
        connection.cloud.as_ref()?;
        let usage = &self.connection.usage;
        let body = if usage.loading && usage.services.is_empty() {
            div()
                .text_sm()
                .text_color(theme::text_dim())
                .child("Fetching from the control plane\u{2026}")
                .into_any_element()
        } else {
            match usage.tab() {
                UsageTab::Overview => self.cloud_usage_overview(),
                UsageTab::Cost => self.cloud_usage_cost(),
                UsageTab::Backups => self.cloud_usage_backups(),
                UsageTab::Metrics => self.cloud_usage_metrics(),
            }
        };
        Some(
            div()
                .id("cloud-usage-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(body)
                .into_any_element(),
        )
    }

    fn cloud_usage_overview(&self) -> gpui::AnyElement {
        let usage = &self.connection.usage;
        if usage.services.is_empty() {
            return div()
                .text_sm()
                .text_color(theme::text_dim())
                .child("No Cloud data yet.")
                .into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .gap_2()
            .children(usage.services.iter().map(|service| {
                let state_color = if service.is_running() {
                    theme::success()
                } else if service.is_waking() {
                    theme::warning()
                } else {
                    theme::text_dim()
                };
                let mut facts: Vec<String> = Vec::new();
                if let Some(version) = &service.clickhouse_version {
                    facts.push(format!("ClickHouse {version}"));
                }
                if let Some(tier) = &service.tier {
                    facts.push(tier.clone());
                }
                if let (Some(replicas), Some(min), Some(max)) = (
                    service.num_replicas,
                    service.min_total_memory_gb,
                    service.max_total_memory_gb,
                ) {
                    facts.push(format!("{replicas} replicas \u{b7} {min}\u{2013}{max} GiB"));
                }
                if let Some(idle) = service.idle_timeout_minutes {
                    facts.push(format!("idles after {idle} min"));
                }
                if let Some(created) = &service.created_at {
                    let day = created.split('T').next().unwrap_or(created);
                    facts.push(format!("created {day}"));
                }
                div()
                    .p_3()
                    .rounded(px(4.))
                    .border_1()
                    .border_color(theme::border())
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().size(px(7.)).rounded_full().bg(state_color))
                            .child(div().text_color(theme::text()).child(service.name.clone()))
                            .when(service.is_primary, |row| {
                                row.child(
                                    div()
                                        .px_1()
                                        .rounded(px(3.))
                                        .border_1()
                                        .border_color(gpui::rgb(0xFFCC01))
                                        .text_xs()
                                        .text_color(gpui::rgb(0xFFCC01))
                                        .child("primary"),
                                )
                            })
                            .when(service.is_readonly, |row| {
                                row.child(
                                    div()
                                        .text_xs()
                                        .text_color(theme::text_dim())
                                        .child("read-only"),
                                )
                            })
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::text_dim())
                                    .child(service.state.clone()),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::text_dim())
                            .child(facts.join("  \u{b7}  ")),
                    )
            }))
            .into_any_element()
    }

    fn cloud_usage_cost(&self) -> gpui::AnyElement {
        let Some(cost) = &self.connection.usage.cost else {
            return div()
                .text_sm()
                .text_color(theme::text_dim())
                .child("No cost data (the credential may lack billing scope).")
                .into_any_element();
        };
        // Everything on this tab is scoped to the connection's
        // warehouse (the console's warehouse row): records match by
        // service id or by the warehouse's own id (warehouse-level
        // entities carry no service id). Other services in the org
        // stay out of the bars and the entity list; the organization
        // total survives only as the headline's second number.
        let warehouse = self
            .connection
            .usage
            .services
            .iter()
            .find_map(|service| service.warehouse_id.clone());
        let scoped: Vec<&clickhouse_cloud::CostRecord> = cost
            .costs
            .iter()
            .filter(|record| {
                record.service_id.as_deref().is_some_and(|id| {
                    self.connection
                        .usage
                        .services
                        .iter()
                        .any(|service| service.id == id)
                }) || (record.warehouse_id.is_some() && record.warehouse_id == warehouse)
            })
            .collect();
        let mut days: Vec<(String, f64)> = Vec::new();
        let mut categories = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
        let mut entities: Vec<(String, f64)> = Vec::new();
        let mut warehouse_total = 0.0f64;
        for record in &scoped {
            match days.iter_mut().find(|(date, _)| *date == record.date) {
                Some((_, total)) => *total += record.total,
                None => days.push((record.date.clone(), record.total)),
            }
            categories.0 += record.metrics.compute;
            categories.1 += record.metrics.storage;
            categories.2 += record.metrics.backup;
            categories.3 += record.metrics.data_transfer + record.metrics.public_data_transfer;
            warehouse_total += record.total;
            match entities
                .iter_mut()
                .find(|(name, _)| *name == record.entity_name)
            {
                Some((_, total)) => *total += record.total,
                None => entities.push((record.entity_name.clone(), record.total)),
            }
        }
        days.sort_by(|a, b| a.0.cmp(&b.0));
        entities.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let peak = days
            .iter()
            .map(|(_, total)| *total)
            .fold(0.0f64, f64::max)
            .max(f64::EPSILON);
        let bar_height = 56.0f32;

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_sm().text_color(theme::text()).child(format!(
                "{warehouse_total:.2} credits this warehouse \u{b7} {:.2} organization \u{b7} last 30 days",
                cost.grand_total
            )))
            .child(
                div()
                    .h(px(bar_height + 4.0))
                    .flex()
                    .items_end()
                    .gap(px(3.))
                    .children(days.iter().map(|(date, total)| {
                        let height =
                            ((total / peak) as f32 * bar_height).max(if *total > 0.0 {
                                2.0
                            } else {
                                1.0
                            });
                        div()
                            .id(gpui::SharedString::from(format!("cost-bar-{date}")))
                            .w(px(12.))
                            .h(px(height))
                            .rounded(px(2.))
                            .bg(gpui::rgb(0xFFCC01))
                            .hover(|bar| bar.bg(theme::warning()))
                            .tooltip({
                                let label = format!("{date}: {total:.2} credits");
                                move |window, cx| {
                                    gpui_component::tooltip::Tooltip::new(label.clone())
                                        .build(window, cx)
                                }
                            })
                    })),
            )
            .child(div().text_xs().text_color(theme::text_dim()).child(format!(
                "compute {:.2} \u{b7} storage {:.2} \u{b7} backups {:.2} \u{b7} transfer {:.2}",
                categories.0, categories.1, categories.2, categories.3
            )))
            .children(entities.into_iter().take(8).map(|(name, total)| {
                div()
                    .flex()
                    .gap_2()
                    .text_xs()
                    .child(
                        div()
                            .w(px(220.))
                            .flex_none()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(theme::text_dim())
                            .child(name),
                    )
                    .child(
                        div()
                            .text_color(theme::text())
                            .child(format!("{total:.2} credits")),
                    )
            }))
            .into_any_element()
    }

    fn cloud_usage_backups(&self) -> gpui::AnyElement {
        let usage = &self.connection.usage;
        if usage.backups.iter().all(|(_, list)| list.is_empty()) {
            return div()
                .text_sm()
                .text_color(theme::text_dim())
                .child("No backups reported.")
                .into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .gap_2()
            .children(usage.backups.iter().flat_map(|(service, list)| {
                let mut rows: Vec<gpui::AnyElement> = Vec::new();
                if usage.backups.len() > 1 {
                    rows.push(
                        div()
                            .text_xs()
                            .text_color(theme::text_dim())
                            .child(service.clone())
                            .into_any_element(),
                    );
                }
                for backup in list.iter().take(6) {
                    let color = match backup.status.as_str() {
                        "done" => theme::success(),
                        "error" => theme::danger(),
                        _ => theme::warning(),
                    };
                    let day = backup
                        .started_at
                        .split('T')
                        .next()
                        .unwrap_or(&backup.started_at)
                        .to_string();
                    let mut facts = vec![day, backup.status.clone()];
                    if let Some(size) = backup.size_bytes {
                        facts.push(Self::format_bytes(size));
                    }
                    if let Some(duration) = backup.duration_secs {
                        facts.push(format!("{duration}s"));
                    }
                    if let Some(kind) = &backup.kind {
                        facts.push(kind.clone());
                    }
                    rows.push(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_sm()
                            .child(div().size(px(6.)).rounded_full().bg(color))
                            .child(
                                div()
                                    .text_color(theme::text_dim())
                                    .child(facts.join("  \u{b7}  ")),
                            )
                            .into_any_element(),
                    );
                }
                rows
            }))
            .into_any_element()
    }

    fn cloud_usage_metrics(&self) -> gpui::AnyElement {
        let usage = &self.connection.usage;
        if usage.metrics.iter().all(|(_, list)| list.is_empty()) {
            return div()
                .text_sm()
                .text_color(theme::text_dim())
                .child("No metrics reported (an idle service exports nothing).")
                .into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .gap_2()
            .children(usage.metrics.iter().map(|(service, list)| {
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .when(usage.metrics.len() > 1, |section| {
                        section.child(
                            div()
                                .text_xs()
                                .text_color(theme::text_dim())
                                .child(service.clone()),
                        )
                    })
                    .children(list.iter().map(|(name, value)| {
                        let short = name
                            .trim_start_matches("ClickHouse")
                            .trim_start_matches("Metrics_")
                            .trim_start_matches("ProfileEvents_")
                            .trim_start_matches("AsyncMetrics_");
                        let display = if name.contains("Bytes") {
                            Self::format_bytes(*value as u64)
                        } else if *value >= 1000.0 {
                            Self::format_count(*value as u64)
                        } else {
                            format!("{value}")
                        };
                        div()
                            .flex()
                            .gap_2()
                            .text_xs()
                            .child(
                                div()
                                    .w(px(300.))
                                    .flex_none()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_color(theme::text_dim())
                                    .child(short.to_string()),
                            )
                            .child(div().text_color(theme::text()).child(display))
                    }))
            }))
            .into_any_element()
    }
}
