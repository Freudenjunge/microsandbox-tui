//! Parallel shell windows: `e` spawns a NEW terminal window with the
//! sandbox's interactive shell while the dashboard keeps running.
//!
//! The launcher list covers the maintainer's desktop (KDE/konsole) first,
//! then the common alternatives; the first launcher present on the machine
//! wins. Spawned windows are detached — their lifetime is independent of
//! the TUI process.

/// Spawn a terminal window running an interactive shell into `sandbox`.
pub fn spawn_shell_window(sandbox: &str) -> Result<(), String> {
    // The command inside the new window: the runtime's ssh bridge into the
    // sandbox (`msb ssh` never touches the TUI's own stdio).
    let inner = format!("msb ssh {sandbox}");

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

    #[test]
    fn which_exists_finds_sh() {
        assert!(which_exists("sh"), "sh must be on PATH in any test env");
        assert!(!which_exists("definitely-not-a-binary-xyz"));
    }
}
