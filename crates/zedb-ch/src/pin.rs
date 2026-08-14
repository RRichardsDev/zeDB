//! Pinned `clickhouse` binary management.
//!
//! Replay and checks must run the exact version the target servers run,
//! so binaries are cached per version and downloaded on demand from the
//! official GitHub release assets (the same source the ancestor tooling
//! used). Nothing is preinstalled or hardcoded.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{ChClient, ChConfig};

#[derive(Debug, thiserror::Error)]
pub enum PinError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error(
        "no ClickHouse build for this platform ({os}-{arch}). \
         macOS and Linux (x86_64/aarch64) are supported; on Windows use WSL2. \
         Without a local binary, replay-backed checks cannot run here."
    )]
    UnsupportedPlatform { os: String, arch: String },
    #[error(
        "could not download ClickHouse {version}: no release asset found \
         (tried {tried:?}). Check the version exists at \
         https://github.com/ClickHouse/ClickHouse/releases"
    )]
    DownloadFailed { version: String, tried: Vec<String> },
    #[error("downloaded binary reports {actual:?}, expected version {expected}")]
    VersionMismatch { expected: String, actual: String },
    #[error("could not discover server version: {0}")]
    Discovery(String),
    #[error("{0}")]
    Http(String),
}

/// Where per-version binaries live: `<cache>/zedb/clickhouse/<version>/clickhouse`.
pub fn binary_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("zedb")
        .join("clickhouse")
}

pub fn binary_path(version: &str) -> PathBuf {
    binary_cache_dir().join(version).join("clickhouse")
}

/// The cached binary for `version`, verified to actually be that version.
pub fn cached_binary(version: &str) -> Option<PathBuf> {
    let path = binary_path(version);
    binary_reports_version(&path, version).then_some(path)
}

fn binary_reports_version(path: &Path, version: &str) -> bool {
    if !path.is_file() {
        return false;
    }
    Command::new(path)
        .args(["local", "--version"])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).contains(version))
        .unwrap_or(false)
}

/// Ask a server which version it runs.
pub async fn discover_server_version(config: ChConfig) -> Result<String, PinError> {
    let client = ChClient::new(config);
    let result = client
        .query("SELECT version()")
        .await
        .map_err(|error| PinError::Discovery(error.to_string()))?;
    result
        .rows
        .first()
        .and_then(|row| row.first())
        .map(|value| value.to_string())
        .ok_or_else(|| PinError::Discovery("SELECT version() returned no rows".into()))
}

/// Return the cached binary for `version`, downloading it first if
/// needed. Cloud servers pin builds (e.g. 26.2.1.558) that have no
/// OSS release asset; when the exact version is not published, the
/// nearest published release of the same major.minor stands in, and
/// the substitution is remembered beside the cache so the release
/// listing is not re-fetched every check.
pub async fn ensure_binary(version: &str) -> Result<PathBuf, PinError> {
    ensure_binary_with_progress(version, None).await
}

/// How far a binary download has come: (bytes received, total when
/// the server said). Called from the download task; keep it cheap.
pub type DownloadProgress = std::sync::Arc<dyn Fn(u64, Option<u64>) + Send + Sync>;

/// `ensure_binary` with download progress reported to `progress`;
/// nothing is reported when the binary is already cached.
pub async fn ensure_binary_with_progress(
    version: &str,
    progress: Option<DownloadProgress>,
) -> Result<PathBuf, PinError> {
    let exact = ensure_exact_binary(version, progress.clone()).await;
    let Err(PinError::DownloadFailed { .. }) = &exact else {
        return exact;
    };
    let alias_path = binary_cache_dir().join(version).join("fallback-version");
    let remembered = std::fs::read_to_string(&alias_path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let fallback = match remembered {
        Some(fallback) => Some(fallback),
        None => nearest_published_release(version).await?,
    };
    let Some(fallback) = fallback else {
        return exact;
    };
    let path = ensure_exact_binary(&fallback, progress).await?;
    if let Some(parent) = alias_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&alias_path, &fallback);
    Ok(path)
}

/// The closest published release to `version`, from the GitHub
/// releases listing; None when nothing is published at all.
async fn nearest_published_release(version: &str) -> Result<Option<String>, PinError> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("zeDB/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| PinError::Http(error.to_string()))?;
    let body = client
        .get("https://api.github.com/repos/ClickHouse/ClickHouse/releases?per_page=100")
        .send()
        .await
        .map_err(|error| PinError::Http(error.to_string()))?
        .error_for_status()
        .map_err(|error| PinError::Http(error.to_string()))?
        .text()
        .await
        .map_err(|error| PinError::Http(error.to_string()))?;
    let releases: serde_json::Value =
        serde_json::from_str(&body).map_err(|error| PinError::Http(error.to_string()))?;
    let tags = releases
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|release| release.get("tag_name").and_then(|tag| tag.as_str()));
    Ok(closest_release_tag(version, tags))
}

fn version_key(version: &str) -> Vec<u64> {
    version
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

/// The closest published version to `version` among tags like
/// `v26.2.1.9999-stable`: newest of the same major.minor first, else
/// the nearest newer release (a newer binary still speaks the older
/// dialect; an older one may not), else the newest older one.
fn closest_release_tag<'a>(version: &str, tags: impl Iterator<Item = &'a str>) -> Option<String> {
    let target = version_key(version);
    let same_minor: Vec<u64> = target.iter().copied().take(2).collect();
    let mut best_same: Option<(Vec<u64>, String)> = None;
    let mut best_newer: Option<(Vec<u64>, String)> = None;
    let mut best_older: Option<(Vec<u64>, String)> = None;
    for tag in tags {
        let Some(candidate) = tag
            .strip_prefix('v')
            .and_then(|tag| tag.rsplit_once('-'))
            .map(|(version, _)| version)
        else {
            continue;
        };
        let key = version_key(candidate);
        let entry = (key.clone(), candidate.to_string());
        if key.len() >= 2 && key[..2] == same_minor[..] {
            if best_same.as_ref().map(|(k, _)| key > *k).unwrap_or(true) {
                best_same = Some(entry);
            }
        } else if key > target {
            if best_newer.as_ref().map(|(k, _)| key < *k).unwrap_or(true) {
                best_newer = Some(entry);
            }
        } else if best_older.as_ref().map(|(k, _)| key > *k).unwrap_or(true) {
            best_older = Some(entry);
        }
    }
    best_same
        .or(best_newer)
        .or(best_older)
        .map(|(_, candidate)| candidate)
}

/// Return the cached binary for exactly `version`, downloading it
/// first if needed.
async fn ensure_exact_binary(
    version: &str,
    progress: Option<DownloadProgress>,
) -> Result<PathBuf, PinError> {
    if let Some(path) = cached_binary(version) {
        return Ok(path);
    }

    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let target = binary_path(version);
    std::fs::create_dir_all(target.parent().expect("binary path has a parent"))?;
    let staging = target.with_extension("tmp");

    // Release tags carry a channel suffix the version alone does not
    // reveal; try both.
    let mut tried = Vec::new();
    let mut downloaded = false;
    for channel in ["lts", "stable"] {
        let release = format!(
            "https://github.com/ClickHouse/ClickHouse/releases/download/v{version}-{channel}"
        );
        let url = match (os, arch) {
            ("macos", "aarch64") => format!("{release}/clickhouse-macos-aarch64"),
            ("macos", "x86_64") => format!("{release}/clickhouse-macos"),
            ("linux", "x86_64") => {
                format!("{release}/clickhouse-common-static-{version}-amd64.tgz")
            }
            ("linux", "aarch64") => {
                format!("{release}/clickhouse-common-static-{version}-arm64.tgz")
            }
            _ => {
                return Err(PinError::UnsupportedPlatform {
                    os: os.into(),
                    arch: arch.into(),
                })
            }
        };
        tried.push(url.clone());
        if download(&url, &staging, version, os == "linux", progress.as_ref()).await? {
            downloaded = true;
            break;
        }
    }
    if !downloaded {
        return Err(PinError::DownloadFailed {
            version: version.into(),
            tried,
        });
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&staging, &target)?;

    if !binary_reports_version(&target, version) {
        let actual = Command::new(&target)
            .args(["local", "--version"])
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .unwrap_or_default();
        return Err(PinError::VersionMismatch {
            expected: version.into(),
            actual,
        });
    }
    Ok(target)
}

/// Download `url` to `staging`; returns false on 404 so the caller can try
/// the next release channel. Linux assets are tarballs holding the binary
/// at `clickhouse-common-static-<version>/usr/bin/clickhouse`.
async fn download(
    url: &str,
    staging: &Path,
    version: &str,
    is_tarball: bool,
    progress: Option<&DownloadProgress>,
) -> Result<bool, PinError> {
    let mut response = reqwest::get(url)
        .await
        .map_err(|error| PinError::Http(error.to_string()))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(false);
    }
    if !response.status().is_success() {
        return Err(PinError::Http(format!("{url}: HTTP {}", response.status())));
    }

    // Stream so callers can show real progress on a ~500MB asset.
    let total = response.content_length();
    let mut bytes: Vec<u8> = Vec::with_capacity(total.unwrap_or(0) as usize);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| PinError::Http(error.to_string()))?
    {
        bytes.extend_from_slice(&chunk);
        if let Some(progress) = progress {
            progress(bytes.len() as u64, total);
        }
    }

    if is_tarball {
        let unpack_dir = staging.with_extension("unpack");
        let tarball = staging.with_extension("tgz");
        std::fs::write(&tarball, &bytes)?;
        std::fs::create_dir_all(&unpack_dir)?;
        let status = Command::new("tar")
            .arg("-xzf")
            .arg(&tarball)
            .arg("-C")
            .arg(&unpack_dir)
            .status()?;
        if !status.success() {
            return Err(PinError::Http(format!("tar failed unpacking {url}")));
        }
        let inner = unpack_dir
            .join(format!("clickhouse-common-static-{version}"))
            .join("usr/bin/clickhouse");
        std::fs::rename(inner, staging)?;
        std::fs::remove_file(tarball).ok();
        std::fs::remove_dir_all(unpack_dir).ok();
    } else {
        std::fs::write(staging, &bytes)?;
    }
    Ok(true)
}

/// Run a trivial query through `clickhouse local` to prove the binary works.
pub fn smoke_replay(binary: &Path) -> Result<(), PinError> {
    let output = Command::new(binary)
        .args(["local", "--query", "SELECT 1"])
        .output()?;
    if output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "1" {
        Ok(())
    } else {
        Err(PinError::Http(format!(
            "smoke replay failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_closest_published_release() {
        let tags = [
            "v26.3.1.100-stable",
            "v26.2.1.400-stable",
            "v26.2.1.999-lts",
            "v26.2.1.998-stable",
            "v25.8.9.1-lts",
            "not-a-version",
        ];
        // Same major.minor available: its newest wins.
        assert_eq!(
            closest_release_tag("26.2.1.558", tags.iter().copied()),
            Some("26.2.1.999".to_string())
        );
        // No 26.1 published: the nearest newer release stands in.
        assert_eq!(
            closest_release_tag("26.1.1.1", tags.iter().copied()),
            Some("26.2.1.400".to_string())
        );
        // Nothing newer than 27.x: the newest older one, last resort.
        assert_eq!(
            closest_release_tag("27.1.1.1", tags.iter().copied()),
            Some("26.3.1.100".to_string())
        );
        assert_eq!(closest_release_tag("26.4.1.1", [].iter().copied()), None);
    }
}
