//! Client-side waiting on refreshable materialized views.
//!
//! `SYSTEM REFRESH VIEW v` only *schedules* a refresh, and
//! `SYSTEM WAIT VIEW v` waits for a refresh that is already running: sent
//! back to back, the wait usually returns before the refresh it was meant
//! to wait for has started. Either way the statement comes back in
//! milliseconds while the view is still rebuilding, so whatever runs next
//! reads the old data.
//!
//! zeDB watches `system.view_refreshes` instead. A refresh statement is
//! finished when the view has left its running states *and* has recorded a
//! refresh newer than the one in flight when the statement was sent, and
//! the failure the server logged there is reported instead of being lost.

use std::time::Duration;

use zedb_core::{QueryResult, Value};

use crate::error::{ChError, Result};
use crate::ChClient;

/// How often the view's state is re-read while waiting.
pub const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// How long to wait for a scheduled refresh to actually start before
/// giving up on it. A refresh that never starts (the view is stopped,
/// another replica holds it, the server dropped the request) must not
/// hang the run.
pub const START_GRACE: Duration = Duration::from_secs(10);

/// The view a `SYSTEM REFRESH VIEW` / `SYSTEM WAIT VIEW` statement names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshTarget {
    /// `None` when the statement named the view unqualified, in which
    /// case the connection's database applies and the lookup matches on
    /// the view name alone.
    pub database: Option<String>,
    pub view: String,
}

impl RefreshTarget {
    /// `db.view`, or just `view` when the statement left it unqualified.
    pub fn label(&self) -> String {
        match &self.database {
            Some(database) => format!("{database}.{}", self.view),
            None => self.view.clone(),
        }
    }
}

/// One row of `system.view_refreshes` for a view.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RefreshState {
    pub status: String,
    /// `last_refresh_time` as a unix timestamp; `None` while the view has
    /// never refreshed.
    pub last_refresh: Option<i64>,
    /// The server's message for the last failed refresh, empty when the
    /// last one succeeded.
    pub exception: String,
    pub progress: f64,
    pub read_rows: u64,
    pub total_rows: u64,
    pub written_rows: u64,
}

impl RefreshState {
    /// Whether a refresh is under way, here or on the replica that took
    /// it. `Scheduling` is the brief state between the request and the
    /// work starting.
    pub fn running(&self) -> bool {
        let status = self.status.as_str();
        status.starts_with("Running") || status == "Scheduling"
    }

    /// Whether this state records a refresh later than `before` did.
    fn newer_than(&self, before: Option<&RefreshState>) -> bool {
        match (
            self.last_refresh,
            before.and_then(|state| state.last_refresh),
        ) {
            (Some(now), Some(previous)) => now > previous,
            (Some(_), None) => true,
            (None, _) => false,
        }
    }
}

/// Whether the waiter should keep polling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitStep {
    Poll,
    Done,
}

/// The decision for one poll: `before` is the state captured just before
/// the statement was sent, `now` the state just read, `seen_running`
/// whether any poll so far caught the view refreshing, and `waited` how
/// long the wait has been going.
///
/// The view is done when a newer refresh is recorded, or when one that
/// was seen running has stopped. A refresh that never starts within
/// `grace` is given up on rather than waited out forever.
pub fn wait_step(
    before: Option<&RefreshState>,
    now: &RefreshState,
    seen_running: bool,
    waited: Duration,
    grace: Duration,
) -> WaitStep {
    if now.running() {
        return WaitStep::Poll;
    }
    if now.newer_than(before) || seen_running || waited >= grace {
        return WaitStep::Done;
    }
    WaitStep::Poll
}

/// The view a refresh statement targets, or `None` for anything else.
///
/// `SYSTEM STOP/START VIEW` deliberately does not match: those schedule
/// nothing and return immediately by design.
pub fn refresh_target(sql: &str) -> Option<RefreshTarget> {
    let body = strip_leading_comments(sql);
    let mut words = body.split_whitespace();
    if !words.next()?.eq_ignore_ascii_case("SYSTEM") {
        return None;
    }
    let verb = words.next()?;
    if !(verb.eq_ignore_ascii_case("REFRESH") || verb.eq_ignore_ascii_case("WAIT")) {
        return None;
    }
    if !words.next()?.eq_ignore_ascii_case("VIEW") {
        return None;
    }
    let name = words.next()?.trim_end_matches(';');
    if words.next().is_some() {
        // Anything trailing (an unknown clause) is not a shape we can
        // claim to understand; leave the statement alone.
        return None;
    }
    parse_name(name)
}

/// `db.view`, `` `db`.`view` `` or `view` into its parts. Returns `None`
/// for an empty or over-qualified name.
fn parse_name(name: &str) -> Option<RefreshTarget> {
    let mut parts = name.split('.').map(|part| part.trim_matches('`').trim());
    let first = parts.next()?;
    let second = parts.next();
    if parts.next().is_some() {
        return None;
    }
    let (database, view) = match second {
        Some(view) => (Some(first.to_string()), view.to_string()),
        None => (None, first.to_string()),
    };
    if view.is_empty() || database.as_deref().is_some_and(str::is_empty) {
        return None;
    }
    Some(RefreshTarget { database, view })
}

/// Drop leading `--` line comments and `/* */` blocks so a commented
/// migration statement is still recognized.
fn strip_leading_comments(sql: &str) -> &str {
    let mut rest = sql.trim_start();
    loop {
        if let Some(after) = rest.strip_prefix("--") {
            rest = match after.find('\n') {
                Some(end) => after[end + 1..].trim_start(),
                None => return "",
            };
            continue;
        }
        if let Some(after) = rest.strip_prefix("/*") {
            rest = match after.find("*/") {
                Some(end) => after[end + 2..].trim_start(),
                None => return "",
            };
            continue;
        }
        return rest;
    }
}

/// The `system.view_refreshes` lookup for a target.
fn state_query(target: &RefreshTarget, database: Option<&str>) -> String {
    let scope = target
        .database
        .as_deref()
        .or(database)
        .map(|database| {
            format!(
                " AND database = '{}'",
                crate::schema::escape_string(database)
            )
        })
        .unwrap_or_default();
    format!(
        "SELECT status, \
            toInt64(ifNull(toUnixTimestamp(last_refresh_time), toInt64(-1))) AS last_refresh, \
            exception, \
            toFloat64(progress) AS progress, \
            toUInt64(read_rows) AS read_rows, \
            toUInt64(total_rows) AS total_rows, \
            toUInt64(written_rows) AS written_rows \
         FROM system.view_refreshes \
         WHERE view = '{}'{scope} \
         LIMIT 1",
        crate::schema::escape_string(&target.view)
    )
}

fn string_at(row: &[Value], index: usize, label: &str) -> Result<String> {
    match row.get(index) {
        Some(Value::String(value)) => Ok(value.clone()),
        Some(Value::Enum(value)) => Ok(value.clone()),
        value => Err(ChError::Decode(format!(
            "expected String for {label}, got {value:?}"
        ))),
    }
}

fn u64_at(row: &[Value], index: usize, label: &str) -> Result<u64> {
    match row.get(index) {
        Some(Value::UInt(value)) => Ok(*value),
        value => Err(ChError::Decode(format!(
            "expected UInt64 for {label}, got {value:?}"
        ))),
    }
}

fn parse_state(result: QueryResult) -> Result<Option<RefreshState>> {
    let Some(row) = result.rows.into_iter().next() else {
        return Ok(None);
    };
    let last_refresh = match row.get(1) {
        // The lookup maps a NULL last_refresh_time (never refreshed) to
        // -1, so the column stays a plain Int64.
        Some(Value::Int(value)) if *value >= 0 => Some(*value),
        Some(Value::Int(_)) => None,
        value => {
            return Err(ChError::Decode(format!(
                "expected Int64 for last_refresh, got {value:?}"
            )))
        }
    };
    let progress = match row.get(3) {
        Some(Value::Float(value)) => *value,
        Some(Value::Null) => 0.0,
        value => {
            return Err(ChError::Decode(format!(
                "expected Float64 for progress, got {value:?}"
            )))
        }
    };
    Ok(Some(RefreshState {
        status: string_at(&row, 0, "status")?,
        last_refresh,
        exception: string_at(&row, 2, "exception")?,
        progress,
        read_rows: u64_at(&row, 4, "read_rows")?,
        total_rows: u64_at(&row, 5, "total_rows")?,
        written_rows: u64_at(&row, 6, "written_rows")?,
    }))
}

impl ChClient {
    /// This view's row in `system.view_refreshes`, or `None` when the
    /// server has none (not a refreshable view, or a name that does not
    /// resolve here).
    pub async fn view_refresh_state(&self, target: &RefreshTarget) -> Result<Option<RefreshState>> {
        let sql = state_query(target, self.cfg.database.as_deref());
        parse_state(self.query(&sql).await?)
    }

    /// Block until the refresh this statement asked for has finished,
    /// reporting the server's own failure if it failed.
    ///
    /// `before` is the state read just before the statement was sent (see
    /// [`ChClient::view_refresh_state`]); without it a refresh that
    /// completes between two polls cannot be told from one that never
    /// started, and the wait falls back to the start grace.
    ///
    /// A view with no refresh row, or a server that will not report one
    /// (no access to `system.view_refreshes`), returns immediately: the
    /// wait is an improvement on the statement's own semantics, never a
    /// new way for a run to fail.
    pub async fn wait_for_refresh(
        &self,
        target: &RefreshTarget,
        before: Option<&RefreshState>,
        mut on_state: impl FnMut(&RefreshState),
    ) -> Result<()> {
        let started = std::time::Instant::now();
        let mut seen_running = false;
        loop {
            let Ok(Some(state)) = self.view_refresh_state(target).await else {
                return Ok(());
            };
            on_state(&state);
            seen_running |= state.running();
            if wait_step(before, &state, seen_running, started.elapsed(), START_GRACE)
                == WaitStep::Done
            {
                // Only our own refresh's failure is worth raising: an
                // exception left by an earlier one is history the user
                // did not just ask for.
                let ours = seen_running || state.newer_than(before);
                if ours && !state.exception.is_empty() {
                    return Err(ChError::Server {
                        code: None,
                        message: format!(
                            "Refresh of {} failed: {}",
                            target.label(),
                            state.exception
                        ),
                    });
                }
                return Ok(());
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(database: Option<&str>, view: &str) -> RefreshTarget {
        RefreshTarget {
            database: database.map(str::to_string),
            view: view.to_string(),
        }
    }

    #[test]
    fn refresh_statements_name_their_view() {
        assert_eq!(
            refresh_target("SYSTEM REFRESH VIEW RefreshableViews.AFAS_Facts"),
            Some(target(Some("RefreshableViews"), "AFAS_Facts"))
        );
        assert_eq!(
            refresh_target("system wait view mv;"),
            Some(target(None, "mv"))
        );
        assert_eq!(
            refresh_target("-- nightly\n/* rebuild */ SYSTEM REFRESH VIEW `db`.`mv`"),
            Some(target(Some("db"), "mv"))
        );
    }

    #[test]
    fn other_statements_are_left_alone() {
        // START/STOP schedule nothing to wait for.
        assert_eq!(refresh_target("SYSTEM STOP VIEW mv"), None);
        assert_eq!(refresh_target("SYSTEM START VIEW mv"), None);
        assert_eq!(refresh_target("SELECT * FROM mv"), None);
        assert_eq!(refresh_target("SYSTEM REFRESH VIEW"), None);
        assert_eq!(refresh_target("SYSTEM REFRESH VIEW a.b.c"), None);
        // An unrecognized trailing clause: not a shape to reason about.
        assert_eq!(refresh_target("SYSTEM REFRESH VIEW mv SOMETHING"), None);
    }

    fn state(status: &str, last_refresh: Option<i64>) -> RefreshState {
        RefreshState {
            status: status.to_string(),
            last_refresh,
            ..RefreshState::default()
        }
    }

    #[test]
    fn waiting_holds_while_the_view_refreshes() {
        let before = state("Scheduled", Some(100));
        let running = state("Running", Some(100));
        assert_eq!(
            wait_step(Some(&before), &running, false, Duration::ZERO, START_GRACE),
            WaitStep::Poll
        );
        // Another replica took it: still our refresh, still waiting.
        let elsewhere = state("RunningOnAnotherReplica", Some(100));
        assert_eq!(
            wait_step(
                Some(&before),
                &elsewhere,
                false,
                Duration::ZERO,
                START_GRACE
            ),
            WaitStep::Poll
        );
    }

    #[test]
    fn waiting_ends_when_the_refresh_lands() {
        let before = state("Scheduled", Some(100));
        let done = state("Scheduled", Some(140));
        assert_eq!(
            wait_step(Some(&before), &done, false, Duration::ZERO, START_GRACE),
            WaitStep::Done
        );
        // A first-ever refresh has nothing to compare against.
        let first = state("Scheduled", Some(140));
        assert_eq!(
            wait_step(
                Some(&state("Scheduled", None)),
                &first,
                false,
                Duration::ZERO,
                START_GRACE
            ),
            WaitStep::Done
        );
    }

    #[test]
    fn a_refresh_seen_running_ends_when_it_stops() {
        // Same-second refreshes leave last_refresh_time unchanged (it is
        // a DateTime); having watched it run is the other proof.
        let before = state("Scheduled", Some(100));
        let stopped = state("Scheduled", Some(100));
        assert_eq!(
            wait_step(Some(&before), &stopped, true, Duration::ZERO, START_GRACE),
            WaitStep::Done
        );
    }

    #[test]
    fn a_refresh_that_never_starts_is_given_up_on() {
        let before = state("Disabled", Some(100));
        let unchanged = state("Disabled", Some(100));
        assert_eq!(
            wait_step(
                Some(&before),
                &unchanged,
                false,
                START_GRACE - Duration::from_millis(1),
                START_GRACE
            ),
            WaitStep::Poll
        );
        assert_eq!(
            wait_step(Some(&before), &unchanged, false, START_GRACE, START_GRACE),
            WaitStep::Done
        );
    }

    #[test]
    fn the_lookup_scopes_by_database_when_one_is_known() {
        let sql = state_query(&target(Some("Refreshable'Views"), "mv"), Some("other"));
        assert!(sql.contains("view = 'mv'"));
        assert!(
            sql.contains("database = 'Refreshable\\'Views'"),
            "the statement's own database wins and is escaped: {sql}"
        );
        // Unqualified: the connection's database scopes the lookup.
        let sql = state_query(&target(None, "mv"), Some("analytics"));
        assert!(sql.contains("database = 'analytics'"), "{sql}");
        // Neither: match on the view name alone.
        let sql = state_query(&target(None, "mv"), None);
        assert!(!sql.contains("database ="), "{sql}");
    }
}
