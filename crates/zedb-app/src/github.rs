//! GitHub identity via the OAuth device flow (docs/PHASE-3.4.md M0).
//!
//! No client secret ships in the binary: the device flow is designed
//! for native apps that cannot keep one. The token lives in the macOS
//! Keychain alongside connection passwords, scope `read:user` only.

use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

/// Public OAuth app identifier; not a secret by design.
pub const CLIENT_ID: &str = "Ov23liSgqeNFWxRXVJ2a";
/// Keychain entry name for the OAuth token.
const KEYCHAIN_KEY: &str = "zedb-github-oauth";

#[derive(Clone, Debug, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u64,
    pub expires_in: u64,
}

#[derive(Clone, Debug)]
pub struct Profile {
    pub login: String,
    pub name: Option<String>,
    pub avatar: Option<PathBuf>,
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("zeDB/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| error.to_string())
}

/// Ask GitHub for a device code the user approves in their browser.
pub async fn start_device_flow() -> Result<DeviceCode, String> {
    start_device_flow_scoped("read:user").await
}

/// Same flow with an explicit scope: the settings-repo bootstrap asks
/// for `repo` this way, holds that token in memory only, and never
/// stores it; the Keychain token stays `read:user`.
pub async fn start_device_flow_scoped(scope: &str) -> Result<DeviceCode, String> {
    let response = client()?
        .post("https://github.com/login/device/code")
        .header("Accept", "application/json")
        .form(&[("client_id", CLIENT_ID), ("scope", scope)])
        .send()
        .await
        .map_err(|error| format!("could not reach GitHub: {error}"))?;
    let body = response
        .text()
        .await
        .map_err(|error| format!("unexpected device-code reply: {error}"))?;
    serde_json::from_str(&body).map_err(|error| format!("unexpected device-code reply: {error}"))
}

#[derive(Clone, Debug, Deserialize)]
pub struct RepoInfo {
    pub full_name: String,
    pub ssh_url: String,
}

/// Look up a repo the token can see. `Ok(None)` means it does not exist
/// (or is invisible to this token, which for our `repo`-scoped lookup
/// amounts to the same thing).
pub async fn get_repo(token: &str, owner: &str, name: &str) -> Result<Option<RepoInfo>, String> {
    let response = client()?
        .get(format!("https://api.github.com/repos/{owner}/{name}"))
        .header("Accept", "application/vnd.github+json")
        .bearer_auth(token)
        .send()
        .await
        .map_err(|error| format!("could not reach GitHub: {error}"))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let response = response
        .error_for_status()
        .map_err(|error| format!("GitHub refused the lookup: {error}"))?;
    let body = response
        .text()
        .await
        .map_err(|error| format!("unexpected repo reply: {error}"))?;
    serde_json::from_str(&body)
        .map(Some)
        .map_err(|error| format!("unexpected repo reply: {error}"))
}

/// Create a private repo on the user's account.
pub async fn create_private_repo(
    token: &str,
    name: &str,
    description: &str,
) -> Result<RepoInfo, String> {
    let body = serde_json::json!({
        "name": name,
        "private": true,
        "description": description,
    });
    let response = client()?
        .post("https://api.github.com/user/repos")
        .header("Accept", "application/vnd.github+json")
        .header("Content-Type", "application/json")
        .bearer_auth(token)
        .body(body.to_string())
        .send()
        .await
        .map_err(|error| format!("could not reach GitHub: {error}"))?
        .error_for_status()
        .map_err(|error| format!("GitHub refused to create the repo: {error}"))?;
    let body = response
        .text()
        .await
        .map_err(|error| format!("unexpected create reply: {error}"))?;
    serde_json::from_str(&body).map_err(|error| format!("unexpected create reply: {error}"))
}

#[derive(Deserialize)]
struct TokenReply {
    access_token: Option<String>,
    error: Option<String>,
    interval: Option<u64>,
}

/// Poll until the user approves (or the code expires). Returns the
/// access token.
pub async fn poll_for_token(device: &DeviceCode) -> Result<String, String> {
    let client = client()?;
    let mut interval = device.interval.max(1);
    let deadline = std::time::Instant::now() + Duration::from_secs(device.expires_in);
    loop {
        tokio::time::sleep(Duration::from_secs(interval)).await;
        if std::time::Instant::now() > deadline {
            return Err("the sign-in code expired; try again".into());
        }
        let reply = client
            .post("https://github.com/login/oauth/access_token")
            .header("Accept", "application/json")
            .form(&[
                ("client_id", CLIENT_ID),
                ("device_code", device.device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await
            .map_err(|error| format!("could not reach GitHub: {error}"))?
            .text()
            .await
            .map_err(|error| format!("unexpected token reply: {error}"))?;
        let reply: TokenReply = serde_json::from_str(&reply)
            .map_err(|error| format!("unexpected token reply: {error}"))?;
        if let Some(token) = reply.access_token {
            return Ok(token);
        }
        match reply.error.as_deref() {
            Some("authorization_pending") => {}
            Some("slow_down") => {
                interval = reply.interval.unwrap_or(interval + 5).max(interval + 5);
            }
            Some("expired_token") => return Err("the sign-in code expired; try again".into()),
            Some("access_denied") => return Err("sign-in was declined on GitHub".into()),
            other => {
                return Err(format!(
                    "GitHub sign-in failed: {}",
                    other.unwrap_or("unknown error")
                ))
            }
        }
    }
}

#[derive(Deserialize)]
struct UserReply {
    login: String,
    name: Option<String>,
    avatar_url: Option<String>,
}

/// Fetch the signed-in user's profile, caching the avatar to disk so
/// gpui can render it from a path.
pub async fn fetch_profile(token: &str) -> Result<Profile, String> {
    let client = client()?;
    let user = client
        .get("https://api.github.com/user")
        .header("Accept", "application/vnd.github+json")
        .bearer_auth(token)
        .send()
        .await
        .map_err(|error| format!("could not reach GitHub: {error}"))?
        .error_for_status()
        .map_err(|error| format!("GitHub rejected the token: {error}"))?
        .text()
        .await
        .map_err(|error| format!("unexpected profile reply: {error}"))?;
    let user: UserReply = serde_json::from_str(&user)
        .map_err(|error| format!("unexpected profile reply: {error}"))?;

    let avatar = match user.avatar_url.as_deref() {
        Some(url) => download_avatar(&client, url, &user.login).await,
        None => None,
    };
    Ok(Profile {
        login: user.login,
        name: user.name,
        avatar,
    })
}

async fn download_avatar(client: &reqwest::Client, url: &str, login: &str) -> Option<PathBuf> {
    let sized = if url.contains('?') {
        format!("{url}&s=128")
    } else {
        format!("{url}?s=128")
    };
    let bytes = client.get(sized).send().await.ok()?.bytes().await.ok()?;
    let directory = dirs::data_local_dir()?.join("zedb");
    std::fs::create_dir_all(&directory).ok()?;
    let path = directory.join(format!("avatar-{login}.png"));
    std::fs::write(&path, &bytes).ok()?;
    Some(path)
}

pub fn store_token(token: &str) -> Result<(), String> {
    zedb_core::secrets::set_plain(KEYCHAIN_KEY, token).map_err(|error| error.to_string())
}

pub fn stored_token() -> Option<String> {
    zedb_core::secrets::get_plain(KEYCHAIN_KEY).ok().flatten()
}

pub fn clear_token() {
    let _ = zedb_core::secrets::delete_plain(KEYCHAIN_KEY);
    // Early builds stored it behind user presence; clear that too.
    let _ = zedb_core::secrets::delete_password(KEYCHAIN_KEY);
}
