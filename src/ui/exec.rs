//! EXEC detail tab: captured command exec + interactive shell hand-off.
//!
//! Docker-Sandbox model (maintainer decision 2.9): type a command, `Enter`
//! runs it via the backend's captured exec and appends the output in-tab;
//! `S` suspends the TUI and opens a full interactive shell in the
//! foreground terminal (the "open in new window" equivalent — implemented
//! by handing the terminal to the sandbox shell process, see
//! [`App::suspend_for_shell`] wiring in the main loop).
//!
//! State is pure and unit-tested; the main loop owns the actual exec calls.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::ui::theme::THEME;

/// One executed command and its captured output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecEntry {
    /// The command line as typed.
    pub cmd: String,
    /// Captured stdout (may be empty).
    pub stdout: String,
    /// Captured stderr (may be empty).
    pub stderr: String,
    /// Process exit code.
    pub exit_code: i32,
}

/// State of the EXEC tab for one sandbox.
#[derive(Debug, Clone, Default)]
pub struct ExecState {
    /// Current input buffer.
    pub input: String,
    /// Completed runs, oldest first (rendered as a scrollback).
    pub history: Vec<ExecEntry>,
    /// True while a captured exec is in flight (spinner / no echo).
    pub running: bool,
}

/// Maximum kept exec entries per sandbox.
pub const MAX_HISTORY: usize = 200;

impl ExecState {
    /// Take the trimmed command line for submission (clears the input).
    /// Called by the app AFTER `handle_key` reported Enter.
    pub fn take_command(&mut self) -> Option<String> {
        let cmd = self.input.trim().to_string();
        if cmd.is_empty() {
            return None;
        }
        self.input.clear();
        Some(cmd)
    }

    /// Route a key into the input. Returns `true` on Enter with a
    /// non-empty command (caller then calls [`Self::take_command`]).
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Enter => {
                return !self.input.trim().is_empty();
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                self.input.push(c);
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            _ => {}
        }
        false
    }

    /// Record a finished exec run.
    pub fn push_result(&mut self, cmd: String, stdout: String, stderr: String, exit_code: i32) {
        self.history.push(ExecEntry {
            cmd,
            stdout,
            stderr,
            exit_code,
        });
        let overflow = self.history.len().saturating_sub(MAX_HISTORY);
        if overflow > 0 {
            self.history.drain(0..overflow);
        }
        self.running = false;
    }
}

/// EXEC detail tab body: history scrollback + input line.
pub fn render_exec_body(frame: &mut Frame, state: &ExecState, area: Rect) {
    let t = &THEME;
    let rows = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(area);

    // History scrollback: `cmd`, then `  ↳ out` lines, exit code marker.
    let mut lines: Vec<Line> = Vec::new();
    if state.history.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No commands yet — type one below and press Enter.",
            Style::default().fg(t.muted),
        )));
    }
    for entry in &state.history {
        let marker = if entry.exit_code == 0 {
            Span::styled(" ✓", Style::default().fg(t.ok))
        } else {
            Span::styled(
                format!(" ✗ {}", entry.exit_code),
                Style::default().fg(t.err),
            )
        };
        lines.push(Line::from(vec![
            Span::styled(
                " $ ".to_string(),
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(entry.cmd.clone(), Style::default().fg(t.fg)),
            marker,
        ]));
        if !entry.stdout.trim().is_empty() {
            for l in entry.stdout.trim_end().lines().take(20) {
                lines.push(Line::from(Span::styled(
                    format!("   {l}"),
                    Style::default().fg(t.text),
                )));
            }
        }
        if !entry.stderr.trim().is_empty() {
            for l in entry.stderr.trim_end().lines().take(10) {
                lines.push(Line::from(Span::styled(
                    format!("   {l}"),
                    Style::default().fg(t.err),
                )));
            }
        }
    }
    frame.render_widget(
        Paragraph::new(lines).scroll((
            state.history.len().saturating_sub(rows[0].height as usize) as u16,
            0,
        )),
        rows[0],
    );

    // Input line.
    let input = Line::from(vec![
        Span::styled(
            " $ ",
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(state.input.clone(), Style::default().fg(t.fg)),
        Span::styled("█", Style::default().fg(t.accent)),
        Span::styled(
            "  [Enter] run  [S] interactive shell",
            Style::default().fg(t.muted),
        ),
    ]);
    frame.render_widget(Paragraph::new(input), rows[1]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn typing_builds_input_and_enter_submits() {
        let mut st = ExecState::default();
        for c in "ls -la /".chars() {
            assert!(!st.handle_key(key(KeyCode::Char(c))));
        }
        assert_eq!(st.input, "ls -la /");
        assert!(st.handle_key(key(KeyCode::Enter)), "Enter signals submit");
        assert_eq!(st.take_command().as_deref(), Some("ls -la /"));
        assert!(st.input.is_empty(), "take_command clears the input");
        assert_eq!(st.take_command(), None, "empty input submits nothing");
    }

    #[test]
    fn enter_with_empty_input_does_not_submit() {
        let mut st = ExecState::default();
        assert!(!st.handle_key(key(KeyCode::Enter)));
        st.input = "   ".into();
        assert!(!st.handle_key(key(KeyCode::Enter)));
    }

    #[test]
    fn backspace_edits() {
        let mut st = ExecState::default();
        for c in "lss".chars() {
            st.handle_key(key(KeyCode::Char(c)));
        }
        st.handle_key(key(KeyCode::Backspace));
        assert_eq!(st.input, "ls");
    }

    #[test]
    fn history_appends_and_is_bounded() {
        let mut st = ExecState::default();
        for i in 0..(MAX_HISTORY + 5) {
            st.push_result(format!("cmd {i}"), format!("out {i}"), String::new(), 0);
            assert!(!st.running);
        }
        assert_eq!(st.history.len(), MAX_HISTORY);
        assert_eq!(st.history[0].cmd, "cmd 5", "oldest entries dropped");
        assert_eq!(st.history.last().unwrap().exit_code, 0);
    }
}
