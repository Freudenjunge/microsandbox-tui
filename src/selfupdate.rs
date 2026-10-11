//! Self-update: detect a newer **stable** release of msb-tui and replace the
//! running binary.
//!
//! The check hits GitHub's `/releases/latest` endpoint, which only reports
//! full (non-draft, non-prerelease) releases — beta tags like `0.2.0-beta.1`
//! are marked prerelease by the release workflow and never trigger a prompt.
//! When the reported version is newer than the running one
//! ([`current_version`], i.e. `CARGO_PKG_VERSION`), a [`UpdateInfo`] is
//! handed to the UI which offers one-key update; accepting it downloads the
//! per-platform tarball, verifies it against the sibling `.sha256` asset,
//! and swaps the binary on disk (backup kept next to it). The running
//! process keeps the old inode — the new version applies on relaunch.
//!
//! Like [`crate::runtime`], the pure pieces (tag parsing, asset picking,
//! sha256 verification, extraction, install) are unit-tested; only the
//! network calls are untested.

use std::io::Read;
use std::time::Duration;

use anyhow::{Context, bail};
use semver::Version as Semver;
use serde::Deserialize;

/// GitHub API endpoint reporting the latest **stable** release (prereleases
/// and drafts are excluded by the endpoint itself).
const RELEASE_API: &str =
    "https://api.github.com/repos/Freudenjunge/microsandbox-tui/releases/latest";

/// The release target triple this binary was built for — used to pick the
/// matching release asset (`msb-tui-v{ver}-{triple}.tar.gz`). Mirrors the
/// release workflow's build matrix.
const TARGET_TRIPLE: &str = if cfg!(all(target_arch = "x86_64", target_os = "linux")) {
    "x86_64-unknown-linux-gnu"
} else if cfg!(all(target_arch = "aarch64", target_os = "linux")) {
    "aarch64-unknown-linux-gnu"
} else if cfg!(all(target_arch = "aarch64", target_os = "macos")) {
    "aarch64-apple-darwin"
} else if cfg!(all(target_arch = "x86_64", target_os = "macos")) {
    "x86_64-apple-darwin"
} else {
    "unknown-target"
};

/// The running binary's version, from the crate manifest at compile time.
pub fn current_version() -> Semver {
    Semver::parse(env!("CARGO_PKG_VERSION")).expect("CARGO_PKG_VERSION is valid semver")
}

/// Parse a release tag (`v0.2.1` / `0.2.1`) into a version; `None` when the
/// tag is not a semver version.
pub fn version_from_tag(tag: &str) -> Option<Semver> {
    Semver::parse(tag.trim().trim_start_matches('v')).ok()
}

/// One newer stable release, ready to download. Passed from the check task
/// through [`crate::event::AppEvent::UpdateAvailable`] into the UI, then to
/// the (confirmed) update op.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    /// The running binary's version.
    pub current: Semver,
    /// The latest stable release's version.
    pub latest: Semver,
    /// Download URL of this platform's tarball asset.
    pub tarball_url: String,
    /// Download URL of the tarball's `.sha256` sibling asset.
    pub sha256_url: String,
}

/// Minimal slice of the GitHub release JSON we depend on.
#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    prerelease: bool,
    assets: Vec<ReleaseAsset>,
}

/// One downloadable release asset (tarball or its `.sha256` sibling).
#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

impl Release {
    /// Tarball + sha256 asset URLs for this platform, matched by the exact
    /// release-workflow asset names.
    fn platform_assets(&self, latest: &Semver) -> anyhow::Result<(String, String)> {
        if TARGET_TRIPLE == "unknown-target" {
            bail!("no release assets are built for this platform ({TARGET_TRIPLE})");
        }
        let want = format!("msb-tui-v{latest}-{TARGET_TRIPLE}.tar.gz");
        let tarball = self
            .assets
            .iter()
            .find(|a| a.name == want)
            .with_context(|| format!("release has no matching asset '{want}'"))?;
        let sha256 = self
            .assets
            .iter()
            .find(|a| a.name == format!("{want}.sha256"))
            .with_context(|| format!("release has no sibling asset '{want}.sha256'"))?;
        Ok((
            tarball.browser_download_url.clone(),
            sha256.browser_download_url.clone(),
        ))
    }
}

/// Check GitHub for a newer stable release. `Ok(None)` = running the
/// latest stable. Network or API failures bubble up as errors — startup
/// swallows them (offline or rate-limited starts stay silent).
pub async fn check() -> anyhow::Result<Option<UpdateInfo>> {
    let current = current_version();
    let client = reqwest::Client::builder()
        .user_agent(format!("msb-tui/{}", current))
        .timeout(Duration::from_secs(30))
        .build()
        .context("build HTTP client")?;
    let release: Release = client
        .get(RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .context("query latest release")?
        .error_for_status()
        .context("query latest release")?
        .json()
        .await
        .context("parse latest-release response")?;

    // Belt and braces: `/releases/latest` never returns a prerelease, but
    // honor the flag if it ever does.
    if release.prerelease {
        bail!("latest release {} is a prerelease", release.tag_name);
    }
    let latest = version_from_tag(&release.tag_name)
        .with_context(|| format!("release tag '{}' is not a version", release.tag_name))?;
    if latest <= current {
        return Ok(None);
    }
    let (tarball_url, sha256_url) = release.platform_assets(&latest)?;
    Ok(Some(UpdateInfo {
        current,
        latest,
        tarball_url,
        sha256_url,
    }))
}

/// Run the self-update: download the release tarball, verify its sha256,
/// extract the binary, and replace the running executable (a `.bak` backup
/// stays next to it). Long-running (network + disk) — call from a tokio
/// task, not the UI thread.
pub async fn perform(update: &UpdateInfo) -> anyhow::Result<()> {
    let client = reqwest::Client::builder()
        .user_agent(format!("msb-tui/{}", update.current))
        // Tarballs are a few MB; the sha file is tiny. One generous timeout
        // covers both.
        .timeout(Duration::from_secs(120))
        .build()
        .context("build HTTP client")?;
    let tarball = download(&client, &update.tarball_url, "tarball").await?;
    let sha256_text = download(&client, &update.sha256_url, "sha256").await?;
    let sha256_text =
        std::str::from_utf8(&sha256_text).context("the sha256 asset is not UTF-8 text")?;
    verify_sha256(&tarball, sha256_text)
        .context("the downloaded tarball does not match the release's sha256")?;
    let exe_bytes = tokio::task::spawn_blocking(move || extract_binary(&tarball))
        .await
        .context("extract task panicked")??;
    tokio::task::spawn_blocking(move || install_binary(&exe_bytes))
        .await
        .context("install task panicked")?
}

/// Fetch a URL as raw bytes, naming the part in errors.
async fn download(client: &reqwest::Client, url: &str, what: &str) -> anyhow::Result<Vec<u8>> {
    let bytes = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("download {what}"))?
        .error_for_status()
        .with_context(|| format!("download {what}"))?
        .bytes()
        .await
        .with_context(|| format!("read {what}"))?
        .to_vec();
    Ok(bytes)
}

/// Verify `data` against a `sha256sum`-style text (`<hex>  <name>`); errors
/// on format problems and, above all, on a mismatch (both hashes named).
pub fn verify_sha256(data: &[u8], sha256_text: &str) -> anyhow::Result<()> {
    use sha2::{Digest, Sha256};
    let expected = sha256_text
        .split_whitespace()
        .next()
        .with_context(|| "sha256 asset is empty")?
        .to_ascii_lowercase();
    if expected.len() != 64 {
        bail!("sha256 asset does not start with a hex digest: '{expected}'");
    }
    let actual = hex(&Sha256::digest(data));
    if actual != expected {
        bail!("sha256 mismatch: expected {expected}, got {actual}");
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Pull the `msb-tui` binary out of a packed release tarball. The archive
/// wraps it in one top-level directory; match on the file name, not the
/// whole path.
fn extract_binary(tarball_gz: &[u8]) -> anyhow::Result<Vec<u8>> {
    let gz = flate2::read::GzDecoder::new(tarball_gz);
    let mut archive = tar::Archive::new(gz);
    for entry in archive.entries().context("read the release tarball")? {
        let mut entry = entry.context("read tar entry")?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let is_binary = entry
            .path()
            .ok()
            .and_then(|p| p.file_name().map(|n| n == "msb-tui"))
            .unwrap_or(false);
        if is_binary {
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .context("read msb-tui from the tarball")?;
            return Ok(bytes);
        }
    }
    bail!("the release tarball contains no msb-tui binary")
}

/// Swap the running executable for the downloaded bytes: write a staging
/// file next to it, mark it executable, then rename (the old binary is kept
/// as `<name>.bak`; a failed second rename rolls it back). The running
/// process keeps its old inode — the update applies on relaunch.
pub fn install_binary(bytes: &[u8]) -> anyhow::Result<()> {
    let exe = std::env::current_exe().context("locate the running binary")?;
    install_binary_at(&exe, bytes)
}

/// [`install_binary`] on an explicit path (the real swap logic; unit-testable).
fn install_binary_at(exe: &std::path::Path, bytes: &[u8]) -> anyhow::Result<()> {
    let dir = exe.parent().context("resolve the install directory")?;
    let name = exe
        .file_name()
        .and_then(|n| n.to_str())
        .with_context(|| format!("resolve the file name of {}", exe.display()))?;
    let staging = dir.join(format!(".{name}.update"));
    let backup = dir.join(format!("{name}.bak"));

    // 1. Stage the new binary fully (write → fsync → chmod) before touching
    //    the running one.
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&staging)
            .with_context(|| format!("create {}", staging.display()))?;
        f.write_all(bytes)
            .with_context(|| format!("write {}", staging.display()))?;
        f.sync_all().context("fsync staged binary")?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("chmod {}", staging.display()))?;
    }

    // 2. Rotate: current → .bak, staged → current. If the final rename
    //    fails, put the backup back so the install is intact.
    std::fs::rename(exe, &backup)
        .with_context(|| format!("back up {} to {}", exe.display(), backup.display()))?;
    if let Err(e) = std::fs::rename(&staging, exe) {
        let _ = std::fs::rename(&backup, exe);
        return Err(
            anyhow::Error::new(e).context(format!("install {} (backup restored)", exe.display()))
        );
    }

    // 3. Best-effort directory fsync so the rename survives a crash.
    #[cfg(unix)]
    if let Ok(dirf) = std::fs::File::open(dir) {
        let _ = dirf.sync_all();
    }
    Ok(())
}

/// The dashboard banner line for an available update (the `[V]` hint must
/// stay in sync with `App::handle_dashboard_key`).
pub fn banner_text(update: &UpdateInfo) -> String {
    format!(
        "msb-tui v{} available (running v{}) — press [V] to update",
        update.latest, update.current
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;

    #[test]
    fn version_from_tag_strips_v() {
        assert_eq!(version_from_tag("v0.2.1"), Some(Semver::new(0, 2, 1)));
        assert_eq!(version_from_tag("0.3.0"), Some(Semver::new(0, 3, 0)));
        // Prerelease tags parse too; `check` filters them via the release
        // flag, not the tag text.
        assert_eq!(
            version_from_tag("v1.2.3-beta.1"),
            Some(Semver::parse("1.2.3-beta.1").unwrap())
        );
    }

    #[test]
    fn version_from_tag_rejects_garbage() {
        assert_eq!(version_from_tag("chore-release"), None);
        assert_eq!(version_from_tag(""), None);
    }

    #[test]
    fn check_is_newer_only_for_strictly_higher() {
        // The comparison lives in `check`; mirror its semantics via semver.
        let current = Semver::new(0, 2, 0);
        assert!(Semver::new(0, 2, 1) > current);
        assert!(!(Semver::new(0, 2, 0) > current));
        // A beta never outranks its own stable (prerelease ordering).
        assert!(Semver::parse("0.2.1-beta.1").unwrap() < Semver::new(0, 2, 1));
    }

    #[test]
    fn platform_assets_match_workflow_names() {
        let release: Release = serde_json::from_str(
            r#"{
                "tag_name": "v0.2.1",
                "prerelease": false,
                "assets": [
                    { "name": "msb-tui-v0.2.1-aarch64-apple-darwin.tar.gz",
                      "browser_download_url": "https://example.com/darwin.tar.gz" },
                    { "name": "msb-tui-v0.2.1-aarch64-apple-darwin.tar.gz.sha256",
                      "browser_download_url": "https://example.com/darwin.tar.gz.sha256" },
                    { "name": "msb-tui-v0.2.1-x86_64-unknown-linux-gnu.tar.gz",
                      "browser_download_url": "https://example.com/linux.tar.gz" },
                    { "name": "msb-tui-v0.2.1-x86_64-unknown-linux-gnu.tar.gz.sha256",
                      "browser_download_url": "https://example.com/linux.tar.gz.sha256" }
                ]
            }"#,
        )
        .unwrap();
        let latest = Semver::new(0, 2, 1);
        let (tar, sha) = release.platform_assets(&latest).expect("assets resolve");
        if TARGET_TRIPLE == "x86_64-unknown-linux-gnu" {
            assert_eq!(tar, "https://example.com/linux.tar.gz");
            assert_eq!(sha, "https://example.com/linux.tar.gz.sha256");
        } else if TARGET_TRIPLE == "aarch64-apple-darwin" {
            assert_eq!(tar, "https://example.com/darwin.tar.gz");
        } else {
            // aarch64-linux builds resolve too; the point is no panic/miss.
            assert!(tar.ends_with(".tar.gz"));
        }
    }

    #[test]
    fn platform_assets_fail_without_a_sibling_sha() {
        let release: Release = serde_json::from_str(
            r#"{
                "tag_name": "v0.2.1",
                "prerelease": false,
                "assets": [
                    { "name": "msb-tui-v0.2.1-x86_64-unknown-linux-gnu.tar.gz",
                      "browser_download_url": "https://example.com/linux.tar.gz" }
                ]
            }"#,
        )
        .unwrap();
        let latest = Semver::new(0, 2, 1);
        if TARGET_TRIPLE == "x86_64-unknown-linux-gnu" {
            let err = release.platform_assets(&latest).unwrap_err();
            assert!(err.to_string().contains(".sha256"), "{err}");
        }
    }

    #[test]
    fn verify_sha256_accepts_the_matching_digest() {
        // Well-known sha256("hello").
        let hello_hash = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        let file_text = format!("{hello_hash}  msb-tui-v0.2.1-x86_64-unknown-linux-gnu.tar.gz\n");
        verify_sha256(b"hello", &file_text).expect("matching digest passes");
        // Uppercase digests are accepted too.
        verify_sha256(b"hello", &file_text.to_uppercase()).expect("case-insensitive");
    }

    #[test]
    fn verify_sha256_rejects_a_tampered_tarball() {
        let hello_hash = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        let err = verify_sha256(b"tampered", hello_hash).unwrap_err();
        assert!(err.to_string().contains("mismatch"), "{err}");
        let err = verify_sha256(b"hello", "nothex").unwrap_err();
        assert!(err.to_string().contains("hex digest"), "{err}");
        let err = verify_sha256(b"hello", "   ").unwrap_err();
        assert!(err.to_string().contains("empty"), "{err}");
    }

    #[test]
    fn extract_binary_finds_the_nested_binary() {
        // Reproduce the release layout: one top-level directory holding the
        // binary (plus a README that must be skipped).
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
        append(&mut builder, "msb-tui-v0.2.1-x86_64/README.md", b"docs").unwrap();
        append(
            &mut builder,
            "msb-tui-v0.2.1-x86_64/msb-tui",
            b"BINARY-BYTES",
        )
        .unwrap();
        let gz = builder.into_inner().unwrap().finish().unwrap();

        let got = extract_binary(&gz).expect("binary extracted");
        assert_eq!(got, b"BINARY-BYTES");
    }

    #[test]
    fn extract_binary_fails_on_an_empty_archive() {
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
        append(&mut builder, "msb-tui-v0.2.1-x86_64/README.md", b"docs").unwrap();
        let gz = builder.into_inner().unwrap().finish().unwrap();
        let err = extract_binary(&gz).unwrap_err();
        assert!(err.to_string().contains("no msb-tui binary"), "{err}");
    }

    fn append(
        builder: &mut tar::Builder<GzEncoder<Vec<u8>>>,
        name: &str,
        data: &[u8],
    ) -> std::io::Result<()> {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder.append_data(&mut header, name, data)
    }

    #[cfg(unix)]
    #[test]
    fn install_binary_at_swaps_and_backs_up() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("msb-tui-selfup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("msb-tui");
        std::fs::write(&exe, b"old-bytes").unwrap();

        install_binary_at(&exe, b"new-bytes").expect("install succeeds");

        assert_eq!(std::fs::read(&exe).unwrap(), b"new-bytes");
        let backup = dir.join("msb-tui.bak");
        assert_eq!(std::fs::read(&backup).unwrap(), b"old-bytes");
        // No staging leftovers; the new binary is executable.
        assert!(!dir.join(".msb-tui.update").exists());
        let mode = std::fs::metadata(&exe).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111, "executable bits set: {mode:o}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn install_binary_at_is_idempotent_across_updates() {
        // A second update overwrites both the binary and the stale backup.
        let dir = std::env::temp_dir().join(format!("msb-tui-selfup2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("msb-tui");
        std::fs::write(&exe, b"v1").unwrap();
        install_binary_at(&exe, b"v2").unwrap();
        install_binary_at(&exe, b"v3").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"v3");
        assert_eq!(std::fs::read(dir.join("msb-tui.bak")).unwrap(), b"v2");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn banner_text_names_both_versions_and_the_key() {
        let update = UpdateInfo {
            current: Semver::new(0, 2, 0),
            latest: Semver::new(0, 2, 1),
            tarball_url: String::new(),
            sha256_url: String::new(),
        };
        let text = banner_text(&update);
        assert!(text.contains("v0.2.1"), "{text}");
        assert!(text.contains("v0.2.0"), "{text}");
        assert!(text.contains("[V]"), "{text}");
    }
}
