//! Parallel shell windows: `e` spawns a NEW terminal window with the
//! sandbox's interactive shell while the dashboard keeps running.
//!
//! The launcher list covers the maintainer's desktop (KDE/konsole) first,
//! then the common alternatives; the first launcher present on the machine
//! wins. Spawned windows are detached — their lifetime is independent of
//! the TUI process.

/// Build the `msb ssh` argv for the sandbox's interactive shell.
///
/// Option C: if the sandbox's `runtime.shell` is `bash` (the create-form
/// toggle sets it), a plain shell request already runs a full-featured
/// shell. Otherwise the window wraps the request as `-- bash -l` (login
/// shell → `/etc/profile` + `~/.bashrc` → PS1 + tab completion), falling
/// back to bash being absent only at the guest level (`msb` exits with an
/// error in the window — visible, non-fatal).
pub fn shell_command_for(sandbox: &str, shell: Option<&str>) -> Vec<String> {
    let mut out = vec!["msb".to_string(), "ssh".to_string(), sandbox.to_string()];
    if shell.map(str::trim) != Some("bash") {
        out.extend(["--".to_string(), "bash".to_string(), "-l".to_string()]);
    }
    out
}

/// Spawn a terminal window running the given argv (terminal-emulator
/// launcher chain; the first launcher present on the machine wins).
pub fn spawn_window_argv(argv_inner: &[String]) -> Result<(), String> {
    let mut inner = argv_inner.join(" ");
    inner = format!("exec {inner}"); // replace sh so the window dies with the ssh session

    let candidates: Vec<Vec<String>> = vec![
        // KDE (maintainer desktop): konsole -e <cmd…>.
        vec![
            "konsole".into(),
            "-e".into(),
            "sh".into(),
            "-c".into(),
            inner.clone(),
        ],
        // Debian alternatives: x-terminal-emulator.
        vec![
            "x-terminal-emulator".into(),
            "-e".into(),
            "sh".into(),
            "-c".into(),
            inner.clone(),
        ],
        // GNOME: gnome-terminal -- <cmd…>.
        vec![
            "gnome-terminal".into(),
            "--".into(),
            "sh".into(),
            "-c".into(),
            inner.clone(),
        ],
        // Tiling/Minimal setups.
        vec![
            "alacritty".into(),
            "-e".into(),
            "sh".into(),
            "-c".into(),
            inner.clone(),
        ],
        vec!["foot".into(), "sh".into(), "-c".into(), inner],
    ];

    for argv in &candidates {
        let program = &argv[0];
        // Only try launchers that actually exist on this machine.
        if which_exists(program) {
            return std::process::Command::new(program)
                .args(&argv[1..])
                .spawn()
                .map(|_| ())
                .map_err(|e| format!("{program}: {e}"));
        }
    }
    Err("no terminal emulator found (tried konsole, x-terminal-emulator, gnome-terminal, alacritty, foot)".into())
}

/// Minimal PATH probe without pulling in the `which` crate.
fn which_exists(program: &str) -> bool {
    if program.contains('/') {
        return std::path::Path::new(program).exists();
    }
    std::env::var("PATH")
        .map(|paths| {
            paths
                .split(':')
                .any(|dir| std::path::Path::new(dir).join(program).exists())
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- shell window command ----

    #[test]
    fn shell_window_command_uses_bash_login() {
        // Option C: the spawned window runs `msb ssh <name> -- bash -l`
        // (login shell → /etc/profile + ~/.bashrc → PS1 + tab completion).
        assert_eq!(
            shell_command_for("web", None),
            vec![
                "msb".to_string(),
                "ssh".to_string(),
                "web".to_string(),
                "--".to_string(),
                "bash".to_string(),
                "-l".to_string()
            ]
        );
        // Shell override from the create form (`runtime.shell = bash`) makes
        // the plain shell request already full-featured: `msb ssh <name>`.
        assert_eq!(
            shell_command_for("web", Some("bash")),
            vec!["msb".to_string(), "ssh".to_string(), "web".to_string()]
        );
        // No shell override → login-shell wrapper.
        assert_eq!(
            shell_command_for("web", None),
            vec![
                "msb".to_string(),
                "ssh".to_string(),
                "web".to_string(),
                "--".to_string(),
                "bash".to_string(),
                "-l".to_string()
            ]
        );
        // Empty/blank shell string counts as "not set".
        assert_eq!(
            shell_command_for("web", Some("  ")),
            vec![
                "msb".to_string(),
                "ssh".to_string(),
                "web".to_string(),
                "--".to_string(),
                "bash".to_string(),
                "-l".to_string()
            ]
        );
    }

    #[test]
    fn which_exists_finds_sh() {
        assert!(which_exists("sh"), "sh must be on PATH in any test env");
        assert!(!which_exists("definitely-not-a-binary-xyz"));
    }
}
