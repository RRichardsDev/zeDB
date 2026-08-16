//! ClickHouse Cloud identity via the OAuth device flow.
//!
//! Same RFC 8628 shape as the forge sign-in (`github.rs`): no client
//! secret in the binary, the refresh token lives in the macOS
//! Keychain, and the access token stays in memory only, refreshed on
//! expiry. The embedded client id is clickhousectl's public one (its
//! CLI ships it in the open); swapping in a zeDB-registered id later
//! is a one-line change. OAuth tokens are read-only on the management
//! API: listing works, waking and mutating still need an API key.

use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;

/// One control plane's auth endpoint. Production is the only one
/// verified live; staging/dev planes have their own hosts and client
/// ids (clickhousectl's KNOWN_CONFIGS) and slot in here when needed.
struct ControlPlane {
    auth_host: &'static str,
    client_id: &'static str,
}

const PRODUCTION: ControlPlane = ControlPlane {
    auth_host: "https://auth.clickhouse.cloud",
    client_id: "9q6XAueAs47R4X5d1d6FbjbJqjsrA2ZJ",
};

const SCOPE: &str = "openid profile email offline_access";
const AUDIENCE: &str = "clickhousectl";
const KEYCHAIN_KEY: &str = "zedb-clickhouse-cloud-oauth";

/// Refresh this many seconds before the token's `exp` claim, so a
/// request never departs with a token that expires in flight.
const EXPIRY_MARGIN_SECS: i64 = 60;

fn plane() -> &'static ControlPlane {
    &PRODUCTION
}

#[derive(Clone, Debug, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    /// The uri with the code baked in; preferred for opening the
    /// browser so the user only confirms.
    #[serde(default)]
    pub verification_uri_complete: Option<String>,
    pub interval: u64,
    pub expires_in: u64,
}

impl DeviceCode {
    pub fn open_url(&self) -> &str {
        self.verification_uri_complete
            .as_deref()
            .unwrap_or(&self.verification_uri)
    }
}

#[derive(Clone, Debug)]
pub struct Tokens {
    pub access: String,
    pub refresh: Option<String>,
    /// OIDC id_token, when granted: the only token whose claims name
    /// the account.
    pub identity: Option<String>,
}

/// Who is signed in, for display: never used for authorization.
#[derive(Clone, Debug, Default)]
pub struct Account {
    pub email: Option<String>,
    pub name: Option<String>,
    pub avatar: Option<std::path::PathBuf>,
}

impl Account {
    pub fn known(&self) -> bool {
        self.email.is_some() || self.name.is_some()
    }
}

fn identity_fields(claims: &serde_json::Value) -> (Option<String>, Option<String>, Option<String>) {
    let field = |key: &str| {
        claims
            .get(key)
            .and_then(|value| value.as_str())
            .map(str::to_string)
    };
    (field("email"), field("name"), field("picture"))
}

/// The signed-in account, from the id_token's claims when granted and
/// the OIDC userinfo endpoint otherwise, with the avatar cached to
/// disk so gpui can render it from a path. Best-effort: display only.
pub async fn fetch_account(access_token: &str, identity: Option<&str>) -> Account {
    let (mut email, mut name, mut picture) = identity
        .and_then(claims)
        .map(|claims| identity_fields(&claims))
        .unwrap_or_default();
    if email.is_none() {
        if let Some(userinfo) = fetch_userinfo(access_token).await {
            let (from_email, from_name, from_picture) = identity_fields(&userinfo);
            email = email.or(from_email);
            name = name.or(from_name);
            picture = picture.or(from_picture);
        }
    }
    let avatar = match picture.as_deref() {
        Some(url) => download_avatar(url).await,
        None => None,
    };
    Account {
        email,
        name,
        avatar,
    }
}

async fn fetch_userinfo(access_token: &str) -> Option<serde_json::Value> {
    let plane = plane();
    let body = client()
        .ok()?
        .get(format!("{}/userinfo", plane.auth_host))
        .bearer_auth(access_token)
        .send()
        .await
        .ok()?
        .text()
        .await
        .ok()?;
    serde_json::from_str(&body).ok()
}

async fn download_avatar(url: &str) -> Option<std::path::PathBuf> {
    let bytes = client()
        .ok()?
        .get(url)
        .send()
        .await
        .ok()?
        .bytes()
        .await
        .ok()?;
    let directory = dirs::data_local_dir()?.join("zedb");
    std::fs::create_dir_all(&directory).ok()?;
    let path = directory.join("avatar-clickhouse-cloud.png");
    std::fs::write(&path, &bytes).ok()?;
    Some(path)
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("zeDB/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| error.to_string())
}

/// Ask the auth host for a device code the user approves in the
/// browser.
pub async fn start_device_flow() -> Result<DeviceCode, String> {
    let plane = plane();
    let response = client()?
        .post(format!("{}/oauth/device/code", plane.auth_host))
        .form(&[
            ("client_id", plane.client_id),
            ("scope", SCOPE),
            ("audience", AUDIENCE),
        ])
        .send()
        .await
        .map_err(|error| format!("could not reach ClickHouse Cloud sign-in: {error}"))?;
    let body = response
        .text()
        .await
        .map_err(|error| format!("unexpected device-code reply: {error}"))?;
    serde_json::from_str(&body).map_err(|error| format!("unexpected device-code reply: {error}"))
}

#[derive(Deserialize)]
struct TokenReply {
    access_token: Option<String>,
    refresh_token: Option<String>,
    /// The OIDC identity token: the access token's claims carry no
    /// email (the auth server keeps identity out of it), so the
    /// display name comes from here or /userinfo.
    id_token: Option<String>,
    error: Option<String>,
    interval: Option<u64>,
}

fn parse_token_reply(body: &str) -> Result<TokenReply, String> {
    serde_json::from_str(body).map_err(|error| format!("unexpected token reply: {error}"))
}

/// Poll until the user approves in the browser (or the code expires).
pub async fn poll_for_tokens(device: &DeviceCode) -> Result<Tokens, String> {
    let plane = plane();
    let client = client()?;
    let mut interval = device.interval.max(1);
    let deadline = std::time::Instant::now() + Duration::from_secs(device.expires_in);
    loop {
        tokio::time::sleep(Duration::from_secs(interval)).await;
        if std::time::Instant::now() > deadline {
            return Err("the sign-in code expired; try again".into());
        }
        let body = client
            .post(format!("{}/oauth/token", plane.auth_host))
            .form(&[
                ("client_id", plane.client_id),
                ("device_code", device.device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await
            .map_err(|error| format!("could not reach ClickHouse Cloud sign-in: {error}"))?
            .text()
            .await
            .map_err(|error| format!("unexpected token reply: {error}"))?;
        let reply = parse_token_reply(&body)?;
        if let Some(access) = reply.access_token {
            return Ok(Tokens {
                access,
                refresh: reply.refresh_token,
                identity: reply.id_token,
            });
        }
        match reply.error.as_deref() {
            Some("authorization_pending") => {}
            Some("slow_down") => {
                interval = reply.interval.unwrap_or(interval + 5).max(interval + 5);
            }
            Some("expired_token") => return Err("the sign-in code expired; try again".into()),
            Some("access_denied") => return Err("sign-in was declined".into()),
            other => {
                return Err(format!(
                    "ClickHouse Cloud sign-in failed: {}",
                    other.unwrap_or("unknown error")
                ))
            }
        }
    }
}

/// Trade the stored refresh token for a fresh access token. Auth0
/// may rotate the refresh token in the reply; the caller stores it.
async fn refresh_tokens(refresh_token: &str) -> Result<Tokens, String> {
    let plane = plane();
    let body = client()?
        .post(format!("{}/oauth/token", plane.auth_host))
        .form(&[
            ("client_id", plane.client_id),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await
        .map_err(|error| format!("could not reach ClickHouse Cloud sign-in: {error}"))?
        .text()
        .await
        .map_err(|error| format!("unexpected token reply: {error}"))?;
    let reply = parse_token_reply(&body)?;
    match reply.access_token {
        Some(access) => Ok(Tokens {
            access,
            refresh: reply.refresh_token,
            identity: reply.id_token,
        }),
        None => Err(format!(
            "ClickHouse Cloud sign-in expired: {}; sign in again",
            reply.error.as_deref().unwrap_or("token refresh refused")
        )),
    }
}

// -- token claims -----------------------------------------------------

/// Decode the (unverified) claims of a JWT. The token came straight
/// from the auth host over TLS; we read claims for display and expiry
/// scheduling, never for authorization decisions.
pub fn claims(token: &str) -> Option<serde_json::Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64url_decode(payload)?;
    serde_json::from_slice(&bytes).ok()
}

fn expiry_unix(token: &str) -> Option<i64> {
    claims(token)?.get("exp").and_then(|value| value.as_i64())
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

/// A token with no readable expiry counts as expired: better one
/// spurious refresh than a request sent with a dead token.
fn is_fresh(expiry: Option<i64>, now: i64) -> bool {
    expiry.is_some_and(|expiry| now + EXPIRY_MARGIN_SECS < expiry)
}

fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    let mut bits: u32 = 0;
    let mut bit_count = 0;
    let mut bytes = Vec::with_capacity(input.len() * 3 / 4);
    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            b'=' => continue,
            _ => return None,
        };
        bits = (bits << 6) | u32::from(value);
        bit_count += 6;
        if bit_count >= 8 {
            bit_count -= 8;
            bytes.push((bits >> bit_count) as u8);
        }
    }
    Some(bytes)
}

// -- stored identity --------------------------------------------------

/// The in-memory access token, the working copy.
static ACCESS: Mutex<Option<String>> = Mutex::new(None);

/// The Keychain also holds the current access token: the auth server
/// grants no refresh token to this client (offline_access is dropped
/// upstream), so the stored access token is what lets a sign-in
/// survive a relaunch for the rest of its roughly one-day life.
const ACCESS_KEYCHAIN_KEY: &str = "zedb-clickhouse-cloud-oauth-access";

pub fn store_refresh_token(token: &str) -> Result<(), String> {
    zedb_core::secrets::set_plain(KEYCHAIN_KEY, token).map_err(|error| error.to_string())
}

/// Whether a stored credential can still open a session: a refresh
/// token, or an access token that has not expired yet.
pub fn signed_in() -> bool {
    if zedb_core::secrets::get_plain(KEYCHAIN_KEY)
        .ok()
        .flatten()
        .is_some()
    {
        return true;
    }
    zedb_core::secrets::get_plain(ACCESS_KEYCHAIN_KEY)
        .ok()
        .flatten()
        .is_some_and(|token| is_fresh(expiry_unix(&token), now_unix()))
}

pub fn sign_out() {
    let _ = zedb_core::secrets::delete_plain(KEYCHAIN_KEY);
    let _ = zedb_core::secrets::delete_plain(ACCESS_KEYCHAIN_KEY);
    *ACCESS.lock().unwrap() = None;
}

/// Remember a just-acquired access token: in memory for this run and
/// in the Keychain for the next one.
pub fn cache_access_token(token: &str) {
    *ACCESS.lock().unwrap() = Some(token.to_string());
    let _ = zedb_core::secrets::set_plain(ACCESS_KEYCHAIN_KEY, token);
}

/// A usable access token, or `Ok(None)` when signed out. Falls back
/// from the in-memory copy to the Keychain copy to a refresh-token
/// exchange (storing any rotation back).
pub async fn access_token() -> Result<Option<String>, String> {
    let cached = ACCESS.lock().unwrap().clone();
    if let Some(token) = cached {
        if is_fresh(expiry_unix(&token), now_unix()) {
            return Ok(Some(token));
        }
    }
    if let Some(token) = zedb_core::secrets::get_plain(ACCESS_KEYCHAIN_KEY)
        .ok()
        .flatten()
    {
        if is_fresh(expiry_unix(&token), now_unix()) {
            *ACCESS.lock().unwrap() = Some(token.clone());
            return Ok(Some(token));
        }
    }
    let Some(refresh) =
        zedb_core::secrets::get_plain(KEYCHAIN_KEY).map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let tokens = refresh_tokens(&refresh).await?;
    if let Some(rotated) = tokens.refresh.as_deref() {
        store_refresh_token(rotated)?;
    }
    cache_access_token(&tokens.access);
    Ok(Some(tokens.access))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_base64url() {
        assert_eq!(
            base64url_decode("aGVsbG8").as_deref(),
            Some("hello".as_bytes())
        );
        assert_eq!(
            base64url_decode("aGVsbG8=").as_deref(),
            Some("hello".as_bytes())
        );
        assert!(base64url_decode("not base64!").is_none());
    }

    fn fake_jwt(payload: &str) -> String {
        fn encode(bytes: &[u8]) -> String {
            let alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
            let mut out = String::new();
            for chunk in bytes.chunks(3) {
                let mut buffer = [0u8; 3];
                buffer[..chunk.len()].copy_from_slice(chunk);
                let bits = u32::from_be_bytes([0, buffer[0], buffer[1], buffer[2]]);
                for position in 0..=chunk.len() {
                    let shift = 18 - 6 * position;
                    let index = ((bits >> shift) & 0x3f) as usize;
                    out.push(alphabet.as_bytes()[index] as char);
                }
            }
            out
        }
        format!("{}.{}.sig", encode(b"{}"), encode(payload.as_bytes()))
    }

    #[test]
    fn reads_claims_from_a_jwt() {
        let token = fake_jwt(
            r#"{"email":"user@example.com","name":"User","picture":"p","exp":1786988782}"#,
        );
        let (email, name, picture) = identity_fields(&claims(&token).unwrap());
        assert_eq!(email.as_deref(), Some("user@example.com"));
        assert_eq!(name.as_deref(), Some("User"));
        assert_eq!(picture.as_deref(), Some("p"));
        assert_eq!(expiry_unix(&token), Some(1786988782));
        assert!(claims("not-a-jwt").is_none());
    }

    #[test]
    fn expiry_margin_forces_early_refresh() {
        let exp = 1_000_000;
        assert!(is_fresh(Some(exp), exp - EXPIRY_MARGIN_SECS - 1));
        assert!(!is_fresh(Some(exp), exp - EXPIRY_MARGIN_SECS));
        assert!(!is_fresh(Some(exp), exp + 10));
        assert!(!is_fresh(None, 0));
    }

    #[test]
    fn token_reply_shapes_parse() {
        let pending = parse_token_reply(r#"{"error":"authorization_pending"}"#).unwrap();
        assert_eq!(pending.error.as_deref(), Some("authorization_pending"));
        assert!(pending.access_token.is_none());
        let granted =
            parse_token_reply(r#"{"access_token":"a.b.c","refresh_token":"r","expires_in":86400}"#)
                .unwrap();
        assert_eq!(granted.access_token.as_deref(), Some("a.b.c"));
        assert_eq!(granted.refresh_token.as_deref(), Some("r"));
    }
}
