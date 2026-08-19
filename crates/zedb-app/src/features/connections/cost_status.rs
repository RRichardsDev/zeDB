//! The status bar's Cloud cost figure (Phase 13 slice 2): a quiet
//! credits-per-day number for the connected Cloud connection, with a
//! warning accent only when the last complete day burned clearly above
//! the month's norm. Scoped to the connection's warehouse exactly like
//! the dashboard's Cost tab; absent (never zero-faked) without data.

use gpui::prelude::*;

use crate::clickhouse_cloud::{self, CloudAuth};
use crate::*;

/// Warn when the last complete day exceeds this multiple of the
/// 30-day median...
const BURN_MULTIPLE: f64 = 1.5;
/// ...and at least this many credits, so near-zero orgs never warn.
const BURN_FLOOR_CHC: f64 = 1.0;

#[derive(Default)]
pub(crate) struct CostStatusState {
    /// The connected connection the figures belong to.
    pub(crate) connection: Option<String>,
    pub(crate) today: f64,
    pub(crate) yesterday: f64,
    /// Median of the complete days before today (up to 29).
    pub(crate) median: f64,
    /// How many complete days informed the median.
    pub(crate) days: usize,
    pub(crate) fetched_at: Option<std::time::Instant>,
}

impl CostStatusState {
    /// The last complete day burned clearly above the month's norm.
    pub(crate) fn high_burn(&self) -> bool {
        high_burn(self.yesterday, self.median, self.days)
    }
}

/// The warning rule, pure for tests: yesterday above BURN_MULTIPLE x
/// the median of the prior complete days, at least BURN_FLOOR_CHC, and
/// only with a week of history so young data cannot cry wolf.
pub(crate) fn high_burn(yesterday: f64, median: f64, days: usize) -> bool {
    days >= 7 && yesterday >= BURN_FLOOR_CHC && yesterday > median * BURN_MULTIPLE
}

/// Daily totals -> (today, yesterday, median-of-prior-days, day count).
/// `today` is the local date; days after it (clock skew) are ignored.
pub(crate) fn burn_figures(daily: &[(String, f64)], today: &str) -> (f64, f64, f64, usize) {
    let today_total = daily
        .iter()
        .filter(|(date, _)| date == today)
        .map(|(_, total)| total)
        .sum();
    let mut complete: Vec<(&String, f64)> = Vec::new();
    for (date, total) in daily {
        if date.as_str() < today {
            complete.push((date, *total));
        }
    }
    complete.sort_by(|left, right| left.0.cmp(right.0));
    let yesterday = complete.last().map(|(_, total)| *total).unwrap_or(0.0);
    let mut totals: Vec<f64> = complete.iter().map(|(_, total)| *total).collect();
    totals.sort_by(|left, right| left.partial_cmp(right).expect("cost totals are finite"));
    let median = if totals.is_empty() {
        0.0
    } else if totals.len() % 2 == 1 {
        totals[totals.len() / 2]
    } else {
        (totals[totals.len() / 2 - 1] + totals[totals.len() / 2]) / 2.0
    };
    (today_total, yesterday, median, totals.len())
}

/// A compact credits figure: two decimals under ten, one above.
pub(crate) fn format_chc(value: f64) -> String {
    if value < 10.0 {
        format!("{value:.2}")
    } else {
        format!("{value:.1}")
    }
}

impl Workspace {
    /// Fetch (or refresh) the status bar's cost for the connected Cloud
    /// connection. Quiet: failures leave the chip absent, never stale
    /// claims. `force` skips the hourly staleness guard.
    pub(crate) fn cost_status_refresh(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(connected) = self.connection.connected.as_ref() else {
            self.connection.cost_status = CostStatusState::default();
            return;
        };
        let name = connected.name.clone();
        let Some(cloud) = self
            .connection
            .connections
            .iter()
            .find(|connection| connection.name == name)
            .and_then(|connection| connection.cloud.clone())
        else {
            self.connection.cost_status = CostStatusState::default();
            return;
        };
        let fresh_enough = self
            .connection
            .cost_status
            .fetched_at
            .is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(3600));
        if !force
            && self.connection.cost_status.connection.as_deref() == Some(name.as_str())
            && fresh_enough
        {
            return;
        }
        let signed_in = cloud_oauth::signed_in();
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
                        _ => return Err(()),
                    },
                    None => return Err(()),
                };
            // The same warehouse scoping as the dashboard's Cost tab:
            // records match the warehouse members or the warehouse id.
            let all = clickhouse_cloud::list_services_authed(&auth, &cloud.org_id)
                .await
                .map_err(|_| ())?;
            let warehouse = all
                .iter()
                .find(|service| service.id == cloud.service_id)
                .and_then(|service| service.warehouse_id.clone());
            let member_ids: Vec<String> = all
                .iter()
                .filter(|service| match &warehouse {
                    Some(warehouse) => service.warehouse_id.as_deref() == Some(warehouse.as_str()),
                    None => service.id == cloud.service_id,
                })
                .map(|service| service.id.clone())
                .collect();
            let today = chrono::Local::now().date_naive();
            let from = today - chrono::Days::new(29);
            let report = clickhouse_cloud::usage_cost(
                &auth,
                &cloud.org_id,
                &from.format("%Y-%m-%d").to_string(),
                &today.format("%Y-%m-%d").to_string(),
            )
            .await
            .map_err(|_| ())?;
            let mut daily: std::collections::HashMap<String, f64> =
                std::collections::HashMap::new();
            for record in &report.costs {
                let in_scope = record
                    .service_id
                    .as_deref()
                    .is_some_and(|id| member_ids.iter().any(|member| member == id))
                    || (record.service_id.is_none()
                        && record.warehouse_id.as_deref() == warehouse.as_deref()
                        && warehouse.is_some());
                if in_scope {
                    *daily.entry(record.date.clone()).or_insert(0.0) += record.total;
                }
            }
            let daily: Vec<(String, f64)> = daily.into_iter().collect();
            Ok((daily, today.format("%Y-%m-%d").to_string()))
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok((daily, today))) = task.await else {
                return;
            };
            this.update(cx, |this, cx| {
                // Still the same connection?
                if this
                    .connection
                    .connected
                    .as_ref()
                    .is_none_or(|connected| connected.name != name)
                {
                    return;
                }
                let (today_total, yesterday, median, days) = burn_figures(&daily, &today);
                this.connection.cost_status = CostStatusState {
                    connection: Some(name),
                    today: today_total,
                    yesterday,
                    median,
                    days,
                    fetched_at: Some(std::time::Instant::now()),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(date: &str, total: f64) -> (String, f64) {
        (date.to_string(), total)
    }

    #[test]
    fn burn_figures_split_today_yesterday_and_median() {
        let daily = vec![
            day("2026-08-17", 2.0),
            day("2026-08-18", 9.0),
            day("2026-08-19", 1.5),
            day("2026-08-16", 4.0),
        ];
        let (today, yesterday, median, days) = burn_figures(&daily, "2026-08-19");
        assert_eq!(today, 1.5);
        assert_eq!(yesterday, 9.0);
        assert_eq!(median, 4.0);
        assert_eq!(days, 3);
        // No history at all.
        assert_eq!(burn_figures(&[], "2026-08-19"), (0.0, 0.0, 0.0, 0));
    }

    #[test]
    fn high_burn_needs_history_a_floor_and_the_multiple() {
        // Clear breach with a month of history.
        assert!(high_burn(9.0, 4.0, 29));
        // Under the multiple: quiet.
        assert!(!high_burn(5.0, 4.0, 29));
        // Under the credits floor: quiet even at a huge multiple.
        assert!(!high_burn(0.9, 0.1, 29));
        // Too little history: quiet.
        assert!(!high_burn(9.0, 4.0, 3));
    }

    #[test]
    fn chc_formatting_keeps_two_then_one_decimal() {
        assert_eq!(format_chc(1.234), "1.23");
        assert_eq!(format_chc(12.34), "12.3");
    }
}
