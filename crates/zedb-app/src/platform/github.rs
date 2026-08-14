//! Forge identity via the OAuth device flow.
//!
//! GitHub and GitLab share the RFC 8628 device flow: no client secret
//! ships in the binary, the token lives in the macOS Keychain, and the
//! basic scope is read-only profile access. Everything provider-shaped
//! hangs off [`Provider`]; the sync transport itself is plain git and
//! never touches these APIs.

use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

/// Public OAuth app identifiers; not secrets by design.
const GITHUB_CLIENT_ID: &str = "Ov23liSgqeNFWxRXVJ2a";
/// Registered GitLab OAuth application (public, scopes read_user +
/// api). Empty would make the GitLab button explain itself instead of
/// starting a flow that cannot work.
const GITLAB_CLIENT_ID: &str = "28a35fe3bcfcc90f04214d43388e014824d2167720fbaeca3c2e18f286096591";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    GitHub,
    GitLab,
}

impl Provider {
    pub const ALL: [Provider; 2] = [Provider::GitHub, Provider::GitLab];

    pub fn name(self) -> &'static str {
        match self {
            Self::GitHub => "GitHub",
            Self::GitLab => "GitLab",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::GitHub => "icons/github.svg",
            Self::GitLab => "icons/gitlab.svg",
        }
    }

    /// Whether this build carries a registered OAuth app for the
    /// provider.
    pub fn configured(self) -> bool {
        !self.client_id().is_empty()
    }

    fn client_id(self) -> &'static str {
        match self {
            Self::GitHub => GITHUB_CLIENT_ID,
            Self::GitLab => GITLAB_CLIENT_ID,
        }
    }

    fn device_code_url(self) -> &'static str {
        match self {
            Self::GitHub => "https://github.com/login/device/code",
            Self::GitLab => "https://gitlab.com/oauth/authorize_device",
        }
    }

    fn token_url(self) -> &'static str {
        match self {
            Self::GitHub => "https://github.com/login/oauth/access_token",
            Self::GitLab => "https://gitlab.com/oauth/token",
        }
    }

    /// Scope for sign-in: profile read only.
    fn basic_scope(self) -> &'static str {
        match self {
            Self::GitHub => "read:user",
            Self::GitLab => "read_user",
        }
    }

    /// Scope for the one-time settings-repo bootstrap.
    pub fn elevated_scope(self) -> &'static str {
        match self {
            Self::GitHub => "repo",
            Self::GitLab => "api",
        }
    }

    fn keychain_key(self) -> &'static str {
        match self {
            Self::GitHub => "zedb-github-oauth",
            Self::GitLab => "zedb-gitlab-oauth",
        }
    }

    pub fn ssh_url(self, owner: &str, repo: &str) -> String {
        match self {
            Self::GitHub => format!("git@github.com:{owner}/{repo}.git"),
            Self::GitLab => format!("git@gitlab.com:{owner}/{repo}.git"),
        }
    }
}

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
    pub provider: Provider,
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

/// Ask the provider for a device code the user approves in their
/// browser, with the read-only sign-in scope.
pub async fn start_device_flow(provider: Provider) -> Result<DeviceCode, String> {
    start_device_flow_scoped(provider, provider.basic_scope()).await
}

/// Same flow with an explicit scope: the settings-repo bootstrap asks
/// for the provider's elevated scope this way, holds that token in
/// memory only, and never stores it; the Keychain token keeps the
/// read-only scope.
pub async fn start_device_flow_scoped(
    provider: Provider,
    scope: &str,
) -> Result<DeviceCode, String> {
    if !provider.configured() {
        return Err(format!(
            "{} sign-in isn't wired up in this build yet",
            provider.name()
        ));
    }
    let response = client()?
        .post(provider.device_code_url())
        .header("Accept", "application/json")
        .form(&[("client_id", provider.client_id()), ("scope", scope)])
        .send()
        .await
        .map_err(|error| format!("could not reach {}: {error}", provider.name()))?;
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

#[derive(Deserialize)]
struct GitLabProject {
    path_with_namespace: String,
    ssh_url_to_repo: String,
}

impl From<GitLabProject> for RepoInfo {
    fn from(project: GitLabProject) -> Self {
        RepoInfo {
            full_name: project.path_with_namespace,
            ssh_url: project.ssh_url_to_repo,
        }
    }
}

/// Look up a repo the token can see. `Ok(None)` means it does not exist
/// (or is invisible to this token, which for our elevated lookup
/// amounts to the same thing).
pub async fn get_repo(
    provider: Provider,
    token: &str,
    owner: &str,
    name: &str,
) -> Result<Option<RepoInfo>, String> {
    let url = match provider {
        Provider::GitHub => format!("https://api.github.com/repos/{owner}/{name}"),
        Provider::GitLab => format!("https://gitlab.com/api/v4/projects/{owner}%2F{name}"),
    };
    let response = client()?
        .get(url)
        .header("Accept", "application/json")
        .bearer_auth(token)
        .send()
        .await
        .map_err(|error| format!("could not reach {}: {error}", provider.name()))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let response = response
        .error_for_status()
        .map_err(|error| format!("{} refused the lookup: {error}", provider.name()))?;
    let body = response
        .text()
        .await
        .map_err(|error| format!("unexpected repo reply: {error}"))?;
    let info = match provider {
        Provider::GitHub => serde_json::from_str::<RepoInfo>(&body)
            .map_err(|error| format!("unexpected repo reply: {error}"))?,
        Provider::GitLab => serde_json::from_str::<GitLabProject>(&body)
            .map_err(|error| format!("unexpected repo reply: {error}"))?
            .into(),
    };
    Ok(Some(info))
}

/// Create a private repo on the user's account.
pub async fn create_private_repo(
    provider: Provider,
    token: &str,
    name: &str,
    description: &str,
) -> Result<RepoInfo, String> {
    let (url, body) = match provider {
        Provider::GitHub => (
            "https://api.github.com/user/repos".to_string(),
            serde_json::json!({
                "name": name,
                "private": true,
                "description": description,
            }),
        ),
        Provider::GitLab => (
            "https://gitlab.com/api/v4/projects".to_string(),
            serde_json::json!({
                "name": name,
                "visibility": "private",
                "description": description,
            }),
        ),
    };
    let response = client()?
        .post(url)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .bearer_auth(token)
        .body(body.to_string())
        .send()
        .await
        .map_err(|error| format!("could not reach {}: {error}", provider.name()))?
        .error_for_status()
        .map_err(|error| format!("{} refused to create the repo: {error}", provider.name()))?;
    let body = response
        .text()
        .await
        .map_err(|error| format!("unexpected create reply: {error}"))?;
    let info = match provider {
        Provider::GitHub => serde_json::from_str::<RepoInfo>(&body)
            .map_err(|error| format!("unexpected create reply: {error}"))?,
        Provider::GitLab => serde_json::from_str::<GitLabProject>(&body)
            .map_err(|error| format!("unexpected create reply: {error}"))?
            .into(),
    };
    Ok(info)
}

#[derive(Deserialize)]
struct TokenReply {
    access_token: Option<String>,
    error: Option<String>,
    interval: Option<u64>,
}

/// Poll until the user approves (or the code expires). Returns the
/// access token.
pub async fn poll_for_token(provider: Provider, device: &DeviceCode) -> Result<String, String> {
    let client = client()?;
    let mut interval = device.interval.max(1);
    let deadline = std::time::Instant::now() + Duration::from_secs(device.expires_in);
    loop {
        tokio::time::sleep(Duration::from_secs(interval)).await;
        if std::time::Instant::now() > deadline {
            return Err("the sign-in code expired; try again".into());
        }
        let reply = client
            .post(provider.token_url())
            .header("Accept", "application/json")
            .form(&[
                ("client_id", provider.client_id()),
                ("device_code", device.device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await
            .map_err(|error| format!("could not reach {}: {error}", provider.name()))?
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
            Some("access_denied") => {
                return Err(format!("sign-in was declined on {}", provider.name()))
            }
            other => {
                return Err(format!(
                    "{} sign-in failed: {}",
                    provider.name(),
                    other.unwrap_or("unknown error")
                ))
            }
        }
    }
}

#[derive(Deserialize)]
struct GitHubUser {
    login: String,
    name: Option<String>,
    avatar_url: Option<String>,
}

#[derive(Deserialize)]
struct GitLabUser {
    username: String,
    name: Option<String>,
    avatar_url: Option<String>,
}

/// Fetch the signed-in user's profile, caching the avatar to disk so
/// gpui can render it from a path.
pub async fn fetch_profile(provider: Provider, token: &str) -> Result<Profile, String> {
    let client = client()?;
    let url = match provider {
        Provider::GitHub => "https://api.github.com/user",
        Provider::GitLab => "https://gitlab.com/api/v4/user",
    };
    let body = client
        .get(url)
        .header("Accept", "application/json")
        .bearer_auth(token)
        .send()
        .await
        .map_err(|error| format!("could not reach {}: {error}", provider.name()))?
        .error_for_status()
        .map_err(|error| format!("{} rejected the token: {error}", provider.name()))?
        .text()
        .await
        .map_err(|error| format!("unexpected profile reply: {error}"))?;
    let (login, name, avatar_url) = match provider {
        Provider::GitHub => {
            let user: GitHubUser = serde_json::from_str(&body)
                .map_err(|error| format!("unexpected profile reply: {error}"))?;
            (user.login, user.name, user.avatar_url)
        }
        Provider::GitLab => {
            let user: GitLabUser = serde_json::from_str(&body)
                .map_err(|error| format!("unexpected profile reply: {error}"))?;
            (user.username, user.name, user.avatar_url)
        }
    };

    let avatar = match avatar_url.as_deref() {
        Some(url) => download_avatar(&client, url, provider, &login).await,
        None => None,
    };
    Ok(Profile {
        provider,
        login,
        name,
        avatar,
    })
}

async fn download_avatar(
    client: &reqwest::Client,
    url: &str,
    provider: Provider,
    login: &str,
) -> Option<PathBuf> {
    // The size hint works on GitHub and gravatar-backed GitLab avatars;
    // uploaded GitLab avatars ignore it harmlessly.
    let sized = if url.contains('?') {
        format!("{url}&s=128")
    } else {
        format!("{url}?s=128")
    };
    let bytes = client.get(sized).send().await.ok()?.bytes().await.ok()?;
    let directory = dirs::data_local_dir()?.join("zedb");
    std::fs::create_dir_all(&directory).ok()?;
    let path = directory.join(format!(
        "avatar-{}-{login}.png",
        provider.name().to_lowercase()
    ));
    std::fs::write(&path, &bytes).ok()?;
    Some(path)
}

pub fn store_token(provider: Provider, token: &str) -> Result<(), String> {
    zedb_core::secrets::set_plain(provider.keychain_key(), token).map_err(|error| error.to_string())
}

/// The first provider with a stored token wins the startup restore;
/// zeDB keeps one signed-in identity at a time.
pub fn stored_token_any() -> Option<(Provider, String)> {
    Provider::ALL.into_iter().find_map(|provider| {
        zedb_core::secrets::get_plain(provider.keychain_key())
            .ok()
            .flatten()
            .map(|token| (provider, token))
    })
}

pub fn clear_token(provider: Provider) {
    let _ = zedb_core::secrets::delete_plain(provider.keychain_key());
    // Early builds stored the GitHub token behind user presence; clear
    // that too.
    let _ = zedb_core::secrets::delete_password(provider.keychain_key());
}
