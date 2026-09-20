//! Runtime management: detect the installed `msb` host runtime, compare it
//! against the SDK version, and install/update the official bundle.
//!
//! Detection reconciles both install locations:
//! - `PATH` lookup (user-installed msb, e.g. `~/.local/bin/msb`)
//! - `~/.microsandbox/bin/msb` (SDK `Setup`-installed bundle)
//!
//! The version is read from the binary's embedded `.msbver` section without
//! executing it (`setup::resolve_runtime_version`).

#![allow(dead_code)] // callers land with the main.rs switch (Task 4)

use std::path::PathBuf;

use anyhow::{Context, Result};
use semver::Version as Semver;

/// The SDK's own version — the version the TUI was compiled against and the
/// version `install_runtime` installs by default.
pub fn sdk_version() -> Semver {
    Semver::parse(microsandbox_utils::PREBUILT_VERSION).expect("PREBUILT_VERSION is valid semver")
}

/// One discovered msb installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledRuntime {
    /// Path to the `msb` executable.
    pub path: PathBuf,
    /// Embedded version, when readable (`None` = very old release).
    pub version: Option<Semver>,
}

/// Reconciled view of the host runtime state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeStatus {
    /// No msb found in either location.
    Missing,
    /// msb installed; `runtime` is the resolved (preferred) installation.
    Installed {
        runtime: InstalledRuntime,
        /// A different-version install also exists in the other location.
        other: Option<InstalledRuntime>,
    },
}

/// Detect installed msb binaries across PATH and the SDK home, preferring
/// the SDK home (`~/.microsandbox/bin`) over a PATH hit.
pub fn detect() -> RuntimeStatus {
    let home = sdk_home_runtime();
    let path = path_runtime();

    match (home, path) {
        (Some(runtime), other) => {
            let runtime_path = runtime.path.clone();
            RuntimeStatus::Installed {
                runtime,
                other: other.filter(|o| o.path != runtime_path),
            }
        }
        (None, Some(runtime)) => RuntimeStatus::Installed {
            runtime,
            other: None,
        },
        (None, None) => RuntimeStatus::Missing,
    }
}

/// The SDK home install: `~/.microsandbox/bin/msb` (honoring `$MSB_HOME`).
fn sdk_home_runtime() -> Option<InstalledRuntime> {
    let home = microsandbox_utils::resolve_home();
    let msb = home.join("bin").join("msb");
    installed_at(msb)
}

/// A PATH lookup for `msb` (manually scans `$PATH` entries).
fn path_runtime() -> Option<InstalledRuntime> {
    let path = std::env::var_os("PATH")?;
    let hit = std::env::split_paths(&path)
        .map(|dir| dir.join("msb"))
        .find(|candidate| candidate.is_file())?;
    installed_at(hit)
}

fn installed_at(path: PathBuf) -> Option<InstalledRuntime> {
    match microsandbox::setup::resolve_runtime_version(&path) {
        Ok(version) => Some(InstalledRuntime { version, path }),
        // Unreadable/non-regular file — treat as absent.
        Err(_) => None,
    }
}

/// Compare installed vs SDK version: what the banner should say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionRelation {
    /// Installed matches the SDK version — nothing to do.
    InSync,
    /// Installed is older than the SDK — offer update.
    Outdated,
    /// Installed is newer than the SDK — persistent hint (SDK may lag the CLI).
    Newer,
    /// Version unreadable (pre-`.msbver` release) — show path, skip compare.
    Unknown,
}

/// Classify the installed version against the SDK version.
pub fn relation(installed: &InstalledRuntime) -> VersionRelation {
    match &installed.version {
        None => VersionRelation::Unknown,
        Some(v) if *v == sdk_version() => VersionRelation::InSync,
        Some(v) if *v < sdk_version() => VersionRelation::Outdated,
        Some(_) => VersionRelation::Newer,
    }
}

/// The banner line for the dashboard, if any (`None` = nothing to show).
pub fn banner_text() -> Option<String> {
    match detect() {
        RuntimeStatus::Missing => Some(format!(
            "microsandbox runtime not found — press {} to install v{}",
            "[U]",
            sdk_version()
        )),
        RuntimeStatus::Installed { runtime, .. } => match relation(&runtime) {
            VersionRelation::InSync => None,
            VersionRelation::Outdated => Some(format!(
                "msb v{} installed, v{} available — press {} to update",
                runtime
                    .version
                    .as_ref()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "?".into()),
                sdk_version(),
                "[U]"
            )),
            VersionRelation::Newer => Some(format!(
                "msb v{} is newer than the SDK (v{}) — TUI features may lag; consider matching versions",
                runtime
                    .version
                    .as_ref()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "?".into()),
                sdk_version()
            )),
            VersionRelation::Unknown => Some(format!(
                "msb version unreadable at {} — SDK v{} is the supported pair",
                runtime.path.display(),
                sdk_version()
            )),
        },
    }
}

/// Outcome of an install/update run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallOutcome {
    /// Runtime installed (or already present and verified).
    Installed {
        /// Path of the installed msb.
        msb_path: PathBuf,
    },
    /// A complete runtime already resolved; nothing was done.
    AlreadyCurrent,
}

/// Install or update the host runtime via the SDK's `Setup` bundle
/// (msb + msbnet + libkrunfw, sha256-verified) into `~/.microsandbox/`.
///
/// Long-running (network + extract) — call from a tokio task, not the UI thread.
pub async fn install_or_update() -> Result<InstallOutcome> {
    let config = microsandbox::config::load_persisted_config_or_default()
        .context("load microsandbox config")?;

    // Force: replace a complete installation with the exact SDK version.
    let options = microsandbox::setup::InstallOptions {
        source: microsandbox::setup::InstallSource::ReleaseDownload,
        version: sdk_version().to_string(),
        force: true,
        verify: true,
        expected_archive_sha256: None,
    };

    let resolved = microsandbox::setup::install_runtime(&config, options).await?;
    Ok(InstallOutcome::Installed {
        msb_path: resolved.msb_path,
    })
}

/// Ensure the runtime exists, installing only when wholly absent.
pub async fn ensure_installed() -> Result<ResolvedPath> {
    let config = microsandbox::config::load_persisted_config_or_default()
        .context("load microsandbox config")?;
    let resolved = microsandbox::setup::ensure_runtime(
        &config,
        microsandbox::setup::InstallOptions {
            version: sdk_version().to_string(),
            ..Default::default()
        },
    )
    .await?;
    Ok(ResolvedPath {
        msb: resolved.msb_path,
        libkrunfw: resolved.libkrunfw_path,
    })
}

/// Resolved host runtime paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPath {
    /// Path to msb.
    pub msb: PathBuf,
    /// Path to the matching libkrunfw.
    pub libkrunfw: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_version_parses() {
        assert!(sdk_version() >= Semver::new(0, 7, 0));
    }

    #[test]
    fn relation_classifies_sync() {
        let rt = InstalledRuntime {
            path: PathBuf::from("/usr/bin/msb"),
            version: Some(sdk_version()),
        };
        assert_eq!(relation(&rt), VersionRelation::InSync);
    }

    #[test]
    fn relation_classifies_outdated_and_newer() {
        let old = InstalledRuntime {
            path: PathBuf::from("/usr/bin/msb"),
            version: Some(Semver::new(0, 6, 9)),
        };
        assert_eq!(relation(&old), VersionRelation::Outdated);

        let new = InstalledRuntime {
            path: PathBuf::from("/usr/bin/msb"),
            version: Some(Semver::new(0, 8, 0)),
        };
        assert_eq!(relation(&new), VersionRelation::Newer);
    }

    #[test]
    fn relation_classifies_unknown() {
        let rt = InstalledRuntime {
            path: PathBuf::from("/usr/bin/msb"),
            version: None,
        };
        assert_eq!(relation(&rt), VersionRelation::Unknown);
    }

    #[test]
    fn banner_text_returns_some_string_type() {
        // Smoke: the function must compile and produce owned strings when the
        // environment has no runtime (CI); the exact value is env-dependent.
        let _ = banner_text();
    }
}
