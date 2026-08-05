//! Update check against the GitHub Releases feed.
//!
//! Runs once at startup, off the UI thread. Any failure (offline, private
//! repo without a token, malformed response) resolves to "no update" so the
//! app never nags about its own plumbing. `ZEDB_GITHUB_TOKEN` authenticates
//! the request while the repository is private, and `ZEDB_UPDATE_URL`
//! overrides the feed endpoint for testing.

use serde::Deserialize;

const RELEASES_API: &str = "https://api.github.com/repos/RRichardsDev/zeDB/releases/latest";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug)]
pub struct UpdateInfo {
    pub version: String,
    pub url: String,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

pub async fn check() -> Option<UpdateInfo> {
    let url = std::env::var("ZEDB_UPDATE_URL").unwrap_or_else(|_| RELEASES_API.to_string());
    check_at(&url).await
}

async fn check_at(url: &str) -> Option<UpdateInfo> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("zeDB/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
    let mut request = client
        .get(url)
        .header("Accept", "application/vnd.github+json");
    if let Ok(token) = std::env::var("ZEDB_GITHUB_TOKEN") {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let release: Release = serde_json::from_str(&response.text().await.ok()?).ok()?;
    if release.draft || release.prerelease {
        return None;
    }
    let version = release.tag_name.trim_start_matches('v').to_string();
    (parse(&version)? > parse(CURRENT_VERSION)?).then_some(UpdateInfo {
        version,
        url: release.html_url,
    })
}

fn parse(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.');
    let triple = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    parts.next().is_none().then_some(triple)
}

#[cfg(test)]
mod tests {
    use super::{check_at, parse};

    fn serve_release(tag: &str) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let body = format!(
            r#"{{"tag_name":"{tag}","html_url":"https://example.test/{tag}","draft":false,"prerelease":false}}"#
        );
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        format!("http://127.0.0.1:{port}/")
    }

    #[tokio::test]
    async fn newer_release_is_reported() {
        let url = serve_release("v99.0.0");
        let update = check_at(&url).await.expect("update expected");
        assert_eq!(update.version, "99.0.0");
        assert_eq!(update.url, "https://example.test/v99.0.0");
    }

    #[tokio::test]
    async fn current_release_is_not_reported() {
        let url = serve_release(concat!("v", env!("CARGO_PKG_VERSION")));
        assert!(check_at(&url).await.is_none());
    }

    #[test]
    fn parses_release_versions() {
        assert_eq!(parse("0.1.0"), Some((0, 1, 0)));
        assert_eq!(parse("12.34.56"), Some((12, 34, 56)));
        assert_eq!(parse("1.2"), None);
        assert_eq!(parse("1.2.3.4"), None);
        assert_eq!(parse("1.2.x"), None);
    }

    #[test]
    fn ordering_matches_semver_for_release_triples() {
        assert!(parse("0.2.0") > parse("0.1.9"));
        assert!(parse("1.0.0") > parse("0.99.99"));
        assert!(parse("0.1.10") > parse("0.1.9"));
        assert_eq!(parse("0.1.0"), parse("0.1.0"));
    }
}
