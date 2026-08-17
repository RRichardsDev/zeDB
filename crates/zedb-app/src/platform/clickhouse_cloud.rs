//! ClickHouse Cloud control-plane client: list the organizations and
//! services an API key can see, and start an idled service. Auth is
//! the key id/secret pair as HTTP basic auth; the secret lives in the
//! Keychain (`secrets::set_plain`), never on disk.

use std::time::Duration;

use serde::Deserialize;

const API_BASE: &str = "https://api.clickhouse.cloud/v1";

/// The Keychain account for one linked organization's API key. The
/// stored value is `key_id:key_secret`.
pub fn keychain_key(org_id: &str) -> String {
    format!("zedb-clickhouse-cloud-{org_id}")
}

/// Split a stored `key_id:key_secret` Keychain value.
pub fn split_credentials(stored: &str) -> Option<(String, String)> {
    stored
        .split_once(':')
        .map(|(id, secret)| (id.to_string(), secret.to_string()))
}

#[derive(Clone, Debug, Deserialize)]
pub struct CloudOrg {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct CloudService {
    pub id: String,
    pub name: String,
    /// The server's state string, verbatim: `running`, `idle`,
    /// `stopped`, `provisioning`, `awaking`, and friends. Kept raw so
    /// new states degrade to display instead of failing to parse.
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub endpoints: Vec<CloudEndpoint>,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub region: String,
    /// The warehouse (shared object store + catalog) this service's
    /// compute is attached to. Services sharing it see the same data.
    #[serde(default, rename = "dataWarehouseId")]
    pub warehouse_id: Option<String>,
    /// The warehouse's original service; its name names the warehouse.
    #[serde(default, rename = "isPrimary")]
    pub is_primary: bool,
    #[serde(default, rename = "isReadonly")]
    pub is_readonly: bool,
    #[serde(default, rename = "numReplicas")]
    pub num_replicas: Option<u64>,
    #[serde(default, rename = "minTotalMemoryGb")]
    pub min_total_memory_gb: Option<u64>,
    #[serde(default, rename = "maxTotalMemoryGb")]
    pub max_total_memory_gb: Option<u64>,
    #[serde(default, rename = "idleTimeoutMinutes")]
    pub idle_timeout_minutes: Option<u64>,
    #[serde(default, rename = "clickhouseVersion")]
    pub clickhouse_version: Option<String>,
    #[serde(default, rename = "createdAt")]
    pub created_at: Option<String>,
    #[serde(default)]
    pub tier: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CloudEndpoint {
    pub protocol: String,
    pub host: String,
    pub port: u16,
}

impl CloudService {
    /// The HTTPS endpoint zeDB connects to, as a connection URL.
    pub fn https_url(&self) -> Option<String> {
        self.endpoints
            .iter()
            .find(|endpoint| endpoint.protocol == "https")
            .map(|endpoint| format!("https://{}:{}", endpoint.host, endpoint.port))
    }

    /// The native TCP-over-TLS port the control plane advertises;
    /// saved as the node's explicit native port so tails need no
    /// runtime discovery.
    pub fn native_secure_port(&self) -> Option<u16> {
        self.endpoints
            .iter()
            .find(|endpoint| endpoint.protocol == "nativesecure")
            .map(|endpoint| endpoint.port)
    }

    pub fn is_running(&self) -> bool {
        self.state == "running"
    }

    /// Asleep and startable: connecting will time out until it wakes.
    pub fn is_asleep(&self) -> bool {
        matches!(self.state.as_str(), "idle" | "stopped" | "paused")
    }

    /// On its way up: connecting may work any moment now.
    pub fn is_waking(&self) -> bool {
        matches!(
            self.state.as_str(),
            "awaking" | "starting" | "provisioning" | "resuming"
        )
    }
}

/// Every reply wraps the payload in `{"result": ...}`.
#[derive(Deserialize)]
struct Envelope<T> {
    result: T,
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("zeDB/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| error.to_string())
}

async fn get<T: serde::de::DeserializeOwned>(
    path: &str,
    key_id: &str,
    key_secret: &str,
) -> Result<T, String> {
    let response = client()?
        .get(format!("{API_BASE}{path}"))
        .basic_auth(key_id, Some(key_secret))
        .send()
        .await
        .map_err(|error| format!("could not reach ClickHouse Cloud: {error}"))?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err("ClickHouse Cloud rejected the API key".into());
    }
    let response = response
        .error_for_status()
        .map_err(|error| format!("ClickHouse Cloud refused the request: {error}"))?;
    let body = response
        .text()
        .await
        .map_err(|error| format!("unexpected Cloud reply: {error}"))?;
    serde_json::from_str::<Envelope<T>>(&body)
        .map(|envelope| envelope.result)
        .map_err(|error| format!("unexpected Cloud reply: {error}"))
}

async fn get_bearer<T: serde::de::DeserializeOwned>(path: &str, token: &str) -> Result<T, String> {
    let response = client()?
        .get(format!("{API_BASE}{path}"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|error| format!("could not reach ClickHouse Cloud: {error}"))?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err("ClickHouse Cloud rejected the sign-in; sign in again".into());
    }
    let response = response
        .error_for_status()
        .map_err(|error| format!("ClickHouse Cloud refused the request: {error}"))?;
    let body = response
        .text()
        .await
        .map_err(|error| format!("unexpected Cloud reply: {error}"))?;
    serde_json::from_str::<Envelope<T>>(&body)
        .map(|envelope| envelope.result)
        .map_err(|error| format!("unexpected Cloud reply: {error}"))
}

/// Either credential the control plane accepts for reads: the org API
/// key (basic auth) or the browser sign-in's Bearer token.
#[derive(Clone)]
pub enum CloudAuth {
    Basic { id: String, secret: String },
    Bearer(String),
}

impl CloudAuth {
    fn apply(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self {
            CloudAuth::Basic { id, secret } => request.basic_auth(id, Some(secret)),
            CloudAuth::Bearer(token) => request.bearer_auth(token),
        }
    }
}

async fn get_raw(path: &str, auth: &CloudAuth) -> Result<String, String> {
    let response = auth
        .apply(client()?.get(format!("{API_BASE}{path}")))
        .send()
        .await
        .map_err(|error| format!("could not reach ClickHouse Cloud: {error}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "ClickHouse Cloud refused the request: {status} {body}"
        ));
    }
    response
        .text()
        .await
        .map_err(|error| format!("unexpected Cloud reply: {error}"))
}

async fn get_authed<T: serde::de::DeserializeOwned>(
    path: &str,
    auth: &CloudAuth,
) -> Result<T, String> {
    let body = get_raw(path, auth).await?;
    serde_json::from_str::<Envelope<T>>(&body)
        .map(|envelope| envelope.result)
        .map_err(|error| format!("unexpected Cloud reply: {error}"))
}

/// One day of one billed entity's usage, in ClickHouse credits.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct CostRecord {
    #[serde(default)]
    pub date: String,
    #[serde(default, rename = "entityName")]
    pub entity_name: String,
    #[serde(default, rename = "serviceId")]
    pub service_id: Option<String>,
    #[serde(default, rename = "dataWarehouseId")]
    pub warehouse_id: Option<String>,
    #[serde(default, rename = "totalCHC")]
    pub total: f64,
    #[serde(default)]
    pub metrics: CostMetrics,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct CostMetrics {
    #[serde(default, rename = "computeCHC")]
    pub compute: f64,
    #[serde(default, rename = "storageCHC")]
    pub storage: f64,
    #[serde(default, rename = "backupCHC")]
    pub backup: f64,
    #[serde(default, rename = "dataTransferCHC")]
    pub data_transfer: f64,
    #[serde(default, rename = "publicDataTransferCHC")]
    pub public_data_transfer: f64,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct CostReport {
    #[serde(default, rename = "grandTotalCHC")]
    pub grand_total: f64,
    #[serde(default)]
    pub costs: Vec<CostRecord>,
}

/// The organization's usage cost between two dates (YYYY-MM-DD,
/// inclusive), in ClickHouse credits, one record per entity per day.
pub async fn usage_cost(
    auth: &CloudAuth,
    org_id: &str,
    from_date: &str,
    to_date: &str,
) -> Result<CostReport, String> {
    let body = get_raw(
        &format!("/organizations/{org_id}/usageCost?from_date={from_date}&to_date={to_date}"),
        auth,
    )
    .await?;
    serde_json::from_str::<Envelope<CostReport>>(&body)
        .map(|envelope| envelope.result)
        .map_err(|error| format!("unexpected Cloud cost reply: {error}"))
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct CloudBackup {
    #[serde(default)]
    pub status: String,
    #[serde(default, rename = "startedAt")]
    pub started_at: String,
    // Numeric fields arrive as floats (durationInSeconds: 42.717),
    // so nothing here assumes integers.
    #[serde(default, rename = "sizeInBytes")]
    pub size_bytes: Option<f64>,
    #[serde(default, rename = "durationInSeconds")]
    pub duration_secs: Option<f64>,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
}

/// The service's backups, newest first as the API returns them.
pub async fn list_backups(
    auth: &CloudAuth,
    org_id: &str,
    service_id: &str,
) -> Result<Vec<CloudBackup>, String> {
    get_authed(
        &format!("/organizations/{org_id}/services/{service_id}/backups"),
        auth,
    )
    .await
}

/// The service's key metrics: the Prometheus endpoint with
/// `filtered_metrics` on, parsed to (name, value) pairs. Gauges and
/// counters only; histogram/summary series are skipped.
pub async fn service_metrics(
    auth: &CloudAuth,
    org_id: &str,
    service_id: &str,
) -> Result<Vec<(String, f64)>, String> {
    let text = get_raw(
        &format!("/organizations/{org_id}/services/{service_id}/prometheus?filtered_metrics=true"),
        auth,
    )
    .await?;
    Ok(parse_prometheus(&text))
}

/// Parse Prometheus text exposition to (metric name, value), summing
/// series that share a name (per-label breakdowns collapse).
pub fn parse_prometheus(text: &str) -> Vec<(String, f64)> {
    let mut metrics: Vec<(String, f64)> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((series, value)) = line.rsplit_once(' ') else {
            continue;
        };
        let Ok(value) = value.trim().parse::<f64>() else {
            continue;
        };
        let name = series.split('{').next().unwrap_or(series).trim();
        if name.is_empty() {
            continue;
        }
        match metrics.iter_mut().find(|(existing, _)| existing == name) {
            Some((_, sum)) => *sum += value,
            None => metrics.push((name.to_string(), value)),
        }
    }
    metrics
}

/// Every service in the organization, with either credential.
pub async fn list_services_authed(
    auth: &CloudAuth,
    org_id: &str,
) -> Result<Vec<CloudService>, String> {
    get_authed(&format!("/organizations/{org_id}/services"), auth).await
}

/// The organizations this API key can see (org-scoped keys see one).
pub async fn list_organizations(key_id: &str, key_secret: &str) -> Result<Vec<CloudOrg>, String> {
    get("/organizations", key_id, key_secret).await
}

/// The signed-in user's organizations (OAuth Bearer; read-only).
pub async fn list_organizations_bearer(token: &str) -> Result<Vec<CloudOrg>, String> {
    get_bearer("/organizations", token).await
}

/// Every service in the organization, via the sign-in token.
pub async fn list_services_bearer(token: &str, org_id: &str) -> Result<Vec<CloudService>, String> {
    get_bearer(&format!("/organizations/{org_id}/services"), token).await
}

/// Every service in the organization, with live state and endpoints.
pub async fn list_services(
    key_id: &str,
    key_secret: &str,
    org_id: &str,
) -> Result<Vec<CloudService>, String> {
    get(
        &format!("/organizations/{org_id}/services"),
        key_id,
        key_secret,
    )
    .await
}

/// Ask the control plane to bring a sleeping service up. The state
/// endpoint's commands are start/stop/awake, and they are not
/// interchangeable: `start` applies to an explicitly stopped service,
/// `awake` to an auto-idled one (sending `start` to an idle service
/// does nothing). Waking takes a while; callers poll `list_services`
/// for the state change.
pub async fn start_service(
    key_id: &str,
    key_secret: &str,
    org_id: &str,
    service_id: &str,
    state: &str,
) -> Result<(), String> {
    let command = if state == "stopped" { "start" } else { "awake" };
    let response = client()?
        .patch(format!(
            "{API_BASE}/organizations/{org_id}/services/{service_id}/state"
        ))
        .basic_auth(key_id, Some(key_secret))
        .header("content-type", "application/json")
        .body(format!(r#"{{"command":"{command}"}}"#))
        .send()
        .await
        .map_err(|error| format!("could not reach ClickHouse Cloud: {error}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "ClickHouse Cloud refused the {command}: {status} {body}"
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
struct PasswordReply {
    #[serde(default)]
    password: Option<String>,
}

/// Rotate the service's database password to a server-generated one
/// and return it. Destructive by design (the old password stops
/// working); callers confirm with the user first.
pub async fn provision_password(
    key_id: &str,
    key_secret: &str,
    org_id: &str,
    service_id: &str,
) -> Result<String, String> {
    let response = client()?
        .patch(format!(
            "{API_BASE}/organizations/{org_id}/services/{service_id}/password"
        ))
        .basic_auth(key_id, Some(key_secret))
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .map_err(|error| format!("could not reach ClickHouse Cloud: {error}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "ClickHouse Cloud refused the password reset: {status} {body}"
        ));
    }
    let body = response
        .text()
        .await
        .map_err(|error| format!("unexpected Cloud reply: {error}"))?;
    serde_json::from_str::<Envelope<PasswordReply>>(&body)
        .ok()
        .and_then(|envelope| envelope.result.password)
        .ok_or_else(|| {
            "ClickHouse Cloud reset the password but returned none; paste one instead".into()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_service_envelope() {
        let raw = r#"{"status": 200, "result": [{
            "id": "abc-123",
            "name": "analytics",
            "state": "idle",
            "provider": "aws",
            "region": "eu-west-1",
            "endpoints": [
                {"protocol": "nativesecure", "host": "x.clickhouse.cloud", "port": 9440},
                {"protocol": "https", "host": "x.clickhouse.cloud", "port": 8443}
            ]
        }]}"#;
        let services: Envelope<Vec<CloudService>> = serde_json::from_str(raw).unwrap();
        let service = &services.result[0];
        assert_eq!(service.name, "analytics");
        assert!(service.is_asleep());
        assert_eq!(
            service.https_url().as_deref(),
            Some("https://x.clickhouse.cloud:8443")
        );
    }

    #[test]
    fn parses_prometheus_exposition() {
        let text = "# HELP ClickHouseMetrics_Query Number of queries\n\
            # TYPE ClickHouseMetrics_Query gauge\n\
            ClickHouseMetrics_Query 3\n\
            ClickHouseProfileEvents_ReadBytes{host=\"a\"} 100\n\
            ClickHouseProfileEvents_ReadBytes{host=\"b\"} 50\n\
            malformed line without value x\n";
        let metrics = parse_prometheus(text);
        assert_eq!(metrics[0], ("ClickHouseMetrics_Query".to_string(), 3.0));
        assert_eq!(
            metrics[1],
            ("ClickHouseProfileEvents_ReadBytes".to_string(), 150.0)
        );
        assert_eq!(metrics.len(), 2);
    }

    #[test]
    fn parses_warehouse_fields() {
        let raw = r#"{"result": [
            {"id": "a", "name": "svc", "state": "running",
             "dataWarehouseId": "wh-1", "isPrimary": true},
            {"id": "b", "name": "side", "state": "running",
             "dataWarehouseId": "wh-1"}
        ]}"#;
        let services: Envelope<Vec<CloudService>> = serde_json::from_str(raw).unwrap();
        assert_eq!(services.result[0].warehouse_id.as_deref(), Some("wh-1"));
        assert!(services.result[0].is_primary);
        assert!(!services.result[1].is_primary);
    }

    #[test]
    fn tolerates_unknown_state_and_missing_endpoints() {
        let raw = r#"{"result": [{"id": "a", "name": "n", "state": "someday-new"}]}"#;
        let services: Envelope<Vec<CloudService>> = serde_json::from_str(raw).unwrap();
        let service = &services.result[0];
        assert!(!service.is_running() && !service.is_asleep() && !service.is_waking());
        assert!(service.https_url().is_none());
    }

    #[test]
    fn credentials_roundtrip() {
        assert_eq!(
            split_credentials("key:secret:with:colons"),
            Some(("key".into(), "secret:with:colons".into()))
        );
        assert_eq!(split_credentials("no-separator"), None);
    }
}
