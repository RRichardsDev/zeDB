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

#[derive(Clone, Debug, Deserialize)]
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

/// The organizations this API key can see (org-scoped keys see one).
pub async fn list_organizations(key_id: &str, key_secret: &str) -> Result<Vec<CloudOrg>, String> {
    get("/organizations", key_id, key_secret).await
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

/// Ask the control plane to start an idled/stopped service. Waking
/// takes a while; callers poll `list_services` for the state change.
pub async fn start_service(
    key_id: &str,
    key_secret: &str,
    org_id: &str,
    service_id: &str,
) -> Result<(), String> {
    let response = client()?
        .patch(format!(
            "{API_BASE}/organizations/{org_id}/services/{service_id}/state"
        ))
        .basic_auth(key_id, Some(key_secret))
        .header("content-type", "application/json")
        .body(r#"{"command":"start"}"#)
        .send()
        .await
        .map_err(|error| format!("could not reach ClickHouse Cloud: {error}"))?;
    response
        .error_for_status()
        .map(|_| ())
        .map_err(|error| format!("ClickHouse Cloud refused the start: {error}"))
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
