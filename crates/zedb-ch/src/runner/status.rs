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

fn statement_body(mut statement: &str) -> Option<&str> {
    loop {
        statement = statement
            .trim_start()
            .trim_start_matches('\u{feff}')
            .trim_start();
        if let Some(comment) = statement
            .strip_prefix("--")
            .or_else(|| statement.strip_prefix('#'))
        {
            statement = comment.split_once('\n').map_or("", |(_, rest)| rest);
            continue;
        }
        if let Some(comment) = statement.strip_prefix("/*") {
            let (_, rest) = comment.split_once("*/")?;
            statement = rest;
            continue;
        }
        return (!statement.is_empty()).then_some(statement);
    }
}

/// Statements needing elevated grants, restricted to an allowlist of
/// statement forms at the start of the SQL body. Comments, literals, and
/// optional clauses cannot grant access to the admin executor.
pub fn needs_admin(statement: &str) -> bool {
    statement_body(statement).is_some_and(|body| NEEDS_ADMIN.is_match(body))
}

/// SYSTEM statements act on the connected node only (no ON CLUSTER on
/// the target servers), so the runner fans them out per replica.
pub fn is_system(statement: &str) -> bool {
    statement_body(statement).is_some_and(|body| {
        body.get(..6)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("SYSTEM"))
            && !body
                .as_bytes()
                .get(6)
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
    })
}

/// The host part of an `http://host:port` URL.
pub(crate) fn host_of(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    parsed.host_str().map(|host| {
        host.trim_start_matches('[')
            .trim_end_matches(']')
            .to_string()
    })
}

/// Swap the host in an `http://host:port` URL, keeping scheme and port.
pub(crate) fn replace_host(url: &str, host: &str) -> String {
    let Ok(mut parsed) = reqwest::Url::parse(url) else {
        return url.to_string();
    };
    let bracketed_host;
    let host = if host.contains(':') && !host.starts_with('[') {
        bracketed_host = format!("[{host}]");
        &bracketed_host
    } else {
        host
    };
    if parsed.set_host(Some(host)).is_err() {
        return url.to_string();
    }
    let mut replaced = parsed.to_string();
    if parsed.path() == "/" && !url.ends_with('/') {
        replaced.pop();
    }
    replaced
}

/// Statements the migration user is genuinely *refused* (not merely
/// routed): what proves admin routing is load-bearing.
pub fn refused_without_admin(statement: &str) -> bool {
    statement_body(statement).is_some_and(|body| NEEDS_ADMIN.is_match(body))
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
        assert!(!is_system("SYSTEM_TABLE"));
        assert!(is_system("/* scheduler */ SYSTEM START VIEW x"));
        assert!(!is_system("ééaé"));
    }

    #[test]
    fn admin_routing_requires_an_allowlisted_statement_form() {
        assert!(needs_admin("-- maintenance\nOPTIMIZE TABLE events FINAL"));
        assert!(needs_admin("ALTER TABLE events DELETE WHERE expired"));
        assert!(needs_admin("CREATE FUNCTION f AS x -> x + 1"));
        assert!(needs_admin("/* maintenance */ OPTIMIZE TABLE events FINAL"));

        assert!(!needs_admin("SELECT 1 /* DEFINER */"));
        assert!(!needs_admin("SELECT 'DEFINER'"));
        assert!(!needs_admin(
            "CREATE VIEW v DEFINER = admin SQL SECURITY DEFINER AS SELECT 1"
        ));
        assert!(!needs_admin("/* OPTIMIZE TABLE secret */ SELECT 1"));
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
        assert_eq!(
            replace_host("https://[::1]:8443/path", "2001:db8::2"),
            "https://[2001:db8::2]:8443/path"
        );
    }
}
