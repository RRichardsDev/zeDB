//! Self-update against the GitHub Releases feed.
//!
//! `check` runs once at startup, off the UI thread. Any failure (offline,
//! private repo without a token, malformed response) resolves to "no update"
//! so the app never nags about its own plumbing. `ZEDB_GITHUB_TOKEN`
//! authenticates requests while the repository is private, and
//! `ZEDB_UPDATE_URL` overrides the feed endpoint for testing.
//!
//! `download_and_install` fetches the release archive, extracts it, verifies
//! the new bundle is signed by our team before trusting it, and swaps it into
//! place; the app keeps running from the old inode until relaunch.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

const RELEASES_API: &str = "https://api.github.com/repos/RRichardsDev/zeDB/releases/latest";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Updates must be signed by this Apple team; anything else is rejected even
/// if its signature is otherwise valid.
const TEAM_ID: &str = "M8Y82YQ4GF";

#[derive(Clone, Debug)]
pub struct UpdateInfo {
    pub version: String,
    pub url: String,
    pub asset: Option<AssetInfo>,
}

#[derive(Clone, Debug)]
pub struct AssetInfo {
    pub name: String,
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
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
}

#[derive(Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

pub async fn check() -> Option<UpdateInfo> {
    let url = std::env::var("ZEDB_UPDATE_URL").unwrap_or_else(|_| RELEASES_API.to_string());
    check_at(&url).await
}

async fn check_at(url: &str) -> Option<UpdateInfo> {
    let mut request = http_client()?
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
    (parse(&version)? > parse(CURRENT_VERSION)?).then(|| UpdateInfo {
        version,
        url: release.html_url,
        asset: release
            .assets
            .into_iter()
            .find(|asset| asset.name.ends_with(".zip"))
            .map(|asset| AssetInfo {
                name: asset.name,
                url: asset.browser_download_url,
            }),
    })
}

/// The .app bundle this process runs from, or `None` for bare-binary runs
/// (`cargo run`), where self-update does not apply.
pub fn current_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let macos_dir = exe.parent()?;
    let contents = macos_dir.parent()?;
    let bundle = contents.parent()?;
    (macos_dir.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app")
        .then(|| bundle.to_path_buf())
}

/// Download the release archive and install it over the current bundle.
/// On success the new version is on disk and takes effect at next launch.
pub async fn download_and_install(update: &UpdateInfo) -> Result<(), String> {
    let bundle = current_bundle().ok_or("not running from an app bundle")?;
    let asset = update
        .asset
        .clone()
        .ok_or("release has no downloadable archive")?;

    let mut request = http_client()
        .ok_or("could not build HTTP client")?
        .get(&asset.url)
        .header("Accept", "application/octet-stream");
    if let Ok(token) = std::env::var("ZEDB_GITHUB_TOKEN") {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .await
        .and_then(|response| response.error_for_status())
        .map_err(|error| format!("download failed: {error}"))?;
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("download failed: {error}"))?;

    let work = std::env::temp_dir().join(format!(
        "zedb-update-{}-{}",
        update.version,
        std::process::id()
    ));
    std::fs::create_dir_all(&work).map_err(|error| error.to_string())?;
    let archive = work.join(&asset.name);
    std::fs::write(&archive, &bytes).map_err(|error| error.to_string())?;

    let result = tokio::task::spawn_blocking(move || {
        let result = install_from_archive(&archive, &bundle);
        let _ = std::fs::remove_dir_all(&work);
        result
    })
    .await
    .map_err(|error| format!("install task failed: {error}"))?;
    result
}

/// Extract `archive` next to it, verify the contained bundle's signature and
/// team, and swap it in over `bundle`. Blocking; run off the UI thread.
fn install_from_archive(archive: &Path, bundle: &Path) -> Result<(), String> {
    let extract_dir = archive
        .parent()
        .ok_or("archive has no parent directory")?
        .join("extracted");
    run(
        "ditto",
        &[
            "-xk".as_ref(),
            archive.as_os_str(),
            extract_dir.as_os_str(),
        ],
    )?;

    let new_bundle = std::fs::read_dir(&extract_dir)
        .map_err(|error| error.to_string())?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .find(|path| path.extension().is_some_and(|extension| extension == "app"))
        .ok_or("archive does not contain an .app bundle")?;

    run(
        "codesign",
        &[
            "--verify".as_ref(),
            "--strict".as_ref(),
            new_bundle.as_os_str(),
        ],
    )?;
    let signing_info = run("codesign", &["-dvv".as_ref(), new_bundle.as_os_str()])?;
    if !signing_info.contains(&format!("TeamIdentifier={TEAM_ID}")) {
        return Err(format!("update is not signed by team {TEAM_ID}; refusing"));
    }

    let backup = bundle.with_extension("app.previous");
    let _ = std::fs::remove_dir_all(&backup);
    run("mv", &[bundle.as_os_str(), backup.as_os_str()])?;
    if let Err(error) = run("mv", &[new_bundle.as_os_str(), bundle.as_os_str()]) {
        let _ = run("mv", &[backup.as_os_str(), bundle.as_os_str()]);
        return Err(error);
    }
    let _ = std::fs::remove_dir_all(&backup);
    Ok(())
}

fn run(program: &str, args: &[&std::ffi::OsStr]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("{program}: {error}"))?;
    // codesign writes its diagnostics to stderr even on success.
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if output.status.success() {
        Ok(combined)
    } else {
        Err(format!("{program} failed: {}", combined.trim()))
    }
}

fn http_client() -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(concat!("zeDB/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()
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
    use super::{check_at, install_from_archive, parse};

    fn serve_release(tag: &str) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let body = format!(
            r#"{{"tag_name":"{tag}","html_url":"https://example.test/{tag}","draft":false,"prerelease":false,"assets":[{{"name":"zeDB-{tag}-macos.dmg","browser_download_url":"https://example.test/{tag}.dmg"}},{{"name":"zeDB-{tag}-macos.zip","browser_download_url":"https://example.test/{tag}.zip"}}]}}"#
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

    #[tokio::test]
    async fn newer_release_is_reported_with_zip_asset() {
        let url = serve_release("v99.0.0");
        let update = check_at(&url).await.expect("update expected");
        assert_eq!(update.version, "99.0.0");
        assert_eq!(update.url, "https://example.test/v99.0.0");
        let asset = update.asset.expect("zip asset expected");
        assert_eq!(asset.name, "zeDB-v99.0.0-macos.zip");
        assert_eq!(asset.url, "https://example.test/v99.0.0.zip");
    }

    #[tokio::test]
    async fn current_release_is_not_reported() {
        let url = serve_release(concat!("v", env!("CARGO_PKG_VERSION")));
        assert!(check_at(&url).await.is_none());
    }

    /// End-to-end extract/verify/swap against a genuinely signed bundle.
    /// Skips unless scripts/bundle-macos.sh has produced a team-signed
    /// target/macos/zeDB.app on this machine (local-only; CI does not run
    /// zedb-app tests).
    #[test]
    fn install_swaps_a_team_signed_bundle() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let source = repo_root.join("target/macos/zeDB.app");
        let signed = source.exists()
            && std::process::Command::new("codesign")
                .args(["-dvv", source.to_str().unwrap()])
                .output()
                .is_ok_and(|output| {
                    String::from_utf8_lossy(&output.stderr)
                        .contains(concat!("TeamIdentifier=", "M8Y82YQ4GF"))
                });
        if !signed {
            eprintln!("skipping: no team-signed bundle at target/macos/zeDB.app");
            return;
        }

        let work = std::env::temp_dir().join(format!("zedb-install-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).unwrap();

        // The "installed" copy that will be replaced.
        let installed = work.join("zeDB.app");
        assert!(std::process::Command::new("ditto")
            .args([source.to_str().unwrap(), installed.to_str().unwrap()])
            .status()
            .unwrap()
            .success());
        // The "downloaded" archive containing the new version.
        let archive = work.join("update.zip");
        assert!(std::process::Command::new("ditto")
            .args([
                "-c",
                "-k",
                "--keepParent",
                source.to_str().unwrap(),
                archive.to_str().unwrap(),
            ])
            .status()
            .unwrap()
            .success());

        install_from_archive(&archive, &installed).expect("install should succeed");
        assert!(installed.join("Contents/MacOS/zedb").exists());
        assert!(!installed.with_extension("app.previous").exists());

        let _ = std::fs::remove_dir_all(&work);
    }
}
