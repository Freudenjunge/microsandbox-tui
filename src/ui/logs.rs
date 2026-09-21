//! Logs panel: streaming log viewer for a sandbox.
//!
//! [`LogsState`] owns the buffer, filters, and scroll position. It is pure and
//! unit-tested; the render function only draws the current state.
//!
//! NOTE(dead_code): consumers arrive in Task 10.
#![allow(dead_code)]

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::backend::LogLine;
use crate::ui::theme::THEME;

/// Maximum number of log lines retained in memory.
const MAX_LINES: usize = 10_000;

/// Action returned by [`LogsState::handle_key`] for the app event loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogsAction {
    /// No state change requiring a redraw.
    Continue,
    /// Return to the dashboard.
    Back,
    /// Follow mode was toggled.
    ToggleFollow,
    /// User pressed `g` to enter grep input mode.
    StartGrep,
    /// User pressed Enter in grep mode (confirm filter).
    EndGrep,
}

/// State of the logs view: buffer, filters, scroll, and grep input.
#[derive(Debug)]
pub struct LogsState {
    /// Sandbox whose logs we are viewing.
    pub sandbox_name: String,
    /// Buffered log lines (capped at [`MAX_LINES`]).
    pub lines: Vec<LogLine>,
    /// Whether we are following the live tail.
    pub follow: bool,
    /// Offset from the bottom (0 = latest line at the bottom).
    pub scroll: usize,
    /// Active grep substring filter (case-insensitive).
    pub grep: Option<String>,
    /// Source filter: `stdout`, `stderr`, `system`, or `None` for all.
    pub source_filter: Option<String>,
    /// Whether to stick to the bottom as new lines arrive.
    pub auto_scroll: bool,
    /// Grep input buffer (active while the user is typing a pattern).
    pub grep_input: String,
    /// True while the user is typing a grep pattern.
    pub grep_mode: bool,
}

impl LogsState {
    /// Initialize with follow and auto_scroll enabled.
    pub fn new(sandbox_name: &str) -> Self {
        Self {
            sandbox_name: sandbox_name.to_string(),
            lines: Vec::new(),
            follow: true,
            scroll: 0,
            grep: None,
            source_filter: None,
            auto_scroll: true,
            grep_input: String::new(),
            grep_mode: false,
        }
    }

    /// Append a log line, enforcing the capacity limit and auto-scrolling.
    pub fn push_line(&mut self, line: LogLine) {
        self.lines.push(line);
        if self.lines.len() > MAX_LINES {
            let overflow = self.lines.len() - MAX_LINES;
            self.lines.drain(0..overflow);
        }
        if self.auto_scroll {
            self.scroll = 0;
        }
    }

    /// Toggle follow mode (and re-enable auto-scroll when following).
    pub fn toggle_follow(&mut self) {
        self.follow = !self.follow;
        if self.follow {
            self.auto_scroll = true;
            self.scroll = 0;
        }
    }

    /// Set the grep filter. `None` clears it.
    pub fn set_grep(&mut self, pattern: Option<String>) {
        self.grep = pattern.filter(|p| !p.is_empty());
    }

    /// Set the source filter. `None` shows all sources.
    pub fn set_source_filter(&mut self, source: Option<String>) {
        self.source_filter = source.filter(|s| !s.is_empty());
    }

    /// Scroll up (toward older lines) by `n` rows.
    pub fn scroll_up(&mut self, n: usize) {
        let max = self.lines.len().saturating_sub(1);
        self.scroll = (self.scroll + n).min(max);
        self.auto_scroll = false;
    }

    /// Scroll down (toward newer lines) by `n` rows.
    pub fn scroll_down(&mut self, n: usize) {
        self.scroll = self.scroll.saturating_sub(n);
        if self.scroll == 0 {
            self.auto_scroll = true;
        }
    }

    /// Cycle the source filter: all → stdout → stderr → system → all.
    pub fn cycle_source_filter(&mut self) {
        let next = match self.source_filter.as_deref() {
            None => Some("stdout"),
            Some("stdout") => Some("stderr"),
            Some("stderr") => Some("system"),
            Some(_) => None,
        };
        self.set_source_filter(next.map(str::to_string));
    }

    /// Lines matching the active grep and source filters, in order.
    pub fn visible_lines(&self) -> Vec<&LogLine> {
        let grep_lower = self.grep.as_ref().map(|g| g.to_lowercase());
        self.lines
            .iter()
            .filter(|line| {
                if let Some(src) = &self.source_filter
                    && line.source != *src
                {
                    return false;
                }
                if let Some(pat) = &grep_lower
                    && !line.data.to_lowercase().contains(pat)
                {
                    return false;
                }
                true
            })
            .collect()
    }

    /// Process a key event and return the action for the event loop.
    pub fn handle_key(&mut self, key: KeyEvent) -> LogsAction {
        if self.grep_mode {
            return self.handle_grep_key(key);
        }
        match key.code {
            KeyCode::Char('f') => {
                self.toggle_follow();
                LogsAction::ToggleFollow
            }
            KeyCode::Char('g') => {
                self.grep_mode = true;
                self.grep_input.clear();
                LogsAction::StartGrep
            }
            KeyCode::Char('s') => {
                self.cycle_source_filter();
                LogsAction::Continue
            }
            KeyCode::Up => {
                self.scroll_up(1);
                LogsAction::Continue
            }
            KeyCode::Down => {
                self.scroll_down(1);
                LogsAction::Continue
            }
            KeyCode::PageUp => {
                self.scroll_up(10);
                LogsAction::Continue
            }
            KeyCode::PageDown => {
                self.scroll_down(10);
                LogsAction::Continue
            }
            KeyCode::Esc => LogsAction::Back,
            KeyCode::Char('q') => LogsAction::Back,
            _ => LogsAction::Continue,
        }
    }

    /// Key handling while in grep input mode.
    fn handle_grep_key(&mut self, key: KeyEvent) -> LogsAction {
        match key.code {
            KeyCode::Enter => {
                self.set_grep(Some(self.grep_input.clone()));
                self.grep_mode = false;
                self.grep_input.clear();
                LogsAction::EndGrep
            }
            KeyCode::Esc => {
                self.grep_mode = false;
                self.grep_input.clear();
                LogsAction::Back
            }
            KeyCode::Backspace => {
                self.grep_input.pop();
                LogsAction::Continue
            }
            KeyCode::Char(c) => {
                self.grep_input.push(c);
                LogsAction::Continue
            }
            _ => LogsAction::Continue,
        }
    }
}

// ---------- rendering ----------

/// Render the logs view into `area`.
pub fn render_logs(frame: &mut Frame, state: &LogsState, area: Rect) {
    let mut constraints = vec![
        Constraint::Length(1), // title bar
        Constraint::Min(1),    // log lines
        Constraint::Length(1), // footer
    ];
    if state.grep_mode {
        constraints.insert(2, Constraint::Length(1)); // grep input
    }
    let chunks = Layout::vertical(constraints).split(area);

    render_title_bar(frame, state, chunks[0]);
    render_log_area(frame, state, chunks[1]);
    if state.grep_mode {
        render_grep_input(frame, state, chunks[2]);
        render_footer(frame, state, chunks[3]);
    } else {
        render_footer(frame, state, chunks[2]);
    }
}

/// Title bar: `Logs: <name> (following)` or `(paused)`.
fn render_title_bar(frame: &mut Frame, state: &LogsState, area: Rect) {
    let t = &THEME;
    let status_text = if state.follow { "following" } else { "paused" };
    let status_color = if state.follow { t.ok } else { t.warn };
    let line = Line::from(vec![
        Span::styled(
            format!(" Logs: {}", state.sandbox_name),
            Style::default().fg(t.fg).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            format!("({status_text})"),
            Style::default().fg(status_color),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

/// Map a log source to a display color.
fn source_color(source: &str) -> Color {
    let t = &THEME;
    match source {
        "stderr" => t.err,
        "system" => t.warn,
        "output" => t.accent,
        _ => t.fg,
    }
}

/// Render the scrollable log line area.
fn render_log_area(frame: &mut Frame, state: &LogsState, area: Rect) {
    let t = &THEME;
    let visible = state.visible_lines();
    let height = area.height as usize;
    let total = visible.len();

    // Determine which slice of lines to display. `scroll` is an offset from the
    // bottom (0 = latest). We show the last `height` lines minus the scroll.
    let end = total.saturating_sub(state.scroll);
    let start = end.saturating_sub(height);
    let window = &visible[start.min(total)..end.min(total)];

    let lines: Vec<Line> = window
        .iter()
        .map(|line| {
            let ts = line.timestamp.format("%Y-%m-%d %H:%M:%S").to_string();
            Line::from(vec![
                Span::styled(format!("[{ts}] "), Style::default().fg(t.muted)),
                Span::styled(
                    line.data.clone(),
                    Style::default().fg(source_color(&line.source)),
                ),
            ])
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.muted));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let para = Paragraph::new(lines);
    frame.render_widget(para, inner);
}

/// Grep input line shown while the user is typing a pattern.
fn render_grep_input(frame: &mut Frame, state: &LogsState, area: Rect) {
    let t = &THEME;
    let line = Line::from(vec![
        Span::styled(" grep: ", Style::default().fg(t.accent)),
        Span::styled(state.grep_input.clone(), Style::default().fg(t.fg)),
        Span::styled(
            "▌",
            Style::default()
                .fg(t.accent)
                .add_modifier(Modifier::SLOW_BLINK),
        ),
        Span::styled(
            "  [Enter] confirm  [Esc] cancel",
            Style::default().fg(t.muted),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

/// Footer keybinding hints + auto-scroll indicator.
fn render_footer(frame: &mut Frame, state: &LogsState, area: Rect) {
    let t = &THEME;
    let footer_hints = "[f] follow  [g] grep  [s] source  [Esc] back";
    let scroll_indicator = if state.auto_scroll {
        "↓ auto".to_string()
    } else {
        format!("↑ {} from bottom", state.scroll)
    };
    let line = Line::from(vec![
        Span::styled(footer_hints, Style::default().fg(t.muted)),
        Span::raw(" "),
        Span::styled(scroll_indicator, Style::default().fg(t.text)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

// ---------- tests ----------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use crossterm::event::KeyModifiers;

    fn log_line(source: &str, data: &str, id: u64) -> LogLine {
        LogLine {
            id,
            source: source.to_string(),
            data: data.to_string(),
            timestamp: DateTime::<Utc>::UNIX_EPOCH,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn new_has_sensible_defaults() {
        let state = LogsState::new("my-app");
        assert_eq!(state.sandbox_name, "my-app");
        assert!(state.follow);
        assert!(state.auto_scroll);
        assert_eq!(state.scroll, 0);
        assert!(state.grep.is_none());
        assert!(state.source_filter.is_none());
        assert!(state.lines.is_empty());
    }

    #[test]
    fn push_line_adds_and_auto_scrolls() {
        let mut state = LogsState::new("app");
        state.push_line(log_line("stdout", "hello", 1));
        state.push_line(log_line("stdout", "world", 2));
        assert_eq!(state.lines.len(), 2);
        assert_eq!(state.scroll, 0, "auto-scroll keeps scroll at 0");
    }

    #[test]
    fn capacity_limit_drops_oldest() {
        let mut state = LogsState::new("app");
        for i in 0..(MAX_LINES + 1) {
            state.push_line(log_line("stdout", &format!("line {i}"), i as u64));
        }
        assert_eq!(state.lines.len(), MAX_LINES);
        // Oldest line ("line 0") should be dropped; "line 1" is now first.
        assert_eq!(state.lines.first().unwrap().data, "line 1");
        assert_eq!(
            state.lines.last().unwrap().data,
            format!("line {}", MAX_LINES)
        );
    }

    #[test]
    fn scroll_up_adjusts_offset() {
        let mut state = LogsState::new("app");
        for i in 0..50 {
            state.push_line(log_line("stdout", &format!("line {i}"), i));
        }
        state.scroll_up(5);
        assert_eq!(state.scroll, 5);
        assert!(!state.auto_scroll, "scrolling up disables auto-scroll");
    }

    #[test]
    fn scroll_down_adjusts_offset() {
        let mut state = LogsState::new("app");
        for i in 0..50 {
            state.push_line(log_line("stdout", &format!("line {i}"), i));
        }
        state.scroll_up(10);
        assert_eq!(state.scroll, 10);
        state.scroll_down(3);
        assert_eq!(state.scroll, 7);
    }

    #[test]
    fn scroll_down_to_bottom_reenables_auto_scroll() {
        let mut state = LogsState::new("app");
        for i in 0..10 {
            state.push_line(log_line("stdout", &format!("line {i}"), i));
        }
        state.scroll_up(5);
        assert!(!state.auto_scroll);
        state.scroll_down(5);
        assert_eq!(state.scroll, 0);
        assert!(state.auto_scroll, "reaching bottom re-enables auto-scroll");
    }

    #[test]
    fn grep_filter_is_case_insensitive_substring() {
        let mut state = LogsState::new("app");
        state.push_line(log_line("stdout", "Starting server", 1));
        state.push_line(log_line("stdout", "ERROR: connection refused", 2));
        state.push_line(log_line("stderr", "error: timeout", 3));

        state.set_grep(Some("error".to_string()));
        let visible = state.visible_lines();
        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].data, "ERROR: connection refused");
        assert_eq!(visible[1].data, "error: timeout");
    }

    #[test]
    fn source_filter_restricts_lines() {
        let mut state = LogsState::new("app");
        state.push_line(log_line("stdout", "out1", 1));
        state.push_line(log_line("stderr", "err1", 2));
        state.push_line(log_line("system", "sys1", 3));

        state.set_source_filter(Some("stderr".to_string()));
        let visible = state.visible_lines();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].data, "err1");
    }

    #[test]
    fn visible_lines_respects_both_filters() {
        let mut state = LogsState::new("app");
        state.push_line(log_line("stdout", "INFO: started", 1));
        state.push_line(log_line("stderr", "ERROR: timeout", 2));
        state.push_line(log_line("stderr", "INFO: retrying", 3));
        state.push_line(log_line("system", "ERROR: reboot", 4));

        state.set_grep(Some("error".to_string()));
        state.set_source_filter(Some("stderr".to_string()));
        let visible = state.visible_lines();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].data, "ERROR: timeout");
    }

    #[test]
    fn empty_grep_pattern_shows_all() {
        let mut state = LogsState::new("app");
        state.push_line(log_line("stdout", "a", 1));
        state.push_line(log_line("stdout", "b", 2));
        state.set_grep(Some(String::new()));
        assert!(state.grep.is_none(), "empty pattern clears grep");
        assert_eq!(state.visible_lines().len(), 2);
    }

    #[test]
    fn toggle_follow_reenables_auto_scroll() {
        let mut state = LogsState::new("app");
        for i in 0..10 {
            state.push_line(log_line("stdout", &format!("line {i}"), i));
        }
        assert!(state.follow, "follow starts true");

        // Toggle off — follow disabled, auto-scroll untouched yet.
        state.toggle_follow();
        assert!(!state.follow);

        // Scroll up to move away from the bottom.
        state.scroll_up(3);
        assert!(!state.auto_scroll);

        // Toggle back on — follow re-enabled, auto-scroll re-enabled, scroll reset.
        state.toggle_follow();
        assert!(state.follow);
        assert!(state.auto_scroll);
        assert_eq!(state.scroll, 0);
    }

    #[test]
    fn cycle_source_filter_cycles_all_stdout_stderr_system() {
        let mut state = LogsState::new("app");
        assert!(state.source_filter.is_none());

        state.cycle_source_filter();
        assert_eq!(state.source_filter.as_deref(), Some("stdout"));

        state.cycle_source_filter();
        assert_eq!(state.source_filter.as_deref(), Some("stderr"));

        state.cycle_source_filter();
        assert_eq!(state.source_filter.as_deref(), Some("system"));

        state.cycle_source_filter();
        assert!(state.source_filter.is_none(), "cycles back to all");
    }

    #[test]
    fn handle_key_f_toggles_follow() {
        let mut state = LogsState::new("app");
        assert!(state.follow);
        let action = state.handle_key(key(KeyCode::Char('f')));
        assert_eq!(action, LogsAction::ToggleFollow);
        assert!(!state.follow);
    }

    #[test]
    fn handle_key_g_enters_grep_mode() {
        let mut state = LogsState::new("app");
        let action = state.handle_key(key(KeyCode::Char('g')));
        assert_eq!(action, LogsAction::StartGrep);
        assert!(state.grep_mode);
    }

    #[test]
    fn handle_key_esc_returns_back() {
        let mut state = LogsState::new("app");
        assert_eq!(state.handle_key(key(KeyCode::Esc)), LogsAction::Back);
    }

    #[test]
    fn handle_key_q_returns_back() {
        let mut state = LogsState::new("app");
        assert_eq!(state.handle_key(key(KeyCode::Char('q'))), LogsAction::Back);
    }

    #[test]
    fn grep_mode_enter_confirms_and_esc_cancels() {
        let mut state = LogsState::new("app");
        state.push_line(log_line("stdout", "hello world", 1));

        // Enter grep mode and type a pattern.
        state.handle_key(key(KeyCode::Char('g')));
        state.handle_key(key(KeyCode::Char('h')));
        state.handle_key(key(KeyCode::Char('i')));
        assert!(state.grep_mode);

        // Enter confirms.
        let action = state.handle_key(key(KeyCode::Enter));
        assert_eq!(action, LogsAction::EndGrep);
        assert!(!state.grep_mode);
        assert_eq!(state.grep.as_deref(), Some("hi"));
    }

    #[test]
    fn grep_mode_esc_cancels_without_setting_filter() {
        let mut state = LogsState::new("app");
        state.handle_key(key(KeyCode::Char('g')));
        state.handle_key(key(KeyCode::Char('x')));
        let action = state.handle_key(key(KeyCode::Esc));
        assert_eq!(action, LogsAction::Back);
        assert!(!state.grep_mode);
        assert!(state.grep.is_none(), "Esc cancels — no filter set");
    }

    #[test]
    fn arrow_keys_scroll_and_disable_auto_scroll() {
        let mut state = LogsState::new("app");
        for i in 0..20 {
            state.push_line(log_line("stdout", &format!("line {i}"), i));
        }
        assert!(state.auto_scroll);

        state.handle_key(key(KeyCode::Up));
        assert!(!state.auto_scroll);
        assert_eq!(state.scroll, 1);

        state.handle_key(key(KeyCode::PageUp));
        assert_eq!(state.scroll, 11);

        // PageDown back toward bottom.
        state.handle_key(key(KeyCode::PageDown));
        assert_eq!(state.scroll, 1);
    }

    #[test]
    fn push_line_respects_auto_scroll_flag() {
        let mut state = LogsState::new("app");
        // Add several lines so scroll has room.
        for i in 0..5 {
            state.push_line(log_line("stdout", &format!("line {i}"), i));
        }
        state.auto_scroll = false;
        state.scroll_up(2);
        assert_eq!(state.scroll, 2);

        // New line arrives while auto_scroll is off — scroll stays.
        state.push_line(log_line("stdout", "new", 99));
        assert_eq!(state.scroll, 2, "auto_scroll off keeps view position");
    }
}
