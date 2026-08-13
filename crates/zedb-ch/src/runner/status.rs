use super::*;

pub struct DatabaseStatus {
    pub database: String,
    pub head: Option<u32>,
    pub latest: u32,
    pub pending: Vec<u32>,
    pub customised: Vec<u32>,
    pub failed: Vec<(u32, String)>,
}

impl Runner<'_> {
    /// Per-database chain position; read-only (no tracking bootstrap).
    pub async fn status(&self, targets: &Targets) -> Result<Vec<DatabaseStatus>, RunnerError> {
        let fleet = self.fleet();
        let latest = fleet.last().map(|migration| migration.number).unwrap_or(0);
        let mut statuses = Vec::new();
        for database in self.target_databases(targets).await? {
            let states = self.last_states(&database).await?;
            let applied: BTreeSet<u32> = states
                .iter()
                .filter(|(_, action, status)| {
                    status == "success" && (action == "upgrade" || action == "stamp")
                })
                .map(|(migration, _, _)| *migration)
                .collect();
            let customised: Vec<u32> = states
                .iter()
                .filter(|(_, action, status)| status == "success" && action == "apply")
                .map(|(migration, _, _)| *migration)
                .collect();
            let failed: Vec<(u32, String)> = states
                .iter()
                .filter(|(_, _, status)| status == "failed")
                .map(|(migration, action, _)| (*migration, action.clone()))
                .collect();
            let pending: Vec<u32> = fleet
                .iter()
                .filter(|migration| {
                    !applied.contains(&migration.number) && !customised.contains(&migration.number)
                })
                .map(|migration| migration.number)
                .collect();
            statuses.push(DatabaseStatus {
                database,
                head: applied.iter().next_back().copied(),
                latest,
                pending,
                customised,
                failed,
            });
        }
        Ok(statuses)
    }
}

use std::sync::LazyLock;

static NEEDS_ADMIN: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)^(OPTIMIZE|TRUNCATE|ALTER\s+TABLE|CREATE\s+FUNCTION|DROP\s+FUNCTION|SYSTEM)\b",
    )
    .expect("static regex")
});
static NEEDS_ADMIN_ANYWHERE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\bDEFINER\b").expect("static regex"));

fn body_lines(statement: &str) -> Vec<&str> {
    statement
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.trim().starts_with("--"))
        .collect()
}

/// Statements needing elevated grants, judged on the statement body.
/// Definers route to admin as belt and braces (the migration user may
/// hold SET DEFINER, but admin is the reliable executor).
pub fn needs_admin(statement: &str) -> bool {
    let lines = body_lines(statement);
    let Some(first) = lines.first() else {
        return false;
    };
    NEEDS_ADMIN.is_match(first.trim())
        || NEEDS_ADMIN_ANYWHERE.is_match(&lines.join(
            "
",
        ))
}

/// SYSTEM statements act on the connected node only (no ON CLUSTER on
/// the target servers), so the runner fans them out per replica.
pub fn is_system(statement: &str) -> bool {
    body_lines(statement).first().is_some_and(|first| {
        let trimmed = first.trim();
        trimmed.len() >= 6
            && trimmed[..6].eq_ignore_ascii_case("SYSTEM")
            && !trimmed
                .as_bytes()
                .get(6)
                .is_some_and(|c| c.is_ascii_alphanumeric())
    })
}

/// The host part of an `http://host:port` URL.
pub(crate) fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let authority = rest.split('/').next().unwrap_or(rest);
    Some(
        authority
            .rsplit_once(':')
            .map(|(host, _)| host)
            .unwrap_or(authority)
            .to_string(),
    )
}

/// Swap the host in an `http://host:port` URL, keeping scheme and port.
pub(crate) fn replace_host(url: &str, host: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("http", url));
    let (authority, path) = rest
        .split_once('/')
        .map(|(a, p)| (a, format!("/{p}")))
        .unwrap_or((rest, String::new()));
    let port = authority
        .rsplit_once(':')
        .map(|(_, port)| format!(":{port}"))
        .unwrap_or_default();
    format!("{scheme}://{host}{port}{path}")
}

/// Statements the migration user is genuinely *refused* (not merely
/// routed): what proves admin routing is load-bearing.
pub fn refused_without_admin(statement: &str) -> bool {
    body_lines(statement)
        .first()
        .is_some_and(|first| NEEDS_ADMIN.is_match(first.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_statements_are_classified_by_first_body_line() {
        assert!(is_system("-- restart the scheduler\nSYSTEM START VIEW x"));
        assert!(is_system("SYSTEM REFRESH VIEW RefreshableViews.db_X"));
        assert!(!is_system(
            "CREATE TABLE system_log (x UInt8) ENGINE = Memory"
        ));
        assert!(!is_system("SELECT * FROM system.tables"));
    }

    #[test]
    fn replica_urls_keep_scheme_and_port() {
        assert_eq!(
            host_of("http://localhost:8123").as_deref(),
            Some("localhost")
        );
        assert_eq!(
            replace_host("http://localhost:8123", "clickhouse-2"),
            "http://clickhouse-2:8123"
        );
        assert_eq!(
            replace_host("http://10.0.0.1:8443/path", "node-b"),
            "http://node-b:8443/path"
        );
    }
}
