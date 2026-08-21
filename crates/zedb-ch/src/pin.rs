//! Pinned `clickhouse` binary management.
//!
//! Replay and checks must run the exact version the target servers run,
//! so binaries are cached per version and downloaded on demand from the
//! official GitHub release assets (the same source the ancestor tooling
//! used). Nothing is preinstalled or hardcoded.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use futures_util::StreamExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::process::output_with_timeout;
use crate::{ChClient, ChConfig};

const MAX_RELEASE_METADATA_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ASSET_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_EXPANDED_ARCHIVE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 100_000;
const BINARY_PROCESS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
#[cfg(not(test))]
const DOWNLOAD_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
#[cfg(test)]
const DOWNLOAD_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(250);

#[derive(Debug, Deserialize)]
struct TrustManifest {
    artifacts: Vec<TrustedArtifact>,
}

#[derive(Debug, Deserialize)]
struct TrustedArtifact {
    version: String,
    channel: String,
    asset: String,
    size: u64,
    sha256: String,
}

static TRUST_MANIFEST: LazyLock<TrustManifest> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../trusted-clickhouse-artifacts.json"))
        .expect("checked-in ClickHouse trust manifest must be valid")
});

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
    #[error("invalid ClickHouse version {0:?}; expected four decimal components")]
    InvalidVersion(String),
    #[error("release metadata for {asset:?} is missing a valid SHA-256 digest")]
    MissingDigest { asset: String },
    #[error("downloaded {asset:?} exceeds the {limit} byte safety limit")]
    DownloadTooLarge { asset: String, limit: u64 },
    #[error("downloaded {asset:?} failed integrity verification")]
    IntegrityMismatch { asset: String },
    #[error("ClickHouse {version} asset {asset:?} is not in zeDB's reviewed trust manifest")]
    UntrustedArtifact { version: String, asset: String },
    #[error("unsafe ClickHouse archive: {0}")]
    UnsafeArchive(String),
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

fn digest_path(binary: &Path) -> PathBuf {
    binary.with_extension("sha256")
}

fn artifact_path(binary: &Path) -> PathBuf {
    binary.with_extension("artifact")
}

/// The cached binary for `version`, verified against its release digest before
/// executing it to check the reported version.
pub fn cached_binary(version: &str) -> Option<PathBuf> {
    if !valid_version(version) {
        return None;
    }
    let path = binary_path(version);
    verify_cached_binary(&path, version).then_some(path)
}

fn verify_cached_binary(path: &Path, version: &str) -> bool {
    if std::fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.file_type().is_symlink() || !metadata.is_file())
        || path.parent().is_some_and(|parent| {
            std::fs::symlink_metadata(parent)
                .is_ok_and(|metadata| metadata.file_type().is_symlink() || !metadata.is_dir())
        })
    {
        return false;
    }
    let Some(asset_name) = platform_asset_name(version) else {
        return false;
    };
    let Some(trusted) = trusted_artifact(version, None, &asset_name) else {
        return false;
    };
    let integrity_matches = if asset_name.ends_with(".tgz") {
        let Some(expected) = parse_sha256(&trusted.sha256) else {
            return false;
        };
        let archive = artifact_path(path);
        let expected_entry =
            PathBuf::from(format!("clickhouse-common-static-{version}")).join("usr/bin/clickhouse");
        sha256_file(&archive).is_ok_and(|actual| actual == expected)
            && binary_digest_from_archive(&archive, &expected_entry)
                .and_then(|archived| sha256_file(path).map(|cached| archived == cached))
                .unwrap_or(false)
    } else {
        // macOS rewrites adhoc linker-signed binaries in place on
        // first execution (signature replacement plus provenance
        // tracking), so the manifest digest matches the verified
        // download stream but never the file at rest. The continuity
        // digest ensure_exact_binary records after that first run is
        // the at-rest truth: it preserves corruption and substitution
        // detection (still checked before anything executes) without
        // invalidating the cache on every probe.
        recorded_digest(path)
            .is_some_and(|recorded| sha256_file(path).is_ok_and(|actual| actual == recorded))
    };
    integrity_matches && binary_reports_version(path, version)
}

/// The continuity digest recorded beside the binary after its first
/// execution; None when absent or malformed.
fn recorded_digest(binary: &Path) -> Option<[u8; 32]> {
    let text = std::fs::read_to_string(digest_path(binary)).ok()?;
    parse_sha256(text.trim())
}

fn valid_version(version: &str) -> bool {
    let mut parts = version.split('.');
    (0..4).all(|_| {
        parts
            .next()
            .is_some_and(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
    }) && parts.next().is_none()
}

fn platform_asset_name(version: &str) -> Option<String> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("clickhouse-macos-aarch64".into()),
        ("macos", "x86_64") => Some("clickhouse-macos".into()),
        ("linux", "x86_64") => Some(format!("clickhouse-common-static-{version}-amd64.tgz")),
        ("linux", "aarch64") => Some(format!("clickhouse-common-static-{version}-arm64.tgz")),
        _ => None,
    }
}

fn trusted_artifact(
    version: &str,
    channel: Option<&str>,
    asset: &str,
) -> Option<&'static TrustedArtifact> {
    TRUST_MANIFEST.artifacts.iter().find(|entry| {
        entry.version == version
            && entry.asset == asset
            && channel.is_none_or(|channel| entry.channel == channel)
    })
}

/// The newest manifest version that ships an asset for this platform,
/// so test-support's opt-in download never chases a hardcoded pin.
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn newest_trusted_version() -> Option<String> {
    fn key(version: &str) -> Vec<u64> {
        version
            .split('.')
            .map(|part| part.parse().unwrap_or(0))
            .collect()
    }
    TRUST_MANIFEST
        .artifacts
        .iter()
        .filter(|entry| {
            platform_asset_name(&entry.version).is_some_and(|asset| asset == entry.asset)
        })
        .map(|entry| entry.version.clone())
        .max_by_key(|version| key(version))
}

fn parse_sha256(value: &str) -> Option<[u8; 32]> {
    let value = value.strip_prefix("sha256:").unwrap_or(value);
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut digest = [0; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(digest)
}

fn format_sha256(digest: &[u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn sha256_file(path: &Path) -> std::io::Result<[u8; 32]> {
    use std::io::Read as _;

    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().into())
}

fn binary_reports_version(path: &Path, version: &str) -> bool {
    binary_version(path).is_some_and(|actual| actual.contains(version))
}

fn binary_version(path: &Path) -> Option<String> {
    if !path.is_file() {
        return None;
    }
    let mut command = Command::new(path);
    command.args(["local", "--version"]);
    output_with_timeout(command, None, BINARY_PROCESS_TIMEOUT)
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
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

/// Where getting a binary currently stands. Called from the pin
/// task; keep the callback cheap.
#[derive(Clone, Copy, Debug)]
pub enum PinPhase {
    Downloading {
        received: u64,
        total: Option<u64>,
    },
    /// First execution of a fresh binary: macOS assesses it before
    /// letting it run, which can take a while. There is no API to
    /// observe that assessment; this phase brackets the exec that
    /// triggers it, which amounts to the same window.
    Verifying,
}

pub type DownloadProgress = std::sync::Arc<dyn Fn(PinPhase) + Send + Sync>;

/// `ensure_binary` with download progress reported to `progress`;
/// nothing is reported when the binary is already cached.
pub async fn ensure_binary_with_progress(
    version: &str,
    progress: Option<DownloadProgress>,
) -> Result<PathBuf, PinError> {
    let exact = ensure_exact_binary(version, progress.clone()).await;
    match &exact {
        Err(PinError::DownloadFailed { .. } | PinError::UntrustedArtifact { .. }) => {}
        _ => return exact,
    }
    let alias_path = binary_cache_dir().join(version).join("fallback-version");
    let remembered = std::fs::read_to_string(&alias_path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| {
            platform_asset_name(value)
                .and_then(|asset| trusted_artifact(value, None, &asset))
                .is_some()
        });
    let fallback = match remembered {
        Some(fallback) => Some(fallback),
        None => nearest_published_release(version).await?,
    };
    let Some(fallback) = fallback else {
        return exact;
    };
    let path = ensure_exact_binary(&fallback, progress).await?;
    if let Ok(parent) = ensure_cache_version_dir(version) {
        let temporary = tempfile::Builder::new()
            .prefix(".zedb-clickhouse-alias-")
            .tempfile_in(parent);
        if let Ok(mut temporary) = temporary {
            use std::io::Write as _;
            if writeln!(temporary, "{fallback}").is_ok() {
                let _ = std::fs::remove_file(&alias_path);
                let _ = temporary.persist(&alias_path);
            }
        }
    }
    Ok(path)
}

fn ensure_cache_version_dir(version: &str) -> Result<PathBuf, PinError> {
    let root = binary_cache_dir();
    std::fs::create_dir_all(&root)?;
    let root_metadata = std::fs::symlink_metadata(&root)?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(PinError::UnsafeArchive(
            "ClickHouse cache root is not a real directory".into(),
        ));
    }
    let directory = root.join(version);
    std::fs::create_dir_all(&directory)?;
    let metadata = std::fs::symlink_metadata(&directory)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(PinError::UnsafeArchive(
            "ClickHouse version cache is not a real directory".into(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(directory)
}

/// The closest published release to `version`, from the GitHub
/// releases listing; None when nothing is published at all.
async fn nearest_published_release(version: &str) -> Result<Option<String>, PinError> {
    let tags: Vec<String> = TRUST_MANIFEST
        .artifacts
        .iter()
        .map(|artifact| format!("v{}-{}", artifact.version, artifact.channel))
        .collect();
    Ok(closest_release_tag(
        version,
        tags.iter().map(String::as_str),
    ))
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
}

#[derive(Debug)]
struct ReleaseAsset {
    name: String,
    url: String,
    size: u64,
    sha256: [u8; 32],
}

fn validate_trusted_metadata(
    trusted: &TrustedArtifact,
    asset: &GithubAsset,
) -> Result<[u8; 32], PinError> {
    let reported = asset
        .digest
        .as_deref()
        .and_then(parse_sha256)
        .ok_or_else(|| PinError::MissingDigest {
            asset: asset.name.clone(),
        })?;
    let expected = parse_sha256(&trusted.sha256).expect("trusted manifest digest is valid");
    if asset.size != trusted.size || reported != expected {
        return Err(PinError::IntegrityMismatch {
            asset: asset.name.clone(),
        });
    }
    Ok(expected)
}

async fn release_asset(
    version: &str,
    channel: &str,
    asset_name: &str,
) -> Result<Option<ReleaseAsset>, PinError> {
    if !valid_version(version) {
        return Err(PinError::InvalidVersion(version.into()));
    }
    let Some(trusted) = trusted_artifact(version, Some(channel), asset_name) else {
        return Ok(None);
    };
    let tag = format!("v{version}-{channel}");
    let api_url = format!("https://api.github.com/repos/ClickHouse/ClickHouse/releases/tags/{tag}");
    let client = reqwest::Client::builder()
        .user_agent(concat!("zeDB/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|error| PinError::Http(error.to_string()))?;
    let mut response = client
        .get(&api_url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|error| PinError::Http(error.to_string()))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(PinError::Http(format!(
            "{api_url}: HTTP {}",
            response.status()
        )));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RELEASE_METADATA_BYTES)
    {
        return Err(PinError::DownloadTooLarge {
            asset: format!("release metadata for {tag}"),
            limit: MAX_RELEASE_METADATA_BYTES,
        });
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| PinError::Http(error.to_string()))?
    {
        if body.len() as u64 + chunk.len() as u64 > MAX_RELEASE_METADATA_BYTES {
            return Err(PinError::DownloadTooLarge {
                asset: format!("release metadata for {tag}"),
                limit: MAX_RELEASE_METADATA_BYTES,
            });
        }
        body.extend_from_slice(&chunk);
    }
    let release: GithubRelease =
        serde_json::from_slice(&body).map_err(|error| PinError::Http(error.to_string()))?;
    let Some(asset) = release
        .assets
        .into_iter()
        .find(|asset| asset.name == asset_name)
    else {
        return Ok(None);
    };
    let expected_url =
        format!("https://github.com/ClickHouse/ClickHouse/releases/download/{tag}/{asset_name}");
    if asset.browser_download_url != expected_url {
        return Err(PinError::Http(format!(
            "release metadata returned unexpected asset URL {:?}",
            asset.browser_download_url
        )));
    }
    if asset.size == 0 || asset.size > MAX_ASSET_BYTES {
        return Err(PinError::DownloadTooLarge {
            asset: asset.name,
            limit: MAX_ASSET_BYTES,
        });
    }
    let trusted_sha256 = validate_trusted_metadata(trusted, &asset)?;
    Ok(Some(ReleaseAsset {
        name: asset.name,
        url: asset.browser_download_url,
        size: asset.size,
        sha256: trusted_sha256,
    }))
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
    if !valid_version(version) {
        return Err(PinError::InvalidVersion(version.into()));
    }
    // Concurrent callers (the three chain checks, Verify-all) share
    // one staging path per version; serialize so a download is not
    // clobbered mid-write. Late arrivals find the cache warm.
    static DOWNLOAD_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = DOWNLOAD_LOCK.lock().await;
    let report = |phase: PinPhase| {
        if let Some(progress) = &progress {
            progress(phase);
        }
    };
    let target = binary_path(version);
    if target.is_file() {
        // Integrity is checked against the release digest before the cache
        // probe executes anything.
        report(PinPhase::Verifying);
        if let Some(path) = cached_binary(version) {
            return Ok(path);
        }
        std::fs::remove_file(&target)?;
        let _ = std::fs::remove_file(digest_path(&target));
        let _ = std::fs::remove_file(artifact_path(&target));
    }

    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let directory = ensure_cache_version_dir(version)?;
    let target = directory.join("clickhouse");
    let staging = target.with_extension("tmp");
    let _ = std::fs::remove_file(&staging);

    // Release tags carry a channel suffix the version alone does not
    // reveal; try both.
    let mut tried = Vec::new();
    let asset_name = match platform_asset_name(version) {
        Some(asset) => asset,
        None => {
            return Err(PinError::UnsupportedPlatform {
                os: os.into(),
                arch: arch.into(),
            })
        }
    };
    if trusted_artifact(version, None, &asset_name).is_none() {
        return Err(PinError::UntrustedArtifact {
            version: version.into(),
            asset: asset_name,
        });
    }
    let mut downloaded = None;
    for channel in ["lts", "stable"] {
        let release = format!(
            "https://github.com/ClickHouse/ClickHouse/releases/download/v{version}-{channel}"
        );
        let url = format!("{release}/{asset_name}");
        tried.push(url.clone());
        let Some(asset) = release_asset(version, channel, &asset_name).await? else {
            continue;
        };
        let archive = download(&asset, &staging, version, os == "linux", progress.as_ref()).await?;
        downloaded = Some((asset.sha256, archive));
        break;
    }
    let Some((_, verified_archive)) = downloaded else {
        return Err(PinError::DownloadFailed {
            version: version.into(),
            tried,
        });
    };

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&staging, &target)?;
    if let Some(archive) = verified_archive {
        let archive_target = artifact_path(&target);
        let _ = std::fs::remove_file(&archive_target);
        if let Err(error) = archive.persist(&archive_target) {
            let _ = std::fs::remove_file(&target);
            return Err(PinError::Io(error.error));
        }
    }
    // This is the first execution. Artifact integrity has already been checked.
    report(PinPhase::Verifying);
    let actual = binary_version(&target).unwrap_or_default();
    if !actual.contains(version) {
        let _ = std::fs::remove_file(&target);
        let _ = std::fs::remove_file(digest_path(&target));
        let _ = std::fs::remove_file(artifact_path(&target));
        return Err(PinError::VersionMismatch {
            expected: version.into(),
            actual,
        });
    }
    // Hash the file only after that run: macOS rewrites adhoc
    // linker-signed binaries in place on first execution, and this
    // at-rest digest is what future cache probes verify against (the
    // trust manifest already anchored the download stream).
    let at_rest = match sha256_file(&target) {
        Ok(digest) => digest,
        Err(error) => {
            let _ = std::fs::remove_file(&target);
            let _ = std::fs::remove_file(artifact_path(&target));
            return Err(error.into());
        }
    };
    if let Err(error) = persist_digest(&target, &at_rest) {
        let _ = std::fs::remove_file(&target);
        let _ = std::fs::remove_file(artifact_path(&target));
        return Err(error);
    }
    Ok(target)
}

/// Download an asset to `staging` after checking its trusted release-metadata
/// digest. Linux assets are tarballs holding the binary
/// at `clickhouse-common-static-<version>/usr/bin/clickhouse`.
async fn download(
    asset: &ReleaseAsset,
    staging: &Path,
    version: &str,
    is_tarball: bool,
    progress: Option<&DownloadProgress>,
) -> Result<Option<tempfile::NamedTempFile>, PinError> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("zeDB/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::limited(5))
        .timeout(std::time::Duration::from_secs(20 * 60))
        .build()
        .map_err(|error| PinError::Http(error.to_string()))?;
    let response = client
        .get(&asset.url)
        .send()
        .await
        .map_err(|error| PinError::Http(error.to_string()))?;
    if !response.status().is_success() {
        return Err(PinError::Http(format!(
            "{}: HTTP {}",
            asset.url,
            response.status()
        )));
    }
    if response.content_length() != Some(asset.size) {
        return Err(PinError::IntegrityMismatch {
            asset: asset.name.clone(),
        });
    }

    let parent = staging
        .parent()
        .ok_or_else(|| PinError::UnsafeArchive("binary staging path has no parent".into()))?;
    let archive_temp = tempfile::Builder::new()
        .prefix(".zedb-clickhouse-download-")
        .tempfile_in(parent)?;
    let mut output = tokio::fs::File::from_std(archive_temp.reopen()?);
    let mut hasher = Sha256::new();
    let mut received = 0u64;
    let mut stream = response.bytes_stream();
    loop {
        let chunk = tokio::time::timeout(DOWNLOAD_IDLE_TIMEOUT, stream.next())
            .await
            .map_err(|_| PinError::Http("download stalled while waiting for data".into()))?;
        let Some(chunk) = chunk else {
            break;
        };
        let chunk = chunk.map_err(|error| PinError::Http(error.to_string()))?;
        received =
            received
                .checked_add(chunk.len() as u64)
                .ok_or_else(|| PinError::DownloadTooLarge {
                    asset: asset.name.clone(),
                    limit: MAX_ASSET_BYTES,
                })?;
        if received > asset.size || received > MAX_ASSET_BYTES {
            return Err(PinError::DownloadTooLarge {
                asset: asset.name.clone(),
                limit: asset.size.min(MAX_ASSET_BYTES),
            });
        }
        hasher.update(&chunk);
        output.write_all(&chunk).await?;
        if let Some(progress) = progress {
            progress(PinPhase::Downloading {
                received,
                total: Some(asset.size),
            });
        }
    }
    output.flush().await?;
    drop(output);
    if received != asset.size || <[u8; 32]>::from(hasher.finalize()) != asset.sha256 {
        return Err(PinError::IntegrityMismatch {
            asset: asset.name.clone(),
        });
    }

    if is_tarball {
        let expected =
            PathBuf::from(format!("clickhouse-common-static-{version}")).join("usr/bin/clickhouse");
        extract_binary(archive_temp.path(), staging, &expected)?;
        Ok(Some(archive_temp))
    } else {
        archive_temp
            .persist(staging)
            .map_err(|error| PinError::Io(error.error))?;
        Ok(None)
    }
}

fn binary_digest_from_archive(archive_path: &Path, expected: &Path) -> std::io::Result<[u8; 32]> {
    use std::io::Read as _;

    let file = std::fs::File::open(archive_path)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let mut found = None;
    for (index, entry) in archive.entries()?.enumerate() {
        if index >= MAX_ARCHIVE_ENTRIES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "archive contains too many entries",
            ));
        }
        let mut entry = entry?;
        if entry.path()?.as_ref() != expected {
            continue;
        }
        if found.is_some() || !entry.header().entry_type().is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "archive binary entry is not unique and regular",
            ));
        }
        if entry.size() == 0 || entry.size() > MAX_ASSET_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "archive binary entry has an unsafe size",
            ));
        }
        let expected_size = entry.size();
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut read_total = 0u64;
        loop {
            let read = entry.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            read_total = read_total.checked_add(read as u64).ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "archive size overflow")
            })?;
            hasher.update(&buffer[..read]);
        }
        if read_total != expected_size {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "archive binary entry ended early",
            ));
        }
        found = Some(hasher.finalize().into());
    }
    found.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "archive does not contain the expected binary",
        )
    })
}

fn persist_digest(binary: &Path, digest: &[u8; 32]) -> Result<(), PinError> {
    use std::io::Write as _;

    let parent = binary
        .parent()
        .ok_or_else(|| PinError::UnsafeArchive("binary path has no parent".into()))?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".zedb-clickhouse-digest-")
        .tempfile_in(parent)?;
    writeln!(temporary, "sha256:{}", format_sha256(digest))?;
    temporary.as_file().sync_all()?;
    let target = digest_path(binary);
    let _ = std::fs::remove_file(&target);
    temporary
        .persist(target)
        .map_err(|error| PinError::Io(error.error))?;
    Ok(())
}

fn extract_binary(archive_path: &Path, staging: &Path, expected: &Path) -> Result<(), PinError> {
    use std::io::Write as _;
    use std::path::Component;

    let file = std::fs::File::open(archive_path)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let parent = staging
        .parent()
        .ok_or_else(|| PinError::UnsafeArchive("binary staging path has no parent".into()))?;
    let mut output = tempfile::Builder::new()
        .prefix(".zedb-clickhouse-extract-")
        .tempfile_in(parent)?;
    let mut found = false;
    let mut expanded = 0u64;
    for (index, entry) in archive.entries()?.enumerate() {
        if index >= MAX_ARCHIVE_ENTRIES {
            return Err(PinError::UnsafeArchive(format!(
                "archive contains more than {MAX_ARCHIVE_ENTRIES} entries"
            )));
        }
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        if path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(PinError::UnsafeArchive(format!(
                "unsafe archive entry path {path:?}"
            )));
        }
        expanded = expanded
            .checked_add(entry.size())
            .ok_or_else(|| PinError::UnsafeArchive("expanded size overflow".into()))?;
        if expanded > MAX_EXPANDED_ARCHIVE_BYTES {
            return Err(PinError::UnsafeArchive(format!(
                "expanded archive exceeds {MAX_EXPANDED_ARCHIVE_BYTES} bytes"
            )));
        }
        if path != expected {
            continue;
        }
        if found || !entry.header().entry_type().is_file() {
            return Err(PinError::UnsafeArchive(format!(
                "expected one regular file at {expected:?}"
            )));
        }
        if entry.size() == 0 || entry.size() > MAX_ASSET_BYTES {
            return Err(PinError::UnsafeArchive(format!(
                "binary entry has unsafe size {}",
                entry.size()
            )));
        }
        let copied = std::io::copy(&mut entry, output.as_file_mut())?;
        if copied != entry.size() {
            return Err(PinError::UnsafeArchive(
                "binary entry ended before its declared size".into(),
            ));
        }
        found = true;
    }
    if !found {
        return Err(PinError::UnsafeArchive(format!(
            "archive does not contain {expected:?}"
        )));
    }
    output.flush()?;
    output.as_file().sync_all()?;
    output
        .persist(staging)
        .map_err(|error| PinError::Io(error.error))?;
    Ok(())
}

/// Run a trivial query through `clickhouse local` to prove the binary works.
pub fn smoke_replay(binary: &Path) -> Result<(), PinError> {
    let mut command = Command::new(binary);
    command.args(["local", "--query", "SELECT 1"]);
    let output = output_with_timeout(command, None, BINARY_PROCESS_TIMEOUT)?;
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
    fn versions_and_digests_are_strict() {
        assert!(valid_version("26.3.12.3"));
        for invalid in ["", "26.3", "26.3.12.3.1", "26.3.x.3", "../26.3.12.3"] {
            assert!(!valid_version(invalid), "accepted {invalid:?}");
        }

        let digest = [0xabu8; 32];
        let formatted = format_sha256(&digest);
        assert_eq!(parse_sha256(&formatted), Some(digest));
        assert_eq!(parse_sha256(&format!("sha256:{formatted}")), Some(digest));
        assert_eq!(parse_sha256("ab"), None);
        assert_eq!(parse_sha256(&"g".repeat(64)), None);
    }

    #[test]
    fn trust_manifest_is_strict_and_metadata_cannot_replace_it() {
        assert_eq!(TRUST_MANIFEST.artifacts.len(), 4);
        for trusted in &TRUST_MANIFEST.artifacts {
            assert!(valid_version(&trusted.version));
            assert!(trusted.size > 0 && trusted.size <= MAX_ASSET_BYTES);
            assert!(parse_sha256(&trusted.sha256).is_some());
        }
        assert!(trusted_artifact("99.99.99.99", None, "clickhouse-macos-aarch64").is_none());

        let trusted =
            trusted_artifact("26.3.12.3", Some("lts"), "clickhouse-macos-aarch64").unwrap();
        let substituted = GithubAsset {
            name: trusted.asset.clone(),
            browser_download_url: "unused".into(),
            size: trusted.size,
            digest: Some(format!("sha256:{}", "00".repeat(32))),
        };
        assert!(matches!(
            validate_trusted_metadata(trusted, &substituted),
            Err(PinError::IntegrityMismatch { .. })
        ));
    }

    /// The at-rest file is governed by the continuity digest recorded
    /// after the first execution, because macOS rewrites adhoc
    /// linker-signed binaries in place on that run: no record rejects,
    /// a matching record accepts, and post-record tampering rejects
    /// before anything executes.
    #[cfg(unix)]
    #[test]
    fn continuity_digest_governs_the_cache_between_downloads() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("clickhouse");
        std::fs::write(
            &binary,
            "#!/bin/sh\necho 'ClickHouse local version 26.3.12.3.'\n",
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();

        // No recorded digest: rejected (and, per the sibling test,
        // rejected before execution).
        assert!(!verify_cached_binary(&binary, "26.3.12.3"));

        // As ensure_exact_binary records it after the first run.
        persist_digest(&binary, &sha256_file(&binary).unwrap()).unwrap();
        assert!(verify_cached_binary(&binary, "26.3.12.3"));

        // Tampering after the record is caught before execution.
        let marker = directory.path().join("executed");
        std::fs::write(
            &binary,
            format!(
                "#!/bin/sh\ntouch '{}'\necho 'ClickHouse local version 26.3.12.3.'\n",
                marker.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!verify_cached_binary(&binary, "26.3.12.3"));
        assert!(!marker.exists(), "tampered payload was executed");
    }

    #[cfg(unix)]
    #[test]
    fn substituted_cached_payload_is_rejected_before_execution() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("executed");
        let payload = directory.path().join("clickhouse");
        std::fs::write(
            &payload,
            format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        )
        .unwrap();
        std::fs::set_permissions(&payload, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(!verify_cached_binary(&payload, "26.3.12.3"));
        assert!(!marker.exists(), "untrusted payload was executed");
    }

    #[test]
    fn archive_extracts_only_the_expected_regular_file() {
        use std::io::Write as _;

        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("asset.tgz");
        let archive_file = std::fs::File::create(&archive_path).unwrap();
        let encoder = flate2::write::GzEncoder::new(archive_file, flate2::Compression::default());
        let mut archive = tar::Builder::new(encoder);

        let mut unrelated = tar::Header::new_gnu();
        unrelated.set_size(7);
        unrelated.set_mode(0o644);
        unrelated.set_cksum();
        archive
            .append_data(&mut unrelated, "safe/unrelated.txt", &b"ignored"[..])
            .unwrap();

        let expected =
            PathBuf::from("clickhouse-common-static-26.3.12.3").join("usr/bin/clickhouse");
        let payload = b"verified binary";
        let mut binary = tar::Header::new_gnu();
        binary.set_size(payload.len() as u64);
        binary.set_mode(0o755);
        binary.set_cksum();
        archive
            .append_data(&mut binary, &expected, &payload[..])
            .unwrap();
        archive
            .into_inner()
            .unwrap()
            .finish()
            .unwrap()
            .flush()
            .unwrap();

        let staging = directory.path().join("clickhouse.tmp");
        extract_binary(&archive_path, &staging, &expected).unwrap();
        assert_eq!(std::fs::read(staging).unwrap(), payload);
        assert_eq!(
            binary_digest_from_archive(&archive_path, &expected).unwrap(),
            <[u8; 32]>::from(Sha256::digest(payload))
        );
        assert!(!directory.path().join("safe/unrelated.txt").exists());
    }

    #[test]
    fn archive_rejects_a_link_at_the_binary_path() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("asset.tgz");
        let archive_file = std::fs::File::create(&archive_path).unwrap();
        let encoder = flate2::write::GzEncoder::new(archive_file, flate2::Compression::default());
        let mut archive = tar::Builder::new(encoder);
        let expected =
            PathBuf::from("clickhouse-common-static-26.3.12.3").join("usr/bin/clickhouse");
        let mut link = tar::Header::new_gnu();
        link.set_entry_type(tar::EntryType::Symlink);
        link.set_size(0);
        link.set_mode(0o755);
        link.set_link_name("/tmp/not-a-binary").unwrap();
        link.set_cksum();
        archive
            .append_data(&mut link, &expected, std::io::empty())
            .unwrap();
        archive.into_inner().unwrap().finish().unwrap();

        let result = extract_binary(
            &archive_path,
            &directory.path().join("clickhouse.tmp"),
            &expected,
        );
        assert!(matches!(result, Err(PinError::UnsafeArchive(_))));
    }

    #[tokio::test]
    #[ignore = "requires the official GitHub release API"]
    async fn official_release_metadata_has_a_digest_and_expected_url() {
        let asset = release_asset("26.3.12.3", "lts", "clickhouse-macos-aarch64")
            .await
            .unwrap()
            .expect("published demo version");
        assert_eq!(asset.name, "clickhouse-macos-aarch64");
        assert_eq!(
            asset.url,
            "https://github.com/ClickHouse/ClickHouse/releases/download/v26.3.12.3-lts/clickhouse-macos-aarch64"
        );
        assert!(asset.size > 100 * 1024 * 1024);
        assert_ne!(asset.sha256, [0; 32]);
    }

    #[tokio::test]
    async fn download_rejects_a_stalled_response_body() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request).await;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\n")
                .await
                .unwrap();
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        });
        let directory = tempfile::tempdir().unwrap();
        let asset = ReleaseAsset {
            name: "stalled-test-asset".into(),
            url: format!("http://{address}/asset"),
            size: 1,
            sha256: [0; 32],
        };

        let started = std::time::Instant::now();
        let result = download(
            &asset,
            &directory.path().join("clickhouse.tmp"),
            "26.3.12.3",
            false,
            None,
        )
        .await;
        assert!(matches!(result, Err(PinError::Http(message)) if message.contains("stalled")));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        server.abort();
    }

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

    #[tokio::test]
    async fn fallback_selection_is_limited_to_the_trust_manifest() {
        assert_eq!(
            nearest_published_release("26.2.1.558").await.unwrap(),
            Some("26.3.12.3".into())
        );
    }
}
