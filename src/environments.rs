//! Environment profiles: named env-var bundles injected at sandbox create
//! time (GitHub issue #7). One human-editable file,
//! `~/.config/microsandbox-tui/environments.toml`, holds a top-level
//! `[[profile]]` array; there are no built-in profiles. Templates reference
//! a profile by name via `TemplateSpec::env_profile`; the create form can
//! pick one too. A profile's `env` entries use the CLI form `KEY=VALUE`.
//!
//! Merge order at create time is documented on
//! [`merge_env`] and [`crate::template::to_create_spec`]: profile vars
//! first, then form/template vars, then credential vars last (the SDK
//! applies last-wins on duplicate keys).

use anyhow::Context;
use std::path::{Path, PathBuf};

/// A named env-var profile (`[[profile]]` entry).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Profile {
    /// Identity within the file; referenced by templates and the form.
    pub name: String,
    /// One-line description; may be empty.
    #[serde(default)]
    pub description: String,
    /// Environment variables in `KEY=VALUE` form.
    #[serde(default)]
    pub env: Vec<String>,
}

/// Serde view of the environments document (`[[profile]]` entries).
#[derive(Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct ProfilesFile {
    profile: Vec<Profile>,
}

/// The environments file path:
/// `$XDG_CONFIG_HOME/microsandbox-tui/environments.toml`, else
/// `$HOME/.config/…` (Spec §2 — same rule as template.rs/autostart.rs).
pub fn environments_file() -> std::path::PathBuf {
    environments_file_in(&config_base())
}

pub(crate) fn environments_file_in(base: &Path) -> PathBuf {
    base.join("microsandbox-tui/environments.toml")
}

/// The msb-tui config dir: `$XDG_CONFIG_HOME`, else `$HOME/.config`
/// (shared by environments.toml and credentials.toml; same rule as
/// template.rs §2 and autostart.rs).
fn config_base() -> PathBuf {
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME")
        && !x.is_empty()
    {
        return PathBuf::from(x);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    Path::new(&home).join(".config")
}

/// Load every profile in file order. A missing file is not an error:
/// no profiles, no warnings. An invalid entry (bad name, malformed env
/// entry) or a duplicate name is — the file is human-edited, and a broken
/// profile must not silently poison creates.
pub fn load_profiles() -> anyhow::Result<Vec<Profile>> {
    load_profiles_from(&environments_file())
}

fn load_profiles_from(path: &Path) -> anyhow::Result<Vec<Profile>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let file: ProfilesFile =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(file.profile.len());
    for p in &file.profile {
        validate_profile(p).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        if !seen.insert(p.name.as_str()) {
            anyhow::bail!(
                "{}: duplicate environment profile '{}'",
                path.display(),
                p.name
            );
        }
        out.push(p.clone());
    }
    Ok(out)
}

/// Save the whole profile set in file order, creating parent directories.
/// Writes through a temp file + rename (autostart-style) so a torn write
/// can never leave a truncated document.
pub fn save_profiles(profiles: &[Profile]) -> anyhow::Result<()> {
    save_profiles_to(&environments_file(), profiles)
}

fn save_profiles_to(path: &Path, profiles: &[Profile]) -> anyhow::Result<()> {
    let body = toml::to_string_pretty(&ProfilesFile {
        profile: profiles.to_vec(),
    })
    .map_err(|e| anyhow::anyhow!("serialize environment profiles: {e}"))?;
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Editor-dialog validation: name must be non-empty (after trim) and
/// unique-checked by the caller; every env entry must be `KEY=VALUE`
/// with a non-empty key; the description may be empty.
pub fn validate_profile(p: &Profile) -> Result<(), String> {
    if p.name.trim().is_empty() {
        return Err("profile name must not be empty".to_string());
    }
    for entry in &p.env {
        let Some((key, _)) = entry.split_once('=') else {
            return Err(format!("env entry '{entry}' must be KEY=VALUE"));
        };
        if key.is_empty() {
            return Err(format!("env entry '{entry}': key must not be empty"));
        }
    }
    Ok(())
}

/// Find a profile by exact name.
pub fn lookup_profile<'a>(profiles: &'a [Profile], name: &str) -> Option<&'a Profile> {
    profiles.iter().find(|p| p.name == name)
}

/// Concatenate env var lists: profile vars first, form/template vars
/// second. The SDK applies last-wins on duplicate keys, so earlier
/// entries are overridden by later ones on the same key.
pub fn merge_env(profile_vars: &[String], form_vars: &[String]) -> Vec<String> {
    profile_vars
        .iter()
        .chain(form_vars.iter())
        .cloned()
        .collect()
}

/// A profile's env vars. Errors when the name is not an existing profile.
pub fn profile_env(profiles: &[Profile], name: &str) -> anyhow::Result<Vec<String>> {
    match lookup_profile(profiles, name) {
        Some(p) => Ok(p.env.clone()),
        None => anyhow::bail!("unknown environment profile '{name}'"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "msb-tui-env-{}-{tag}/environments.toml",
            std::process::id()
        ))
    }

    fn profile(name: &str, env: &[&str]) -> Profile {
        Profile {
            name: name.to_string(),
            description: "test".to_string(),
            env: env.iter().map(|e| e.to_string()).collect(),
        }
    }

    #[test]
    fn save_load_roundtrips_in_file_order() {
        let path = temp_path("roundtrip");
        let profiles = vec![profile("dev", &["A=1"]), profile("prod", &["B=2", "C=3"])];
        save_profiles_to(&path, &profiles).unwrap();
        let loaded = load_profiles_from(&path).unwrap();
        assert_eq!(loaded, profiles);
        // Whole-set update (delete + edit) also round-trips.
        let updated = vec![profile("prod", &["B=2"])];
        save_profiles_to(&path, &updated).unwrap();
        assert_eq!(load_profiles_from(&path).unwrap(), updated);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn missing_file_loads_empty_no_error() {
        assert!(
            load_profiles_from(Path::new("/nonexistent/msb-tui/environments.toml"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn loads_toml_with_empty_description_and_env() {
        let path = temp_path("sparse");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "[[profile]]\nname = \"dev\"\n").unwrap();
        let loaded = load_profiles_from(&path).unwrap();
        assert_eq!(
            loaded,
            vec![Profile {
                name: "dev".into(),
                description: String::new(),
                env: Vec::new(),
            }]
        );
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn duplicate_names_on_load_are_an_error() {
        let path = temp_path("dup");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "[[profile]]\nname = \"dev\"\n\n[[profile]]\nname = \"dev\"\n",
        )
        .unwrap();
        let e = load_profiles_from(&path).unwrap_err().to_string();
        assert!(e.contains("duplicate"), "got: {e}");
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn validate_rejects_empty_name() {
        let e = validate_profile(&profile("", &[])).unwrap_err();
        assert!(e.contains("name"), "{e}");
    }

    #[test]
    fn validate_rejects_non_kv_env_entries() {
        let e = validate_profile(&profile("dev", &["NOVALUE"])).unwrap_err();
        assert!(e.contains("KEY=VALUE"), "{e}");
        let e = validate_profile(&profile("dev", &["=VALUE"])).unwrap_err();
        assert!(e.contains("key must not be empty"), "{e}");
    }

    #[test]
    fn validate_accepts_empty_description_and_value() {
        assert!(validate_profile(&profile("dev", &["KEY="])).is_ok());
    }

    #[test]
    fn lookup_finds_by_exact_name() {
        let profiles = vec![profile("dev", &["A=1"]), profile("prod", &["B=2"])];
        assert_eq!(
            lookup_profile(&profiles, "prod").map(|p| p.name.as_str()),
            Some("prod")
        );
        assert!(lookup_profile(&profiles, "Dev").is_none());
        assert!(lookup_profile(&profiles, "staging").is_none());
    }

    #[test]
    fn merge_env_is_concatenation_form_wins_on_duplicates() {
        let merged = merge_env(
            &["A=1".to_string(), "B=1".to_string()],
            &["A=2".to_string(), "C=3".to_string()],
        );
        // Order: profile vars first, form vars appended after — the SDK
        // applies last-wins, so the form's A=2 overrides the profile's A=1.
        assert_eq!(merged, vec!["A=1", "B=1", "A=2", "C=3"]);
    }

    #[test]
    fn profile_env_errors_on_unknown_name() {
        let profiles = vec![profile("dev", &["A=1"])];
        let e = profile_env(&profiles, "staging").unwrap_err().to_string();
        assert!(e.contains("unknown environment profile 'staging'"), "{e}");
        assert_eq!(
            profile_env(&profiles, "dev").unwrap(),
            vec!["A=1".to_string()]
        );
    }

    #[test]
    fn file_path_sits_in_the_config_dir() {
        assert_eq!(
            environments_file_in(Path::new("/cfg")),
            Path::new("/cfg/microsandbox-tui/environments.toml")
        );
    }
}
