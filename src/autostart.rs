//! Boot-autostart registry: which sandboxes start when the system boots.
//!
//! microsandbox has no built-in restart-on-boot (0.7.2): a boot kills the
//! detached `msb machine` processes; only each sandbox's config record
//! survives. This module marks the restart-worthy ones in
//! `~/.config/microsandbox-tui/autostart.toml` — a human-editable list —
//! and provides the on-disk plumbing:
//!
//!   - [`parse`]/[`serialize`]: the TOML document format
//!     (`sandboxes = ["claude", "devin"]`)
//!   - [`load`]/[`save`]: file I/O; a malformed document degrades to an
//!     empty list plus a warning, never a crash
//!
//! The boot-time start pass lives in [`crate::actions::restore_pass`]; the
//! systemd unit writer/installer below is driven by the `msb-tui autostart`
//! CLI (see `main.rs`).

use anyhow::Context;
use std::path::{Path, PathBuf};

/// The autostart file path: `$XDG_CONFIG_HOME/microsandbox-tui/autostart.toml`,
/// else `$HOME/.config/microsandbox-tui/autostart.toml` (Spec: template.rs §2).
pub(crate) fn autostart_path() -> PathBuf {
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME")
        && !x.is_empty()
    {
        return Path::new(&x).join("microsandbox-tui/autostart.toml");
    }
    let home = std::env::var("HOME").unwrap_or_default();
    Path::new(&home).join(".config/microsandbox-tui/autostart.toml")
}

/// The autostart file: one simple list of sandbox names.
#[derive(Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct AutostartFile {
    #[serde(default)]
    sandboxes: Vec<String>,
}

/// Normalize names: trim, drop blanks, sort, deduplicate. The file and the
/// app-side set always agree on this canonical order.
fn normalize(names: &[String]) -> Vec<String> {
    let mut out: Vec<String> = names
        .iter()
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Parse an autostart document: names (sorted, deduplicated, blanks
/// dropped) plus warnings for a malformed document. A malformed document
/// degrades to an empty list with a warning — never a crash.
pub(crate) fn parse(text: &str) -> (Vec<String>, Vec<String>) {
    match toml::from_str::<AutostartFile>(text) {
        Ok(f) => (normalize(&f.sandboxes), Vec::new()),
        Err(e) => (Vec::new(), vec![format!("autostart file: {e}")]),
    }
}

/// Serialize names to the document format (`sandboxes = [ … ]`); always
/// sorted + deduplicated.
pub(crate) fn serialize(names: &[String]) -> String {
    let file = AutostartFile {
        sandboxes: normalize(names),
    };
    toml::to_string(&file).unwrap_or_default()
}

/// Load from `path`. A missing file is not an error: empty list, no
/// warnings.
pub(crate) fn load(path: &Path) -> (Vec<String>, Vec<String>) {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Vec::new(), Vec::new()),
        Err(e) => (Vec::new(), vec![format!("autostart file: {e}")]),
    }
}

/// Save `names` to `path`, creating parent directories.
pub(crate) fn save(path: &Path, names: &[String]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serialize(names))?;
    Ok(())
}

/// One full-set change to persist (queued by the app, written by the main
/// loop — App stays free of I/O).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AutostartChange {
    /// The full new mark set at change time.
    pub(crate) names: Vec<String>,
}

/// Dashboard banner hint: marks exist but the systemd unit is not
/// installed. `None` in every other case — no state, no nag.
pub(crate) fn hint_text(marked: usize, installed: bool) -> Option<String> {
    if marked == 0 || installed {
        return None;
    }
    Some(format!(
        "boot autostart: {marked} sandbox{} marked but the systemd unit is not installed — run `msb-tui autostart install`",
        if marked == 1 { "" } else { "es" }
    ))
}

/// The systemd user unit that runs the boot restore pass.
///
/// `Type=exec` + `RemainAfterExit` (a oneshot cannot carry `Restart=` on
/// older systemd): the pass starts the marked sandboxes and exits; when
/// it *fails* (e.g. the SDK raced early boot), systemd retries after 30 s
/// — self-healing, no host-side ordering unit dance needed.
pub(crate) fn unit_contents(exe: &Path) -> String {
    format!(
        "[Unit]
Description=msb-tui boot autostart: start marked microsandbox sandboxes
After=network-online.target
Wants=network-online.target

[Service]
Type=exec
ExecStart={} autostart
RemainAfterExit=yes
Restart=on-failure
RestartSec=30
# The SDK locates the msb runtime in the SDK home; at boot the user
# manager's PATH is minimal, so make the common install locations visible.
Environment=PATH=%h/.microsandbox/bin:%h/.local/bin:%h/.cargo/bin:%h/bin:/usr/local/bin:/usr/bin:/bin

[Install]
WantedBy=default.target
",
        exe.display()
    )
}

/// The unit file path: `$XDG_CONFIG_HOME/systemd/user/msb-tui-autostart.service`,
/// else `$HOME/.config/systemd/user/…`.
pub(crate) fn unit_path_at(config_dir: &Path) -> PathBuf {
    config_dir.join("systemd/user/msb-tui-autostart.service")
}

/// The TUI config dir: `$XDG_CONFIG_HOME`, else `$HOME/.config` (same rule
/// as template.rs §2, shared by both files).
fn config_dir() -> PathBuf {
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME")
        && !x.is_empty()
    {
        return Path::new(&x).to_path_buf();
    }
    let home = std::env::var("HOME").unwrap_or_default();
    Path::new(&home).join(".config")
}

/// Whether the unit file exists ([`is_installed_at`] is the tested core).
#[cfg_attr(test, allow(dead_code))]
pub(crate) fn is_installed() -> bool {
    is_installed_at(&config_dir())
}

/// Whether the unit file exists.
pub(crate) fn is_installed_at(config_dir: &Path) -> bool {
    unit_path_at(config_dir).is_file()
}

/// Write the unit file under `config_dir`, creating the `systemd/user`
/// directories. Returns the unit path.
pub(crate) fn write_unit_at(config_dir: &Path, exe: &Path) -> anyhow::Result<PathBuf> {
    let unit = unit_path_at(config_dir);
    if let Some(parent) = unit.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&unit, unit_contents(exe))?;
    Ok(unit)
}

/// Write the unit file, then enable it. Runs `systemctl --user` and a
/// best-effort `loginctl enable-linger` (typically needs interactive auth;
/// when it fails the unit still works for *login* sessions — the hint in
/// the returned lines tells how to make it boot-persistent). Returns the
/// actions taken, for the CLI to echo.
///
/// Untested: it shells out to the host. Everything it writes is covered by
/// the tested pieces above.
pub(crate) fn install() -> anyhow::Result<Vec<String>> {
    if !cfg!(target_os = "linux") {
        anyhow::bail!("boot autostart is only implemented for Linux/systemd on this platform");
    }
    let exe = std::env::current_exe().context("locate the msb-tui binary")?;
    let unit = write_unit_at(&config_dir(), &exe)?;
    let mut ran = vec![format!("wrote {}", unit.display())];
    for args in [
        vec!["systemctl", "--user", "daemon-reload"],
        vec![
            "systemctl",
            "--user",
            "enable",
            "--now",
            "msb-tui-autostart.service",
        ],
    ] {
        run_quiet(&args).with_context(|| format!("run `{}`", args.join(" ")))?;
        ran.push(args.join(" "));
    }
    let user = std::env::var("USER").unwrap_or_default();
    match run_quiet(&["loginctl", "enable-linger", &user]) {
        Ok(()) => ran.push("loginctl enable-linger".to_string()),
        Err(e) => ran.push(format!(
            "loginctl enable-linger needs root (run it once with sudo) to start sandboxes\
             without logging in; unit install otherwise OK ({e:#})"
        )),
    }
    Ok(ran)
}

/// Run a command, capturing output; map failure to an error.
fn run_quiet(args: &[&str]) -> anyhow::Result<()> {
    let out = std::process::Command::new(args[0])
        .args(&args[1..])
        .output()
        .with_context(|| format!("spawn `{}`", args.join(" ")))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if stderr.is_empty() {
        anyhow::bail!("`{}` exited with {}", args.join(" "), out.status);
    }
    anyhow::bail!("`{}`: {stderr}", args.join(" "));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("msb-tui-autostart-{}-{tag}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        dir
    }

    #[test]
    fn parse_reads_names_sorted_and_deduplicated() {
        let (names, warnings) = parse("sandboxes = [\"b\", \"a\", \"b\", \"claude\"]\n");
        assert_eq!(
            names,
            vec!["a".to_string(), "b".to_string(), "claude".to_string()]
        );
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn parse_drops_blank_entries() {
        let (names, warnings) = parse("sandboxes = [\"claude\", \"\", \"   \"]\n");
        assert_eq!(names, vec!["claude".to_string()]);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn parse_malformed_document_warns_with_empty_list() {
        let (names, warnings) = parse("kein = [toml");
        assert!(names.is_empty());
        assert!(!warnings.is_empty(), "a malformed file must warn");
    }

    #[test]
    fn parse_empty_file_is_empty_no_warnings() {
        let (names, warnings) = parse("");
        assert!(names.is_empty());
        assert!(warnings.is_empty());
    }

    #[test]
    fn serialize_produces_the_document_format() {
        let text = serialize(&["claude".to_string(), "devin".to_string()]);
        assert!(text.contains("sandboxes"), "{text}");
        // Round-trips through parse.
        let (names, warnings) = parse(&text);
        assert_eq!(names, vec!["claude".to_string(), "devin".to_string()]);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn load_missing_file_is_empty_no_warnings() {
        let (names, warnings) = load(Path::new("/nonexistent/msb-tui/autostart.toml"));
        assert!(names.is_empty());
        assert!(warnings.is_empty());
    }

    #[test]
    fn load_reads_saved_document() {
        let dir = temp_dir("load");
        let path = dir.join("nested/autostart.toml");
        save(&path, &["devin".into(), "claude".into()]).unwrap();
        let (names, warnings) = load(&path);
        assert_eq!(names, vec!["claude".to_string(), "devin".to_string()]);
        assert!(warnings.is_empty(), "{warnings:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_malformed_file_warns_with_empty_list() {
        let dir = temp_dir("malformed");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("autostart.toml");
        std::fs::write(&path, "sandboxes = 5").unwrap();
        let (names, warnings) = load(&path);
        assert!(names.is_empty());
        assert!(!warnings.is_empty(), "{warnings:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn boot_hint_only_when_marks_wait_without_install() {
        let hint = hint_text(3, false).expect("hint when marks wait");
        assert!(hint.contains("3"), "{hint}");
        assert!(hint.contains("msb-tui autostart install"), "{hint}");
        assert!(hint_text(0, false).is_none(), "nothing marked → no nag");
        assert!(hint_text(0, true).is_none());
        assert!(hint_text(3, true).is_none(), "installed → no nag");
    }

    // ---- Cycle 3: the systemd unit ----

    #[test]
    fn unit_contents_runs_the_restore_pass_at_boot() {
        let text = unit_contents(Path::new("/usr/local/bin/msb-tui"));
        assert!(text.contains("[Unit]"), "{text}");
        assert!(text.contains("[Service]"), "{text}");
        // The pass runs headless against the same binary.
        let exec_line = text
            .lines()
            .find(|l| l.starts_with("ExecStart="))
            .expect("ExecStart line");
        assert!(exec_line.contains("/usr/local/bin/msb-tui"), "{exec_line}");
        assert!(exec_line.ends_with("autostart"), "{exec_line}");
        // Boot wiring + retry safety net.
        assert!(text.contains("WantedBy=default.target"), "{text}");
        assert!(text.contains("network-online"), "{text}");
        assert!(text.contains("Restart=on-failure"), "{text}");
        // A oneshot cannot carry Restart= on older systemd.
        assert!(!text.contains("Type=oneshot"), "{text}");
    }

    #[test]
    fn unit_path_sits_in_systemd_user_dir() {
        let p = unit_path_at(Path::new("/cfg"));
        assert_eq!(
            p,
            Path::new("/cfg/systemd/user/msb-tui-autostart.service"),
            "{p:?}"
        );
    }

    #[test]
    fn is_installed_check_is_file_existence() {
        let dir = temp_dir("installed");
        assert!(!is_installed_at(&dir), "missing file is not installed");
        std::fs::create_dir_all(unit_path_at(&dir).parent().unwrap()).unwrap();
        std::fs::write(unit_path_at(&dir), "irrelevant").unwrap();
        assert!(is_installed_at(&dir));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn write_unit_creates_dirs_and_writes_the_unit() {
        let dir = temp_dir("write-unit");
        let exe = dir.join("bin/msb-tui");
        let unit = write_unit_at(&dir, &exe).unwrap();
        assert_eq!(unit, unit_path_at(&dir));
        let text = std::fs::read_to_string(&unit).unwrap();
        assert!(
            text.contains(&exe.display().to_string()),
            "unit names the exe: {text}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
