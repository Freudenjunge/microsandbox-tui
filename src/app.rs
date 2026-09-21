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
use crate::models::{Metrics, PublishedPort, SandboxSummary};
use crate::ui::create::CreateForm;
use crate::ui::logs::LogsState;
use crate::ui::ports::PortsState;

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

/// A lifecycle/exec operation awaiting confirmation or execution.
///
/// Destructive ops (`x` stop, `Del` remove, `r` restart) require the user to
/// confirm; the main loop resolves queued ops by running the corresponding
/// action on the backend.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// Start the named sandbox.
    Start(String),
    /// Gracefully stop the named sandbox.
    Stop(String),
    /// Restart the named sandbox.
    Restart(String),
    /// Stop + remove the named sandbox.
    Remove(String),
    /// Run a command in the named sandbox.
    Exec { name: String, cmd: Vec<String> },
    /// Publish a port on the named sandbox (recreate flow).
    PublishPort { name: String, port: PublishedPort },
    /// Unpublish a port from the named sandbox (recreate flow).
    UnpublishPort { name: String, port: PublishedPort },
    /// Install or update the host microsandbox runtime to the SDK version.
    InstallRuntime,
}

impl Op {
    /// Human-readable description shown in the confirmation dialog.
    pub fn describe(&self) -> String {
        match self {
            Op::Start(n) => format!("Start sandbox '{n}'?"),
            Op::Stop(n) => format!("Stop sandbox '{n}'?"),
            Op::Restart(n) => format!("Restart sandbox '{n}'?"),
            Op::Remove(n) => format!("REMOVE sandbox '{n}'? (rootfs is deleted)"),
            Op::Exec { name, cmd } => format!("Run in '{name}': {}", cmd.join(" ")),
            Op::PublishPort { name, port } => {
                format!(
                    "Publish {}:{}→{}/{} on '{name}'? (recreates the sandbox; rootfs resets)",
                    port.host_bind, port.host_port, port.guest_port, port.protocol
                )
            }
            Op::UnpublishPort { name, port } => {
                format!(
                    "Unpublish {}:{} from '{name}'? (recreates the sandbox; rootfs resets)",
                    port.host_port, port.guest_port
                )
            }
            Op::InstallRuntime => {
                format!(
                    "Install/update microsandbox runtime to v{}? (downloads the official bundle)",
                    crate::runtime::sdk_version()
                )
            }
        }
    }

    /// Whether this op must be confirmed before running.
    pub fn requires_confirmation(&self) -> bool {
        !matches!(self, Op::Exec { .. })
    }
}

/// A queued, confirmed operation waiting for the main loop to run.
#[derive(Debug)]
pub struct PendingOp {
    /// The operation to perform.
    pub op: Op,
}

/// Maximum lines kept for the dashboard sidebar log preview.
const PREVIEW_MAX_LINES: usize = 6;

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
    /// Operation awaiting confirmation (`y`/`n`), if any.
    pub confirm: Option<Op>,
    /// Confirmed op queued for the main loop; consumed via [`App::take_op`].
    pub queued_op: Option<Op>,
    /// Validated create spec queued by the form, consumed via
    /// [`App::take_create_spec`].
    pub queued_create: Option<crate::backend::CreateSpec>,
    /// Create-form state (lives across renders while the form is open).
    pub create_form: Option<CreateForm>,
    /// Logs-view state; `Some` while the logs view is open.
    pub logs_state: Option<LogsState>,
    /// Bounded log preview for the selected sandbox on the dashboard
    /// sidebar (fed by the same tail task as the logs view).
    pub preview_lines: Vec<crate::backend::LogLine>,
    /// Sandbox name the preview buffer belongs to.
    pub preview_for: Option<String>,
    /// Ports-view state; `Some` while the ports view is open.
    pub ports_state: Option<PortsState>,
    /// Cached image references for the create form autocomplete.
    pub images: Vec<String>,
    /// Published ports per sandbox (dashboard cards + ports view), refreshed
    /// lazily when the ports view is opened.
    pub ports: std::collections::HashMap<String, Vec<crate::models::PublishedPort>>,
    /// True while a long-running `msb` operation is in flight.
    pub busy: bool,
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
            confirm: None,
            queued_op: None,
            queued_create: None,
            create_form: None,
            logs_state: None,
            preview_lines: Vec::new(),
            preview_for: None,
            ports_state: None,
            images: Vec::new(),
            ports: std::collections::HashMap::new(),
            busy: false,
        }
    }

    /// Process an event and return what the main loop should do next.
    pub fn handle_event(&mut self, event: AppEvent) -> Action {
        match event {
            AppEvent::Key(key) => self.handle_key(key),
            AppEvent::Tick => {
                if self.busy {
                    Action::Render // spinner animation cadence
                } else {
                    Action::Continue
                }
            }
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
                self.busy = false;
                Action::Render
            }
            AppEvent::OpDone(msg) => {
                self.status = Some(msg);
                self.busy = false;
                Action::Render
            }
            AppEvent::LogLines(lines) => {
                let mut rendered = false;
                if let Some(state) = &mut self.logs_state {
                    for line in lines.iter() {
                        state.push_line(line.clone());
                    }
                    rendered = true;
                }
                // Dashboard sidebar preview mirrors the same lines for the
                // sandbox it is tailing.
                if self.preview_for.is_some() {
                    self.push_preview_lines(&lines);
                    rendered = true;
                }
                if rendered {
                    Action::Render
                } else {
                    Action::Continue
                }
            }
            AppEvent::LogsEnded => {
                if let Some(state) = &mut self.logs_state {
                    state.follow = false;
                    Action::Render
                } else {
                    Action::Continue
                }
            }
            AppEvent::ImagesUpdated(images) => {
                self.images = images.clone();
                if let Some(form) = &mut self.create_form {
                    form.images = images;
                }
                Action::Render
            }
            AppEvent::PortsUpdated { name, ports } => {
                self.ports.insert(name.clone(), ports.clone());
                match &mut self.ports_state {
                    Some(state) if state.sandbox_name == name => {
                        state.update_ports(ports);
                        Action::Render
                    }
                    _ => Action::Continue,
                }
            }
            AppEvent::ExecDone { name, output } => {
                self.busy = false;
                let summary = if output.exit_code == 0 {
                    format!("exec '{name}' ok: {}", output.stdout.trim())
                } else {
                    format!(
                        "exec '{name}' exit {}: {}{}",
                        output.exit_code,
                        output.stderr.trim(),
                        if output.stderr.trim().is_empty() {
                            output.stdout.trim()
                        } else {
                            ""
                        }
                    )
                };
                self.status = Some(summary);
                Action::Render
            }
        }
    }

    /// The next confirmed operation, if any (consumes it).
    pub fn take_op(&mut self) -> Option<Op> {
        self.queued_op.take()
    }

    /// The queued create spec from the form, if any (consumes it).
    pub fn take_create_spec(&mut self) -> Option<crate::backend::CreateSpec> {
        self.queued_create.take()
    }

    /// Key handling for the current view.
    fn handle_key(&mut self, key: KeyEvent) -> Action {
        // Confirmation dialog takes precedence over everything.
        if self.confirm.is_some() {
            return self.handle_confirm_key(key);
        }

        match self.view {
            View::Dashboard => self.handle_dashboard_key(key),
            View::Create => self.handle_create_key(key),
            View::Logs => self.handle_logs_key(key),
            View::Ports => self.handle_ports_key(key),
            View::Help => self.handle_help_key(key),
            View::Inspect => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.view = View::Dashboard;
                    Action::Render
                }
                _ => Action::Continue,
            },
        }
    }

    /// Keys while a confirmation dialog is open: `y`/`Enter` queues the op,
    /// anything else cancels.
    fn handle_confirm_key(&mut self, key: KeyEvent) -> Action {
        let op = match self.confirm.take() {
            Some(op) => op,
            None => return Action::Continue,
        };
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                self.queued_op = Some(op);
                self.busy = true;
                self.status = Some("Working…".into());
                Action::Render
            }
            _ => {
                self.status = Some("Cancelled".into());
                Action::Render
            }
        }
    }

    /// Keys on the dashboard: selection, view switching, lifecycle ops.
    fn handle_dashboard_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            // Tab-bar number keys (mockup: [1] sandboxes, [2] logs, [3] ports).
            KeyCode::Char('1') => Action::Render, // already home
            KeyCode::Char('2') => self.open_logs(),
            KeyCode::Char('3') => self.open_ports(),
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
                self.open_create_form();
                Action::Render
            }
            KeyCode::Enter => self.open_view(View::Inspect),
            KeyCode::Char('l') => self.open_logs(),
            KeyCode::Char('p') => self.open_ports(),
            KeyCode::Char('r') => self.confirm_named(Op::Restart),
            KeyCode::Char('x') => self.confirm_named(Op::Stop),
            KeyCode::Char('s') => self.confirm_named(Op::Start),
            KeyCode::Delete => self.confirm_named(Op::Remove),
            KeyCode::Char('e') => {
                // Phase 1: exec opens the confirm dialog with a trivial demo
                // command; free-form command input lands in Phase 2.
                let Some(sbx) = self.selected_sandbox() else {
                    return Action::Continue;
                };
                let name = sbx.name.clone();
                self.confirm = Some(Op::Exec {
                    name,
                    cmd: vec!["/bin/sh".into(), "-c".into(), "echo exec-ok".into()],
                });
                Action::Render
            }
            KeyCode::Char('?') => {
                self.view = View::Help;
                Action::Render
            }
            // Uppercase U: install/update the host runtime (confirm dialog).
            KeyCode::Char('U') => {
                self.confirm = Some(Op::InstallRuntime);
                Action::Render
            }
            KeyCode::Esc => Action::Continue,
            _ => Action::Continue,
        }
    }

    /// Keys on the create form.
    fn handle_create_key(&mut self, key: KeyEvent) -> Action {
        let Some(form) = &mut self.create_form else {
            self.view = View::Dashboard;
            return Action::Render;
        };
        match form.handle_key(key) {
            crate::ui::create::FormAction::Cancel => {
                self.create_form = None;
                self.view = View::Dashboard;
                Action::Render
            }
            crate::ui::create::FormAction::Submit => match form.to_create_spec() {
                Ok(spec) => {
                    let label = spec.name.clone().unwrap_or_else(|| "<auto-name>".into());
                    self.create_form = None;
                    self.view = View::Dashboard;
                    self.busy = true;
                    self.status = Some(format!("Creating {label}…"));
                    self.queued_create = Some(spec);
                    Action::Render
                }
                Err(e) => {
                    form.error = Some(e.to_string());
                    Action::Render
                }
            },
            _ => Action::Render,
        }
    }

    /// Keys on the logs panel.
    fn handle_logs_key(&mut self, key: KeyEvent) -> Action {
        let Some(state) = &mut self.logs_state else {
            self.view = View::Dashboard;
            return Action::Render;
        };
        match state.handle_key(key) {
            crate::ui::logs::LogsAction::Back => {
                self.logs_state = None;
                self.view = View::Dashboard;
                Action::Render
            }
            _ => Action::Render,
        }
    }

    /// Keys on the ports view: binding selection, publish/unpublish, back.
    fn handle_ports_key(&mut self, key: KeyEvent) -> Action {
        // Publish/unpublish target the currently focused matrix binding.
        if let Some(op) = self.pending_port_op(key) {
            self.confirm = Some(op);
            return Action::Render;
        }
        let Some(state) = &mut self.ports_state else {
            self.view = View::Dashboard;
            return Action::Render;
        };
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.ports_state = None;
                self.view = View::Dashboard;
                Action::Render
            }
            KeyCode::Char('1') => {
                self.ports_state = None;
                self.view = View::Dashboard;
                Action::Render
            }
            KeyCode::Up => {
                state.select_prev();
                Action::Render
            }
            KeyCode::Down => {
                let rows = crate::ui::ports::matrix_rows(&self.sandboxes, &self.ports).len();
                state.select_next(rows.max(1));
                Action::Render
            }
            _ => Action::Continue,
        }
    }

    /// Map `+`/`-` on the ports view to a confirmed recreate op for the
    /// selected binding. `+` publishes (Phase 1 note: form lands with the
    /// quick-create sidebar; today it reports the recreate requirement for
    /// the selected binding), `-` unpublishes the focused binding.
    fn pending_port_op(&self, key: KeyEvent) -> Option<Op> {
        let state = self.ports_state.as_ref()?;
        match key.code {
            KeyCode::Char('-') | KeyCode::Char('_') => {
                let (name, p) =
                    crate::ui::ports::selected_binding(state, &self.sandboxes, &self.ports)?;
                Some(Op::UnpublishPort {
                    name: name.to_string(),
                    port: p.clone(),
                })
            }
            _ => None,
        }
    }

    /// Keys on the help overlay.
    fn handle_help_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc | KeyCode::Char('?') => {
                self.view = View::Dashboard;
                Action::Render
            }
            // `q` remains the global quit key, consistent with every view.
            KeyCode::Char('q') => {
                self.quit = true;
                Action::Quit
            }
            _ => Action::Continue,
        }
    }

    /// Open the create form, pre-seeding the image list from cache.
    fn open_create_form(&mut self) {
        self.create_form = Some(CreateForm::new(self.images.clone()));
        self.view = View::Create;
    }

    /// Open the logs view for the selected sandbox.
    pub fn open_logs(&mut self) -> Action {
        let Some(sbx) = self.selected_sandbox() else {
            return Action::Continue;
        };
        self.logs_state = Some(LogsState::new(&sbx.name));
        self.view = View::Logs;
        Action::Render
    }

    /// Open the ports view for the selected sandbox.
    fn open_ports(&mut self) -> Action {
        let Some(sbx) = self.selected_sandbox() else {
            return Action::Continue;
        };
        self.ports_state = Some(PortsState::new(&sbx.name));
        self.view = View::Ports;
        Action::Render
    }

    /// Begin confirming `op` for the selected sandbox.
    fn confirm_named(&mut self, make_op: impl FnOnce(String) -> Op) -> Action {
        let Some(sbx) = self.selected_sandbox() else {
            return Action::Continue;
        };
        self.confirm = Some(make_op(sbx.name.clone()));
        Action::Render
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
        self.reset_preview_if_moved();
    }

    /// Move the selection down one card (clamped at the bottom).
    pub fn select_next(&mut self) {
        if self.selected + 1 < self.sandboxes.len() {
            self.selected += 1;
        }
        self.reset_preview_if_moved();
    }

    /// Clear the preview buffer when the selection moved to another sandbox.
    fn reset_preview_if_moved(&mut self) {
        let current = self.selected_sandbox().map(|s| s.name.clone());
        if self.preview_for.as_ref() != current.as_ref() {
            self.preview_lines.clear();
            self.preview_for = current;
        }
    }

    /// Append preview lines, bounded to [`PREVIEW_MAX_LINES`].
    fn push_preview_lines(&mut self, lines: &[crate::backend::LogLine]) {
        for line in lines {
            self.preview_lines.push(line.clone());
        }
        let overflow = self.preview_lines.len().saturating_sub(PREVIEW_MAX_LINES);
        if overflow > 0 {
            self.preview_lines.drain(0..overflow);
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
        // Keep the preview target in sync with the (possibly changed) list.
        self.reset_preview_if_moved();
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
    use crate::backend::LogLine;
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

    // ---- confirm + op queue ----

    #[test]
    fn stop_key_confirms_then_queues_op() {
        let mut app = App::new();
        app.update_sandboxes(vec![summary("web")]);

        // `x` opens the confirm dialog; the op is not queued yet.
        assert_eq!(app.handle_event(key(KeyCode::Char('x'))), Action::Render);
        assert_eq!(app.confirm, Some(Op::Stop("web".into())));
        assert!(app.take_op().is_none());

        // `y` confirms; the op is queued and busy is set.
        assert_eq!(app.handle_event(key(KeyCode::Char('y'))), Action::Render);
        assert!(app.confirm.is_none());
        assert_eq!(app.take_op(), Some(Op::Stop("web".into())));
        assert!(app.busy);
        assert!(app.take_op().is_none()); // consumed
    }

    #[test]
    fn confirm_cancel_dismisses_without_queueing() {
        let mut app = App::new();
        app.update_sandboxes(vec![summary("web")]);
        app.handle_event(key(KeyCode::Delete)); // remove confirm

        assert_eq!(app.handle_event(key(KeyCode::Char('n'))), Action::Render);
        assert!(app.confirm.is_none());
        assert!(app.take_op().is_none());
        assert!(!app.busy);
    }

    #[test]
    fn remove_op_describes_destruction() {
        assert!(Op::Remove("db".into()).describe().contains("rootfs"));
        assert_eq!(Op::Stop("db".into()).describe(), "Stop sandbox 'db'?");
    }

    #[test]
    fn exec_key_queues_after_confirm_and_completes() {
        let mut app = App::new();
        app.update_sandboxes(vec![summary("api")]);
        app.handle_event(key(KeyCode::Char('e')));

        let queued = app.confirm.take().expect("exec opens confirm");
        let Op::Exec { name, cmd } = queued else {
            panic!("expected Exec op");
        };
        assert_eq!(name, "api");
        assert!(cmd.contains(&"echo exec-ok".to_string()));

        // ExecDone clears busy and reports the output.
        app.busy = true;
        let action = app.handle_event(AppEvent::ExecDone {
            name: "api".into(),
            output: crate::backend::ExecOutput {
                stdout: "exec-ok\n".into(),
                stderr: String::new(),
                exit_code: 0,
            },
        });
        assert_eq!(action, Action::Render);
        assert!(!app.busy);
        assert!(
            app.status
                .as_deref()
                .unwrap_or_default()
                .contains("exec-ok")
        );
    }

    #[test]
    fn lifecycle_keys_require_a_selection() {
        let mut app = App::new();
        for k in ['r', 'x', 's', 'e'] {
            app.view = View::Dashboard;
            assert_eq!(app.handle_event(key(KeyCode::Char(k))), Action::Continue);
            assert!(
                app.confirm.is_none(),
                "key {k} must not open confirm on empty list"
            );
        }
        // Delete on an empty list is also a no-op.
        assert_eq!(app.handle_event(key(KeyCode::Delete)), Action::Continue);

        // U works even with an empty list (runtime may be missing entirely).
        assert_eq!(app.handle_event(key(KeyCode::Char('U'))), Action::Render);
        assert_eq!(app.confirm, Some(Op::InstallRuntime));
    }

    #[test]
    fn opdone_and_error_clear_busy() {
        let mut app = App::new();
        app.busy = true;
        app.handle_event(AppEvent::OpDone("done".into()));
        assert!(!app.busy);
        assert_eq!(app.status.as_deref(), Some("done"));

        app.busy = true;
        app.handle_event(AppEvent::Error("boom".into()));
        assert!(!app.busy);
        assert_eq!(app.error.as_deref(), Some("boom"));
    }

    #[test]
    fn log_lines_feed_logs_state() {
        let mut app = App::new();
        app.update_sandboxes(vec![summary("web")]);
        app.open_logs();

        let line = LogLine {
            id: 1,
            source: "stdout".into(),
            data: "hello\n".into(),
            timestamp: chrono::Utc::now(),
        };
        assert_eq!(
            app.handle_event(AppEvent::LogLines(vec![line])),
            Action::Render
        );
        assert_eq!(app.logs_state.as_ref().unwrap().lines.len(), 1);
    }
}
