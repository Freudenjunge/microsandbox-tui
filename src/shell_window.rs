//! Parallel shell windows: `x` spawns a NEW terminal window with the
//! sandbox's interactive shell while the dashboard keeps running.
//!
//! The window runs the CLI's `msb exec -t -- shell` (the agent exec path —
//! verified to deliver a working pty; `msb ssh -- <cmd>` does not carry
//! output over a pty, and the no-command exec would attach the image's
//! default workload, not a shell). The launcher list covers the
//! maintainer's desktop (KDE/konsole) first, then the common alternatives;
//! the first launcher present on the machine wins. Spawned windows are
//! detached — their lifetime is independent of the TUI process.
//!
//! The session prepends `$HOME/.local/bin` (guest-resolved) to the PATH so
//! pip/pipx-installed tools are runnable (issue #5), and prefers bash when
//! the guest has it (issue #4; probed by the caller, errors degrade to sh).

/// Build the command a spawned window runs: `msb exec -t <sandbox> -- sh -c
/// 'PATH=…; exec <shell>'` (PTY, PATH fixed, explicit shell).
pub fn inner_command(sandbox: &str, prefer_bash: bool) -> String {
    let sh = if prefer_bash { "bash" } else { "sh" };
    format!("msb exec -t {sandbox} -- sh -c 'PATH=$HOME/.local/bin:$PATH; exec {sh}'")
}

/// Spawn a terminal window running an interactive shell into `sandbox`.
pub fn spawn_shell_window(sandbox: &str, prefer_bash: bool) -> Result<(), String> {
    // The command inside the new window: the CLI's exec bridge into the
    // sandbox (detached from the TUI's own stdio).
    let inner = inner_command(sandbox, prefer_bash);

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
    fn inner_command_fixes_path_and_picks_the_shell() {
        let bash = inner_command("web", true);
        let sh = inner_command("web", false);
        for cmd in [&bash, &sh] {
            assert!(
                cmd.starts_with("msb exec -t web -- sh -c '"),
                "exec bridge form: {cmd}"
            );
            assert!(cmd.contains("PATH=$HOME/.local/bin:$PATH; "), "PATH: {cmd}");
        }
        assert!(bash.contains("exec bash'"), "bash: {bash}");
        assert!(sh.contains("exec sh'"), "sh: {sh}");
    }

    #[test]
    fn which_exists_finds_sh() {
        assert!(which_exists("sh"), "sh must be on PATH in any test env");
        assert!(!which_exists("definitely-not-a-binary-xyz"));
    }
}
