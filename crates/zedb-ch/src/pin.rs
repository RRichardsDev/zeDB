//! Pinned `clickhouse` binary management (docs/PHASE-1.md M2).
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

/// Return the cached binary for `version`, downloading it first if needed.
pub async fn ensure_binary(version: &str) -> Result<PathBuf, PinError> {
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
        if download(&url, &staging, version, os == "linux").await? {
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
) -> Result<bool, PinError> {
    let response = reqwest::get(url)
        .await
        .map_err(|error| PinError::Http(error.to_string()))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(false);
    }
    if !response.status().is_success() {
        return Err(PinError::Http(format!("{url}: HTTP {}", response.status())));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|error| PinError::Http(error.to_string()))?;

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
