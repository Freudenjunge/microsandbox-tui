//! Credentials: OS-keyring-backed secrets injected as env vars at sandbox
//! create time (GitHub issue #6). One human-editable file,
//! `~/.config/microsandbox-tui/credentials.toml`, holds the *metadata*
//! (`[[credential]]` entries with `name`, `description`, `env_key`, and
//! optional `profiles`/`sandboxes` scoping lists); the secret *values*
//! live ONLY in the OS keyring (service [`KEYRING_SERVICE`], account =
//! credential name) — never on disk, never in logs, never in TOML.
//!
//! Stores per platform: Secret Service on Linux
//! (`sync-secret-service`), Keychain on macOS (`apple-native`), Windows
//! Credential Manager on Windows (`windows-native`).

use anyhow::Context as _;

/// The keyring service name all msb-tui credentials live under.
pub const KEYRING_SERVICE: &str = "msb-tui";

/// A credential (`[[credential]]` entry): which env var a named secret
/// becomes, scoped to optional environment profiles and sandbox names.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Credential {
    /// Identity; also the keyring account name.
    pub name: String,
    /// One-line description; may be empty.
    #[serde(default)]
    pub description: String,
    /// Env var the secret is injected as at create time.
    pub env_key: String,
    /// Environment profiles this credential is attached to (empty = unassigned; injected nowhere).
    #[serde(default)]
    pub profiles: Vec<String>,
    /// Sandboxes this credential is attached to (empty = unassigned; injected nowhere).
    #[serde(default)]
    pub sandboxes: Vec<String>,
}

/// Serde view of the credentials document (`[[credential]]` entries).
#[derive(Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct CredentialsFile {
    /// TOML array-of-tables name is singular; the Rust field is not.
    #[serde(rename = "credential")]
    pub credentials: Vec<Credential>,
}

/// The credentials file path:
/// `$XDG_CONFIG_HOME/microsandbox-tui/credentials.toml`, else
/// `$HOME/.config/…` (Spec §2 — same rule as template.rs/environments.rs).
pub fn credentials_file() -> std::path::PathBuf {
    credentials_file_in(&config_base())
}

pub(crate) fn credentials_file_in(base: &std::path::Path) -> std::path::PathBuf {
    base.join("microsandbox-tui/credentials.toml")
}

/// The msb-tui config dir: `$XDG_CONFIG_HOME`, else `$HOME/.config`
/// (same rule as template.rs §2).
fn config_base() -> std::path::PathBuf {
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME")
        && !x.is_empty()
    {
        return std::path::PathBuf::from(x);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    std::path::Path::new(&home).join(".config")
}

/// Load the metadata in file order (insertion order is preserved). A
/// missing file is not an error: an empty set. An invalid entry (bad
/// name, non-identifier `env_key`) or a duplicate name is — the file is
/// human-edited, and a broken credential must not silently inject nothing.
pub fn load_credentials() -> anyhow::Result<CredentialsFile> {
    load_credentials_from(&credentials_file())
}

fn load_credentials_from(path: &std::path::Path) -> anyhow::Result<CredentialsFile> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CredentialsFile::default());
        }
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let file: CredentialsFile =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let mut seen = std::collections::HashSet::new();
    for c in &file.credentials {
        validate_credential(c).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        if !seen.insert(c.name.as_str()) {
            anyhow::bail!("{}: duplicate credential '{}'", path.display(), c.name);
        }
    }
    Ok(file)
}

/// Save the whole metadata set in file order, creating parent
/// directories. Writes through a temp file + rename (autostart-style) so
/// a torn write can never leave a truncated document.
pub fn save_credentials(file: &CredentialsFile) -> anyhow::Result<()> {
    save_credentials_to(&credentials_file(), file)
}

fn save_credentials_to(path: &std::path::Path, file: &CredentialsFile) -> anyhow::Result<()> {
    let body =
        toml::to_string_pretty(file).map_err(|e| anyhow::anyhow!("serialize credentials: {e}"))?;
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

/// Editor-dialog validation: name and `env_key` must be non-empty and
/// `env_key` a valid env-var identifier (`[A-Za-z_][A-Za-z0-9_]*`).
pub fn validate_credential(c: &Credential) -> Result<(), String> {
    if c.name.trim().is_empty() {
        return Err("credential name must not be empty".to_string());
    }
    if !is_env_ident(&c.env_key) {
        return Err(format!(
            "'{}' is not a valid env var name ([A-Za-z_][A-Za-z0-9_]*)",
            c.env_key
        ));
    }
    Ok(())
}

/// `env_key` must be a plausible env-var identifier.
fn is_env_ident(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Read the secret for credential `name` (the keyring account). Blocking
/// keyring I/O on a spawn_blocking task; the TUI awaits it.
pub async fn keyring_get(name: &str) -> anyhow::Result<String> {
    let name = name.to_string();
    let res: Result<String, keyring::Error> = tokio::task::spawn_blocking({
        let name = name.clone();
        move || keyring::Entry::new(KEYRING_SERVICE, &name).and_then(|entry| entry.get_password())
    })
    .await
    .map_err(|e| anyhow::anyhow!("keyring read task cancelled: {e}"))?;
    res.map_err(|e| keyring_error(&name, e))
}

/// Store the secret for credential `name`; overwrites an existing value.
pub async fn keyring_set(name: &str, secret: &str) -> anyhow::Result<()> {
    let name = name.to_string();
    let secret = secret.to_string();
    let res: Result<(), keyring::Error> = tokio::task::spawn_blocking({
        let name = name.clone();
        move || {
            keyring::Entry::new(KEYRING_SERVICE, &name)
                .and_then(|entry| entry.set_password(&secret))
        }
    })
    .await
    .map_err(|e| anyhow::anyhow!("keyring write task cancelled: {e}"))?;
    res.map_err(|e| keyring_error(&name, e))
}

/// Remove the secret for credential `name`. A missing secret is not an
/// error (deleting a credential that never had one is idempotent).
pub async fn keyring_delete(name: &str) -> anyhow::Result<()> {
    let name = name.to_string();
    let res: Result<(), keyring::Error> = tokio::task::spawn_blocking({
        let name = name.clone();
        move || {
            keyring::Entry::new(KEYRING_SERVICE, &name).and_then(|entry| entry.delete_credential())
        }
    })
    .await
    .map_err(|e| anyhow::anyhow!("keyring delete task cancelled: {e}"))?;
    match res {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(keyring_error(&name, e)),
    }
}

/// The env-var entry a secret becomes at create time: `KEY=VALUE`.
pub fn secret_env_entry(env_key: &str, secret: &str) -> String {
    format!("{env_key}={secret}")
}

/// Map a [`keyring::Error`] 1:1 into a user-readable anyhow message. The
/// keyring returns platform causes only in `PlatformFailure` /
/// `NoStorageAccess`; carry them through verbatim.
fn keyring_error(name: &str, e: keyring::Error) -> anyhow::Error {
    match e {
        keyring::Error::NoEntry => anyhow::anyhow!("credential '{name}' not found in keyring"),
        keyring::Error::PlatformFailure(cause) => {
            anyhow::anyhow!("keyring platform failure: {cause}")
        }
        keyring::Error::NoStorageAccess(cause) => {
            anyhow::anyhow!("keyring storage is locked or inaccessible: {cause}")
        }
        keyring::Error::BadEncoding(_) => {
            anyhow::anyhow!("credential '{name}' in the keyring is not valid UTF-8")
        }
        keyring::Error::TooLong(attr, limit) => {
            anyhow::anyhow!("keyring attribute '{attr}' too long (platform limit {limit})")
        }
        keyring::Error::Invalid(attr, reason) => {
            anyhow::anyhow!("keyring rejects '{attr}': {reason}")
        }
        keyring::Error::Ambiguous(_) => {
            anyhow::anyhow!("keyring holds multiple entries for '{name}'")
        }
        other => anyhow::anyhow!("keyring error for '{name}': {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "msb-tui-cred-{}-{tag}/credentials.toml",
            std::process::id()
        ))
    }

    fn credential(name: &str, env_key: &str, profiles: &[&str], sandboxes: &[&str]) -> Credential {
        Credential {
            name: name.to_string(),
            description: "test".to_string(),
            env_key: env_key.to_string(),
            profiles: profiles.iter().map(|p| p.to_string()).collect(),
            sandboxes: sandboxes.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn save_load_roundtrips_with_scopes() {
        let path = temp_path("roundtrip");
        let file = CredentialsFile {
            credentials: vec![
                credential("openai", "OPENAI_API_KEY", &["dev"], &[]),
                credential("gh", "GITHUB_TOKEN", &["dev", "prod"], &["claude", "codex"]),
            ],
        };
        save_credentials_to(&path, &file).unwrap();
        assert_eq!(load_credentials_from(&path).unwrap(), file);
        // Insertion order preserved on disk: second stored entry loads second.
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn missing_file_loads_empty_no_error() {
        assert!(
            load_credentials_from(Path::new("/nonexistent/msb-tui/credentials.toml"))
                .unwrap()
                .credentials
                .is_empty()
        );
    }

    #[test]
    fn loads_toml_with_empty_description_and_scopes() {
        let path = temp_path("sparse");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "[[credential]]\nname = \"openai\"\nenv_key = \"OPENAI_API_KEY\"\n",
        )
        .unwrap();
        let file = load_credentials_from(&path).unwrap();
        assert_eq!(
            file.credentials,
            vec![Credential {
                name: "openai".into(),
                description: String::new(),
                env_key: "OPENAI_API_KEY".into(),
                profiles: Vec::new(),
                sandboxes: Vec::new(),
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
            "[[credential]]\nname = \"gh\"\nenv_key = \"A\"\n\n[[credential]]\nname = \"gh\"\nenv_key = \"B\"\n",
        )
        .unwrap();
        let e = load_credentials_from(&path).unwrap_err().to_string();
        assert!(e.contains("duplicate"), "got: {e}");
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn validate_rejects_empty_name_or_key() {
        assert!(validate_credential(&credential("", "A", &[], &[])).is_err());
        assert!(validate_credential(&credential("gh", "", &[], &[])).is_err());
    }

    #[test]
    fn validate_rejects_non_ident_env_key() {
        for bad in ["1FOO", "A-B", "A B", "ÜBER", "FOO "] {
            let e = validate_credential(&credential("gh", bad, &[], &[])).unwrap_err();
            assert!(e.contains("not a valid env var name"), "{bad}: {e}");
        }
        for good in ["FOO", "FOO_BAR_1", "_X"] {
            assert!(
                validate_credential(&credential("gh", good, &[], &[])).is_ok(),
                "{good}"
            );
        }
    }

    #[test]
    fn secret_env_entry_is_key_value() {
        assert_eq!(
            secret_env_entry("GITHUB_TOKEN", "s3kr3t"),
            "GITHUB_TOKEN=s3kr3t"
        );
    }

    #[test]
    fn service_is_namespaced() {
        assert_eq!(KEYRING_SERVICE, "msb-tui");
    }

    #[test]
    fn file_path_sits_in_the_config_dir() {
        assert_eq!(
            credentials_file_in(Path::new("/cfg")),
            Path::new("/cfg/microsandbox-tui/credentials.toml")
        );
    }
}
