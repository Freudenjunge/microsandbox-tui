//! Central application state and the key → action state machine.
//!
//! [`App`] owns everything the renderer needs (current view, sandbox list,
//! live metrics, selection, last error) plus the pure transition logic that
//! maps an [`AppEvent`] to an [`Action`]. Keeping the transitions here — and
//! free of any I/O — makes them unit-testable without a terminal.
//!
//! The `tokio::select!` event loop that drives this state lives in Task 10
//! (`src/main.rs`), which also owns rendering (Task 5) and the `msb` pollers.
//!
//! NOTE(dead_code): the runtime caller (main loop) lands in Task 10. Remove
//! this allow once it does.

#![allow(dead_code)]

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::event::Action;
use crate::event::AppEvent;
use crate::models::{Metrics, SandboxSummary};

/// Which screen is currently active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// Sandbox card dashboard (default).
    Dashboard,
    /// Create-sandbox form.
    Create,
    /// Port forwards view.
    Ports,
    /// Logs panel.
    Logs,
    /// Keybindings overlay.
    Help,
    /// Sandbox detail view.
    Inspect,
}

impl View {
    /// Whether this is the default (top-level) view.
    pub fn is_dashboard(self) -> bool {
        matches!(self, View::Dashboard)
    }
}

/// The central application state.
#[derive(Debug)]
pub struct App {
    /// Currently active screen.
    pub view: View,
    /// Latest sandbox list from `msb ls`.
    pub sandboxes: Vec<SandboxSummary>,
    /// Latest metrics samples, keyed by sandbox name.
    pub metrics: HashMap<String, Metrics>,
    /// Index into [`App::sandboxes`] of the highlighted card.
    pub selected: usize,
    /// Last user-facing error, shown on the status line.
    pub error: Option<String>,
    /// Set when the user asked to quit.
    pub quit: bool,
    /// Transient status message (spinner text, last action result).
    pub status: Option<String>,
}

impl App {
    /// Initialize empty state on the dashboard.
    pub fn new() -> Self {
        Self {
            view: View::Dashboard,
            sandboxes: Vec::new(),
            metrics: HashMap::new(),
            selected: 0,
            error: None,
            quit: false,
            status: None,
        }
    }

    /// Process an event and return what the main loop should do next.
    pub fn handle_event(&mut self, event: AppEvent) -> Action {
        match event {
            AppEvent::Key(key) => self.handle_key(key),
            AppEvent::Tick => Action::Continue,
            AppEvent::SandboxesUpdated(list) => {
                self.update_sandboxes(list);
                Action::Render
            }
            AppEvent::MetricsUpdated(samples) => {
                self.update_metrics(samples);
                Action::Render
            }
            AppEvent::Error(msg) => {
                self.error = Some(msg);
                Action::Render
            }
        }
    }

    /// Key handling for the current view.
    fn handle_key(&mut self, key: KeyEvent) -> Action {
        // `q` and Ctrl-C quit from the dashboard. Sub-views reserve their own
        // key space (form text entry) and exit with `Esc`.
        if self.view.is_dashboard() {
            return match key.code {
                KeyCode::Char('q') => {
                    self.quit = true;
                    Action::Quit
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.quit = true;
                    Action::Quit
                }
                KeyCode::Up => {
                    self.select_prev();
                    Action::Render
                }
                KeyCode::Down => {
                    self.select_next();
                    Action::Render
                }
                KeyCode::Char('c') => {
                    self.view = View::Create;
                    Action::Render
                }
                KeyCode::Enter => self.open_view(View::Inspect),
                KeyCode::Char('l') => self.open_view(View::Logs),
                KeyCode::Char('p') => self.open_view(View::Ports),
                KeyCode::Char('?') => {
                    self.view = View::Help;
                    Action::Render
                }
                KeyCode::Esc => Action::Continue,
                _ => Action::Continue,
            };
        }

        // Sub-views: `Esc` returns to the dashboard, `q` quits globally.
        match key.code {
            KeyCode::Esc => {
                self.view = View::Dashboard;
                Action::Render
            }
            KeyCode::Char('q') => {
                self.quit = true;
                Action::Quit
            }
            _ => Action::Continue,
        }
    }

    /// Switch to `view` only when there is a selected sandbox to act on.
    ///
    /// Inspect/logs/ports are all per-sandbox views, so with an empty list we
    /// stay put rather than render an empty detail screen.
    fn open_view(&mut self, view: View) -> Action {
        if self.selected_sandbox().is_some() {
            self.view = view;
            Action::Render
        } else {
            Action::Continue
        }
    }

    /// Move the selection up one card (clamped at the top).
    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Move the selection down one card (clamped at the bottom).
    pub fn select_next(&mut self) {
        if self.selected + 1 < self.sandboxes.len() {
            self.selected += 1;
        }
    }

    /// Replace the sandbox list, keeping the selected sandbox highlighted.
    ///
    /// Selection follows the sandbox *name* so reordering or removal by the
    /// poller does not jump the highlight to a different card. If the selected
    /// sandbox disappeared, the index is clamped to the new length.
    pub fn update_sandboxes(&mut self, list: Vec<SandboxSummary>) {
        let previous = self.selected_sandbox().map(|s| s.name.clone());
        self.sandboxes = list;
        self.selected = match previous {
            Some(name) => self
                .sandboxes
                .iter()
                .position(|s| s.name == name)
                .unwrap_or_else(|| self.selected.min(self.sandboxes.len().saturating_sub(1))),
            None => self.selected.min(self.sandboxes.len().saturating_sub(1)),
        };
    }

    /// Replace the metrics map, keyed by sandbox name.
    pub fn update_metrics(&mut self, samples: Vec<Metrics>) {
        self.metrics = samples.into_iter().map(|m| (m.name.clone(), m)).collect();
    }

    /// The currently highlighted sandbox, if any.
    pub fn selected_sandbox(&self) -> Option<&SandboxSummary> {
        self.sandboxes.get(self.selected)
    }

    /// Metrics for the currently highlighted sandbox, if available.
    pub fn selected_metrics(&self) -> Option<&Metrics> {
        let name = self.selected_sandbox()?.name.as_str();
        self.metrics.get(name)
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SandboxState;
    use chrono::{DateTime, Utc};

    fn summary(name: &str) -> SandboxSummary {
        SandboxSummary {
            created_at: DateTime::<Utc>::UNIX_EPOCH,
            image: "alpine".into(),
            name: name.into(),
            status: SandboxState::Running,
        }
    }

    fn sample_metrics(name: &str) -> Metrics {
        let mut m: Metrics =
            serde_json::from_str::<Vec<Metrics>>(include_str!("fixtures/metrics.json"))
                .unwrap()
                .into_iter()
                .next()
                .unwrap();
        m.name = name.into();
        m
    }

    fn key(code: KeyCode) -> AppEvent {
        AppEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn new_starts_empty_on_dashboard() {
        let app = App::new();
        assert_eq!(app.view, View::Dashboard);
        assert!(app.sandboxes.is_empty());
        assert!(app.metrics.is_empty());
        assert_eq!(app.selected, 0);
        assert!(app.error.is_none());
        assert!(!app.quit);
        assert!(app.selected_sandbox().is_none());
    }

    #[test]
    fn selection_clamps_at_both_ends() {
        let mut app = App::new();
        app.update_sandboxes(vec![summary("a"), summary("b"), summary("c")]);

        // Up at the top stays put.
        app.select_prev();
        assert_eq!(app.selected, 0);

        // Down advances, and clamps at the last card.
        app.select_next();
        app.select_next();
        assert_eq!(app.selected, 2);
        app.select_next();
        assert_eq!(app.selected, 2);
    }

    #[test]
    fn arrow_keys_drive_selection() {
        let mut app = App::new();
        app.update_sandboxes(vec![summary("a"), summary("b")]);

        assert_eq!(app.handle_event(key(KeyCode::Down)), Action::Render);
        assert_eq!(app.selected, 1);
        assert_eq!(app.handle_event(key(KeyCode::Up)), Action::Render);
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn q_quits_from_dashboard() {
        let mut app = App::new();
        assert_eq!(app.handle_event(key(KeyCode::Char('q'))), Action::Quit);
        assert!(app.quit);
    }

    #[test]
    fn c_opens_create_view() {
        let mut app = App::new();
        assert_eq!(app.handle_event(key(KeyCode::Char('c'))), Action::Render);
        assert_eq!(app.view, View::Create);
    }

    #[test]
    fn question_mark_opens_help_from_any_view() {
        let mut app = App::new();
        assert_eq!(app.handle_event(key(KeyCode::Char('?'))), Action::Render);
        assert_eq!(app.view, View::Help);

        // ...and help can still quit.
        assert_eq!(app.handle_event(key(KeyCode::Char('q'))), Action::Quit);
    }

    #[test]
    fn esc_returns_to_dashboard_from_subviews() {
        for view in [
            View::Create,
            View::Help,
            View::Inspect,
            View::Logs,
            View::Ports,
        ] {
            let mut app = App::new();
            app.update_sandboxes(vec![summary("a")]);
            app.view = view;
            assert_eq!(app.handle_event(key(KeyCode::Esc)), Action::Render);
            assert_eq!(app.view, View::Dashboard);
        }
    }

    #[test]
    fn esc_on_dashboard_is_a_noop() {
        let mut app = App::new();
        assert_eq!(app.handle_event(key(KeyCode::Esc)), Action::Continue);
        assert_eq!(app.view, View::Dashboard);
    }

    #[test]
    fn per_sandbox_views_require_a_selection() {
        let mut app = App::new();

        // Empty list: Enter/l/p stay put.
        assert_eq!(app.handle_event(key(KeyCode::Enter)), Action::Continue);
        assert_eq!(app.handle_event(key(KeyCode::Char('l'))), Action::Continue);
        assert_eq!(app.handle_event(key(KeyCode::Char('p'))), Action::Continue);
        assert_eq!(app.view, View::Dashboard);

        // With a sandbox present they open.
        app.update_sandboxes(vec![summary("a")]);
        assert_eq!(app.handle_event(key(KeyCode::Enter)), Action::Render);
        assert_eq!(app.view, View::Inspect);

        app.view = View::Dashboard;
        assert_eq!(app.handle_event(key(KeyCode::Char('l'))), Action::Render);
        assert_eq!(app.view, View::Logs);

        app.view = View::Dashboard;
        assert_eq!(app.handle_event(key(KeyCode::Char('p'))), Action::Render);
        assert_eq!(app.view, View::Ports);
    }

    #[test]
    fn update_sandboxes_preserves_selection_by_name() {
        let mut app = App::new();
        app.update_sandboxes(vec![summary("a"), summary("b"), summary("c")]);
        app.select_next(); // -> b
        assert_eq!(app.selected_sandbox().unwrap().name, "b");

        // Reordered list: selection follows "b" to its new index.
        app.update_sandboxes(vec![summary("c"), summary("b"), summary("a")]);
        assert_eq!(app.selected_sandbox().unwrap().name, "b");
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn update_sandboxes_clamps_when_selection_vanishes() {
        let mut app = App::new();
        app.update_sandboxes(vec![summary("a"), summary("b"), summary("c")]);
        app.select_next();
        app.select_next(); // -> c (last)

        // "c" is gone; stay in range instead of pointing past the end.
        app.update_sandboxes(vec![summary("a"), summary("b")]);
        assert_eq!(app.selected, 1);
        assert_eq!(app.selected_sandbox().unwrap().name, "b");

        // Empty list resets to the only valid index.
        app.update_sandboxes(vec![]);
        assert_eq!(app.selected, 0);
        assert!(app.selected_sandbox().is_none());
    }

    #[test]
    fn update_metrics_keys_by_name() {
        let mut app = App::new();
        app.update_metrics(vec![sample_metrics("a"), sample_metrics("b")]);
        assert_eq!(app.metrics.len(), 2);
        assert!(app.metrics.contains_key("a"));
        assert!(app.metrics.contains_key("b"));

        // Replacing the sample set replaces the map.
        app.update_metrics(vec![sample_metrics("b")]);
        assert_eq!(app.metrics.len(), 1);
        assert!(app.metrics.contains_key("b"));
        assert!(!app.metrics.contains_key("a"));
    }

    #[test]
    fn selected_metrics_looks_up_by_selected_name() {
        let mut app = App::new();
        app.update_sandboxes(vec![summary("a"), summary("b")]);
        app.update_metrics(vec![sample_metrics("b")]);
        assert!(app.selected_metrics().is_none());

        app.select_next(); // -> b
        assert_eq!(app.selected_metrics().unwrap().name, "b");
    }

    #[test]
    fn sandboxes_updated_event_replaces_list() {
        let mut app = App::new();
        let action = app.handle_event(AppEvent::SandboxesUpdated(vec![summary("x")]));
        assert_eq!(action, Action::Render);
        assert_eq!(app.sandboxes.len(), 1);
        assert_eq!(app.sandboxes[0].name, "x");
    }

    #[test]
    fn error_event_sets_status_line() {
        let mut app = App::new();
        let action = app.handle_event(AppEvent::Error("boom".into()));
        assert_eq!(action, Action::Render);
        assert_eq!(app.error.as_deref(), Some("boom"));
    }

    #[test]
    fn tick_is_a_noop() {
        let mut app = App::new();
        assert_eq!(app.handle_event(AppEvent::Tick), Action::Continue);
    }
}
