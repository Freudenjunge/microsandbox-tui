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

use crate::actions::CardAction;
use crate::event::Action;
use crate::event::AppEvent;
use crate::models::{Metrics, PublishedPort, SandboxState, SandboxSummary};
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
    /// Environments & credentials manager (issues #7/#6, dashboard `E`).
    Environments,
}

/// Detail pane tabs for the sandbox selected in the left rail (2.9 IA:
/// sandboxes always visible on the left, everything scoped to ONE sandbox
/// on the right — the `sbx <name> …` mental model).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DetailTab {
    /// Live metrics + this sandbox's ports.
    #[default]
    Overview,
    /// Log stream of this sandbox.
    Logs,
    /// Port publish/unpublish of this sandbox (`sbx publish/unpublish`).
    Ports,
}

impl DetailTab {
    /// All tabs in number-key order.
    pub const ALL: [DetailTab; 3] = [DetailTab::Overview, DetailTab::Logs, DetailTab::Ports];

    /// The `1`-`3` number key that selects this tab.
    pub fn key(self) -> char {
        match self {
            Self::Overview => '1',
            Self::Logs => '2',
            Self::Ports => '3',
        }
    }

    /// Tab label for the detail bar.
    pub fn label(self) -> &'static str {
        match self {
            Self::Overview => "OVERVIEW",
            Self::Logs => "LOGS",
            Self::Ports => "PORTS",
        }
    }

    /// Cycle to the next tab.
    pub fn next(self) -> Self {
        let idx = Self::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }

    /// Cycle to the previous tab.
    pub fn prev(self) -> Self {
        let idx = Self::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Self::ALL[(idx + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

impl View {
    /// Whether this is the default (top-level) view.
    pub fn is_dashboard(self) -> bool {
        matches!(self, View::Dashboard)
    }
}

/// State of the create flow (Spec §3/§4): the master-detail template
/// select is the entry; `e` opens the full-screen form pre-filled, `n`
/// empty. `Enter` in the select creates directly from the template.
#[derive(Debug)]
pub enum CreateState {
    /// Template picker with the highlighted row.
    Select { selected: usize },
    /// Full-screen grouped form.
    Form(Box<crate::ui::create::CreateForm>),
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
    /// Grow the named sandbox's root disk to `mib` MiB (grow-only; the
    /// backend restarts the VM when the growth cannot be applied online).
    ResizeDisk { name: String, mib: u32 },
    /// Install or update the host microsandbox runtime to the SDK version.
    InstallRuntime,
    /// Delete a user template file (built-ins cannot be deleted).
    DeleteTemplate(String),
    /// Write a template file over an existing one (confirmed).
    OverwriteTemplate(Box<crate::template::Template>),
    /// Update the TUI itself to the detected newer stable release
    /// (downloads the release tarball, sha256-verified, replaces the
    /// binary; applies on relaunch).
    UpdateSelf(Box<crate::selfupdate::UpdateInfo>),
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
                    "Publish {}:{}→{}/{} on '{name}'? (snapshots the disk, recreates from it — your data is preserved, processes restart)",
                    port.host_bind, port.host_port, port.guest_port, port.protocol
                )
            }
            Op::UnpublishPort { name, port } => {
                format!(
                    "Unpublish {}:{} from '{name}'? (snapshots the disk, recreates from it — your data is preserved, processes restart)",
                    port.host_port, port.guest_port
                )
            }
            Op::ResizeDisk { name, mib } => {
                format!(
                    "Grow root disk of '{name}' to {}? (grow-only — data is preserved; the sandbox restarts if the runtime cannot grow it online)",
                    crate::ui::dashboard::format_bytes(u64::from(*mib) * 1024 * 1024)
                )
            }
            Op::InstallRuntime => {
                format!(
                    "Install/update microsandbox runtime to v{}? (downloads the official bundle)",
                    crate::runtime::sdk_version()
                )
            }
            Op::DeleteTemplate(id) => {
                format!("Delete template '{id}'? (the file will be removed)")
            }
            Op::OverwriteTemplate(t) => {
                format!(
                    "Template '{}' exists — overwrite? ({slug}.toml will be replaced)",
                    t.meta.name,
                    slug = crate::template::slugify(&t.meta.name)
                )
            }
            Op::UpdateSelf(u) => format!(
                "Update msb-tui v{} → v{}? (downloads the release tarball, sha256-verified, replaces this binary — relaunch to apply)",
                u.current, u.latest
            ),
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

/// An OS-keyring operation queued by the environments & credentials
/// manager; the main loop spawns it (keyring I/O is blocking) and reports
/// back as [`AppEvent::KeyringResult`], which clears the view's pending
/// marker. Consumed via [`App::take_keyring_op`].
pub enum KeyringOp {
    /// Store/update the secret stored under credential `name`.
    Set { name: String, secret: String },
    /// Delete the secret stored under credential `name`.
    Delete { name: String },
}

impl std::fmt::Debug for KeyringOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The secret never reaches Debug (and so never logs): same rule
        // as the keyring storage itself.
        match self {
            Self::Set { name, .. } => write!(f, "KeyringOp::Set({name})"),
            Self::Delete { name } => write!(f, "KeyringOp::Delete({name})"),
        }
    }
}

/// One-field input prompt for growing a sandbox's root disk (`g` on the
/// dashboard). Enter validates and queues [`Op::ResizeDisk`] for the usual
/// confirm dialog; Esc cancels.
#[derive(Debug)]
pub struct ResizePrompt {
    /// Target sandbox name.
    pub name: String,
    /// Typed size string (e.g. `16G`).
    pub input: String,
}

/// Maximum lines kept for the dashboard sidebar log preview.
const PREVIEW_MAX_LINES: usize = 6;

/// The central application state.
/// Whether the main loop should (re)fetch ports for all sandboxes this
/// cycle.
///
/// Triggers:
/// - the ports cache is dirty (PORTS tab entry via `open_ports`, the `r`
///   refresh key, or a publish/unpublish recreate via `OpDone`), or
/// - the cache was never primed for this session (the initial fetch lets
///   cards show ports before the PORTS tab is ever opened).
///
/// Dirty is one-shot: the loop clears it when the fetch is spawned, so a
/// fetch happens exactly once per trigger. Replaces the pre-2.9
/// `View::Ports` trigger, which could never fire after the detail-tab
/// rework (smoke finding 2026-09-24).
pub fn ports_fetch_needed(app: &App) -> bool {
    if app.sandboxes.is_empty() {
        return false;
    }
    app.ports_cache_dirty || !app.ports_primed
}

/// How many 250 ms UI ticks a transient status/error message stays
/// visible before auto-clearing (20 ticks ≈ 5 s).
pub const STATUS_TTL_TICKS: u32 = 20;

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
    /// Ticks since the status message was (re)set; expires it.
    status_age_ticks: u32,
    /// Ticks since the error message was (re)set; expires it.
    error_age_ticks: u32,
    /// Operation awaiting confirmation (`y`/`n`), if any.
    pub confirm: Option<Op>,
    /// Open root-disk size prompt (`g` on the dashboard), if any.
    pub resize_prompt: Option<ResizePrompt>,
    /// Confirmed op queued for the main loop; consumed via [`App::take_op`].
    pub queued_op: Option<Op>,
    /// Resolved shell-window request from the guest-bash probe; consumed
    /// via [`App::take_shell_ready`].
    pub shell_ready: Option<(String, bool)>,
    /// Resolved create context for the queued spec, consumed via
    /// [`App::take_create`].
    pub queued_create: Option<crate::backend::CreateSpec>,
    /// Form-selected credential for the queued create; the create task
    /// resolves the keyring secret from it (issue #6, fail closed).
    pub queued_credential: Option<String>,
    /// Effective profile for the queued create (form select or the
    /// template's `env_profile`) — profile-scoped credentials key on it.
    pub queued_profile: Option<String>,
    /// Keyring op queued by the environments manager; the main loop runs
    /// it off-thread ([`App::take_keyring_op`]).
    pub queued_keyring: Option<KeyringOp>,
    /// Create-flow state: template select ↔ full-screen form.
    pub create_state: Option<CreateState>,
    /// Template to write to disk (Ctrl+S), consumed by the main loop.
    pub take_save_template: Option<crate::template::Template>,
    /// Loaded templates (user files shadow built-ins).
    pub templates: Vec<crate::template::Template>,
    /// Logs-view state; `Some` while the logs view is open.
    pub logs_state: Option<LogsState>,
    /// Bounded log preview for the selected sandbox on the dashboard
    /// sidebar (fed by the same tail task as the logs view).
    pub preview_lines: Vec<crate::backend::LogLine>,
    /// Sandbox name the preview buffer belongs to.
    pub preview_for: Option<String>,
    /// Ports-view state; `Some` while the ports view is open.
    pub ports_state: Option<PortsState>,
    /// Active detail tab for the rail-selected sandbox.
    pub detail: DetailTab,
    /// Environments & credentials manager (issues #7/#6); `Some` while the
    /// view is open. Created lazily on the first `E` — the view reads both
    /// config files, which must not run at startup.
    pub environments: Option<crate::ui::environments::EnvironmentsView>,
    /// True while the interactive shell hand-off is running (TUI paused).
    pub shell_suspended: bool,
    /// Cached images for the create form picker (with sizes).
    pub images: Vec<crate::models::Image>,
    /// Published ports per sandbox (dashboard cards + ports view).
    pub ports: std::collections::HashMap<String, Vec<crate::models::PublishedPort>>,
    /// True when the port cache should be refetched (view entered, `r`
    /// pressed, or a publish/unpublish recreate finished).
    pub ports_cache_dirty: bool,
    /// True once the ports cache has been primed for the current sandbox
    /// list (cards show ports before the PORTS tab is ever opened).
    pub ports_primed: bool,
    /// True while a long-running `msb` operation is in flight.
    pub busy: bool,
    /// True until the FIRST sandbox-list refresh arrives (drives the
    /// dashboard's loading placeholder — smoke-test finding 2.8).
    pub initial_list_loaded: bool,
    /// Sandboxes marked for boot autostart (persisted in
    /// `~/.config/microsandbox-tui/autostart.toml`; the main loop owns
    /// the file I/O, App holds the in-memory truth).
    pub autostart: std::collections::HashSet<String>,
    /// Whether the boot-autostart systemd unit is installed (one-time
    /// main-loop read; drives the banner hint).
    pub autostart_installed: bool,
    /// Set when the autostart set changed and needs persisting; consumed
    /// by the main loop's writer via [`App::take_autostart_change`].
    autostart_dirty: bool,
    /// A newer **stable** release of msb-tui itself, detected by the
    /// one-shot startup check (`None` = no update known). Drives the
    /// dashboard banner and the `V` key; the main loop runs the op.
    pub update: Option<crate::selfupdate::UpdateInfo>,
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
            status_age_ticks: 0,
            error_age_ticks: 0,
            confirm: None,
            resize_prompt: None,
            queued_op: None,
            shell_ready: None,
            queued_create: None,
            queued_credential: None,
            queued_profile: None,
            queued_keyring: None,
            create_state: None,
            take_save_template: None,
            templates: Vec::new(),
            logs_state: None,
            preview_lines: Vec::new(),
            preview_for: None,
            ports_state: None,
            detail: DetailTab::default(),
            environments: None,
            shell_suspended: false,
            images: Vec::new(),
            ports: std::collections::HashMap::new(),
            ports_cache_dirty: false,
            ports_primed: false,
            busy: false,
            initial_list_loaded: false,
            autostart: std::collections::HashSet::new(),
            autostart_installed: false,
            autostart_dirty: false,
            update: None,
        }
    }

    /// Process an event and return what the main loop should do next.
    pub fn handle_event(&mut self, event: AppEvent) -> Action {
        match event {
            AppEvent::Key(key) => self.handle_key(key),
            AppEvent::Tick => {
                // Spinner animation while busy; otherwise age the transient
                // status/error messages. Either way, check whether a
                // re-render is warranted.
                if self.busy || self.expire_transient_messages() {
                    Action::Render
                } else {
                    Action::Continue
                }
            }
            AppEvent::SandboxesUpdated(list) => {
                self.update_sandboxes(list);
                self.initial_list_loaded = true;
                Action::Render
            }
            AppEvent::MetricsUpdated(samples) => {
                self.update_metrics(samples);
                Action::Render
            }
            AppEvent::Error(msg) => {
                self.error = Some(msg);
                self.error_age_ticks = 0;
                self.busy = false;
                Action::Render
            }
            AppEvent::OpDone(msg) => {
                self.status = Some(msg);
                self.status_age_ticks = 0;
                self.busy = false;
                // Publish/unpublish/restart recreate the sandbox: its port
                // bindings changed, so the ports cache must be refetched.
                self.ports_cache_dirty = true;
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
                if let Some(CreateState::Form(form)) = &mut self.create_state {
                    form.images = images;
                }
                Action::Render
            }
            AppEvent::TemplatesLoaded(templates) => {
                self.templates = templates;
                // Clamp a stale picker selection to the new list length
                // (delete/reload shrink the list; + 1 for the scratch row).
                if let Some(CreateState::Select { selected }) = &mut self.create_state {
                    if self.templates.is_empty() {
                        *selected = 0;
                    } else {
                        *selected = (*selected).min(self.templates.len());
                    }
                }
                Action::Render
            }
            AppEvent::TemplateSaved(t) => {
                // The saved template was written by the main loop; refresh
                // the in-memory list entry (id = shadowing key).
                let id = t.id.clone();
                match self.templates.iter().position(|x| x.id == id) {
                    Some(pos) => self.templates[pos] = t,
                    None => self.templates.insert(0, t),
                }
                Action::Render
            }
            AppEvent::TemplateDeleted(id) => {
                self.templates.retain(|t| t.id != id);
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
            AppEvent::ExecDone { .. } => {
                // Legacy captured-exec event (confirm-dialog demo path).
                self.busy = false;
                Action::Render
            }
            AppEvent::UpdateAvailable(update) => {
                self.update = Some(update);
                Action::Render
            }
            AppEvent::ShellReady { name, bash } => {
                // The guest-bash probe finished; the main loop spawns the
                // window from `take_shell_ready()` (carries name + shell).
                self.shell_ready = Some((name, bash));
                Action::Render
            }
            AppEvent::UpdateInstalled(version) => {
                // Binary swapped on disk; the banner must not offer the
                // same update again. Relaunch applies it.
                self.update = None;
                self.busy = false;
                self.status = Some(format!(
                    "updated to v{version} — exit and relaunch to run it"
                ));
                self.status_age_ticks = 0;
                Action::Render
            }
            AppEvent::KeyringResult { ok, msg } => {
                // Clears the environments view's pending marker; the view
                // not existing (closed before completion) loses nothing —
                // the result also lands on the app status line.
                if let Some(view) = &mut self.environments {
                    view.note_async_result(ok, &msg);
                }
                if ok {
                    self.set_status(msg);
                } else {
                    self.set_error(msg);
                }
                Action::Render
            }
        }
    }

    /// The next confirmed operation, if any (consumes it).
    pub fn take_op(&mut self) -> Option<Op> {
        self.queued_op.take()
    }

    /// The resolved shell-window request, if any (consumes it).
    pub fn take_shell_ready(&mut self) -> Option<(String, bool)> {
        self.shell_ready.take()
    }

    /// The queued create spec from the form or template picker, consumed
    /// TOGETHER with the credential/profile context captured at queueing
    /// time (issues #6/#7 — the create task's keyring injection keys on
    /// them). One take so the three fields can never drift apart.
    pub fn take_create(
        &mut self,
    ) -> Option<(crate::backend::CreateSpec, Option<String>, Option<String>)> {
        self.queued_create.take().map(|spec| {
            (
                spec,
                self.queued_credential.take(),
                self.queued_profile.take(),
            )
        })
    }

    /// The queued keyring operation from the environments manager, if any
    /// (consumes it).
    pub fn take_keyring_op(&mut self) -> Option<KeyringOp> {
        self.queued_keyring.take()
    }

    /// The template queued by the Ctrl+S dialog (no collision), consumed
    /// by the main loop's writer task.
    pub fn take_save_template(&mut self) -> Option<crate::template::Template> {
        self.take_save_template.take()
    }

    /// Key handling for the current view.
    fn handle_key(&mut self, key: KeyEvent) -> Action {
        // Confirmation dialog takes precedence over everything; the disk
        // resize prompt sits directly below it.
        if self.confirm.is_some() {
            return self.handle_confirm_key(key);
        }
        if self.resize_prompt.is_some() {
            return self.handle_resize_key(key);
        }

        match self.view {
            View::Dashboard => {
                // The LOGS detail tab owns its keys (follow/grep/source,
                // scroll): its state exists only while the tab is open.
                if self.detail == DetailTab::Logs && self.logs_state.is_some() {
                    return self.handle_logs_key(key);
                }
                // The PORTS detail tab owns its keys (publish form etc.).
                if self.detail == DetailTab::Ports && self.ports_state.is_some() {
                    return self.handle_ports_key(key);
                }
                self.handle_dashboard_key(key)
            }
            View::Create => self.handle_create_key(key),
            // The manager consumes its inner-tab keys (1/2/Left/Right/Tab)
            // itself; nothing global may fire first (only the confirm /
            // resize overlays above take precedence).
            View::Environments => self.handle_environments_key(key),
            View::Logs | View::Help => self.handle_help_key(key),
            View::Ports | View::Inspect => self.handle_dashboard_key(key),
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
                self.set_status("Working…");
                Action::Render
            }
            _ => {
                self.set_status("Cancelled");
                Action::Render
            }
        }
    }

    /// Keys while the root-disk resize prompt is open. Enter validates the
    /// size (`parse_memory_mib`) and hands the op to the confirm dialog;
    /// anything else keeps typing, with Backspace editing and Esc cancelling.
    fn handle_resize_key(&mut self, key: KeyEvent) -> Action {
        let Some(prompt) = self.resize_prompt.as_mut() else {
            return Action::Render;
        };
        match key.code {
            KeyCode::Enter => {
                let mib = crate::backend::sdk::parse_memory_mib(prompt.input.trim());
                match mib {
                    Some(mib) if mib > 0 => {
                        let name = prompt.name.clone();
                        self.resize_prompt = None;
                        self.confirm = Some(Op::ResizeDisk { name, mib });
                    }
                    _ => self.set_error("Disk size must be like '4G' or '16G'"),
                }
                Action::Render
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.resize_prompt = None;
                self.set_status("Cancelled");
                Action::Render
            }
            KeyCode::Backspace => {
                prompt.input.pop();
                Action::Render
            }
            KeyCode::Char(c) => {
                prompt.input.push(c);
                Action::Render
            }
            _ => Action::Render,
        }
    }

    /// Keys on the dashboard (2.9 IA): rail selection + detail tabs.
    fn handle_dashboard_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            // Detail-tab number keys + Tab cycling.
            KeyCode::Char('1') => {
                self.detail = DetailTab::Overview;
                Action::Render
            }
            KeyCode::Char('2') => {
                self.detail = DetailTab::Logs;
                self.open_logs()
            }
            KeyCode::Char('3') => {
                self.detail = DetailTab::Ports;
                self.open_ports()
            }
            KeyCode::Tab => {
                self.detail = self.detail.next();
                // Ensure tab-scoped state exists (logs tail, ports fetch).
                match self.detail {
                    DetailTab::Logs => {
                        self.open_logs();
                    }
                    DetailTab::Ports => {
                        self.open_ports();
                    }
                    _ => {}
                }
                Action::Render
            }
            KeyCode::BackTab => {
                self.detail = self.detail.prev();
                Action::Render
            }
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
                self.open_create_templates();
                Action::Render
            }
            KeyCode::Enter => {
                self.detail = DetailTab::Overview;
                Action::Render
            }
            // Docker-sbx parity: `s` toggles start/stop by state.
            KeyCode::Char('s') | KeyCode::Char('S') => match self.gated_state_toggle() {
                Some(crate::actions::CardAction::Start) => {
                    self.confirm_gated(CardAction::Start, Op::Start)
                }
                Some(crate::actions::CardAction::Stop) => {
                    self.confirm_gated(CardAction::Stop, Op::Stop)
                }
                _ => self.reject_unavailable(CardAction::Start),
            },
            // Docker-sbx parity: `x` = E**x**ec (interactive shell in a new
            // window — leave the TUI, run the sandbox shell, restore on exit).
            KeyCode::Char('x') => {
                match self.gated_state(CardAction::Shell) {
                    Some(_) => {
                        self.shell_suspended = true;
                        Action::Render
                    }
                    // Unavailable for this sandbox's state: explain, no dialog.
                    None if self.selected_sandbox().is_some() => {
                        self.reject_unavailable(CardAction::Shell)
                    }
                    // Empty list: previous no-op behavior (render, nothing else).
                    None => Action::Render,
                }
            }
            KeyCode::Char('r') => self.confirm_gated(CardAction::Restart, Op::Restart),
            // `g`: grow the selected sandbox's root disk (grow-only, data
            // preserved). Opens the size prompt; the SDK restarts the VM
            // when it cannot grow online.
            KeyCode::Char('g') | KeyCode::Char('G') => {
                if let Some(sbx) = self.selected_sandbox() {
                    self.resize_prompt = Some(ResizePrompt {
                        name: sbx.name.clone(),
                        input: String::new(),
                    });
                }
                Action::Render
            }
            KeyCode::Delete => self.confirm_named(Op::Remove),
            // `a`/`A`: mark/unmark the selected sandbox for boot autostart
            // (s/S parity: caps-lock must not hide the feature).
            KeyCode::Char('a') | KeyCode::Char('A') => self.toggle_autostart(),
            KeyCode::Char('?') => {
                self.view = View::Help;
                Action::Render
            }
            // `E`: the environments & credentials manager (issues #7/#6).
            // Uppercase — distinct from lowercase `e`, which belongs to the
            // create flow's pre-filled form, not the dashboard.
            KeyCode::Char('E') => self.open_environments(),
            // Uppercase U: install/update the host runtime (confirm dialog).
            KeyCode::Char('U') => {
                self.confirm = Some(Op::InstallRuntime);
                Action::Render
            }
            // `v`/`V`: update msb-tui itself when the startup check found a
            // newer stable release (caps parity with s/S and a/A). Key is
            // inert while no update is known.
            KeyCode::Char('v') | KeyCode::Char('V') => {
                if let Some(info) = self.update.clone() {
                    self.confirm = Some(Op::UpdateSelf(Box::new(info)));
                }
                Action::Render
            }
            KeyCode::Esc => Action::Continue,
            _ => Action::Continue,
        }
    }

    /// Keys on the create form.
    fn handle_create_key(&mut self, key: KeyEvent) -> Action {
        match self.create_state.take() {
            Some(CreateState::Select { selected }) => {
                self.handle_template_select_key(key, selected)
            }
            Some(CreateState::Form(mut form)) => match form.handle_key(key) {
                crate::ui::create::FormAction::Cancel => {
                    // Back to the picker, not the dashboard (Spec §3).
                    self.create_state = Some(CreateState::Select { selected: 0 });
                    Action::Render
                }
                crate::ui::create::FormAction::Submit => match form.to_create_spec() {
                    Ok(spec) => {
                        // Capture the form's picks before the form is
                        // dropped: the create task resolves credential
                        // injection from them (issue #6).
                        self.queued_profile = form.selected_profile().map(str::to_string);
                        self.queued_credential = form.selected_credential().map(str::to_string);
                        let label = spec.name.clone().unwrap_or_else(|| "<auto-name>".into());
                        self.view = View::Dashboard;
                        self.busy = true;
                        self.set_status(format!("Creating {label}…"));
                        self.queued_create = Some(spec);
                        Action::Render
                    }
                    Err(e) => {
                        form.error = Some(e.to_string());
                        self.create_state = Some(CreateState::Form(form));
                        Action::Render
                    }
                },
                crate::ui::create::FormAction::SaveTemplate(t) => {
                    // Collision (same slug among loaded templates, incl.
                    // built-ins) → ask before overwriting; a fresh name
                    // goes straight to disk (Task 8 consumes it).
                    let slug = crate::template::slugify(&t.meta.name);
                    if self.templates.iter().any(|x| x.id == slug) {
                        self.confirm = Some(Op::OverwriteTemplate(t));
                    } else {
                        self.take_save_template = Some(*t);
                    }
                    self.create_state = Some(CreateState::Form(form));
                    Action::Render
                }
                other => {
                    self.create_state = Some(CreateState::Form(form));
                    let _ = other;
                    Action::Render
                }
            },
            None => {
                self.view = View::Dashboard;
                Action::Render
            }
        }
    }

    /// Keys on the template picker (master-detail, Spec §3). Row 0 is the
    /// pseudo-entry "Create from scratch" (an empty form, not a template);
    /// template rows sit at index + 1.
    fn handle_template_select_key(&mut self, key: KeyEvent, selected: usize) -> Action {
        // Navigation and leave work the same on every row — including the
        // scratch row (0), where the picker always opens.
        let last = self.templates.len(); // rows: scratch (0) … len() (last template)
        match key.code {
            KeyCode::Up => {
                self.create_state = Some(CreateState::Select {
                    selected: selected.saturating_sub(1),
                });
                return Action::Render;
            }
            KeyCode::Down => {
                self.create_state = Some(CreateState::Select {
                    selected: (selected + 1).min(last),
                });
                return Action::Render;
            }
            KeyCode::Esc => {
                self.view = View::Dashboard;
                return Action::Render;
            }
            _ => {}
        }
        // Row 0 is the scratch pseudo-entry and is always present, so the
        // picker is usable even with zero templates loaded. It is not a
        // template: Enter/e open the empty form (queues nothing, so the
        // busy guard does not apply) and `d` has nothing to delete.
        if selected == 0 {
            match key.code {
                KeyCode::Enter | KeyCode::Char('e') | KeyCode::Char('n') => {
                    return self.open_scratch_form();
                }
                KeyCode::Char('d') => {
                    self.set_status("create-from-scratch is not a saved template");
                    self.create_state = Some(CreateState::Select { selected });
                    return Action::Render;
                }
                _ => {
                    self.create_state = Some(CreateState::Select { selected });
                    return Action::Continue;
                }
            }
        }
        let tpl_index = selected - 1;
        match key.code {
            KeyCode::Enter => {
                // Review Focus 3: never queue a second create while one
                // is running.
                if self.busy {
                    self.create_state = Some(CreateState::Select { selected });
                    return Action::Continue;
                }
                let Some(template) = self.templates.get(tpl_index) else {
                    // Stale index (list shrank): no-op with a hint, no panic.
                    self.set_status("template list changed — select again");
                    self.create_state = Some(CreateState::Select { selected: 0 });
                    return Action::Render;
                };
                let cwd = self.create_cwd();
                match crate::template::to_create_spec(template, &cwd) {
                    Ok(mut spec) => {
                        // Name pattern taken → first free -N suffix.
                        if let Some(pattern) = template.spec.name.clone() {
                            let existing: Vec<String> =
                                self.sandboxes.iter().map(|s| s.name.clone()).collect();
                            if let Some(resolved) =
                                crate::template::resolve_name(&pattern, &existing)
                            {
                                spec.name = Some(resolved);
                            }
                        }
                        let label = spec.name.clone().unwrap_or_else(|| "<auto-name>".into());
                        // Direct template create: the template's env
                        // profile drives profile-scoped credential
                        // injection; there is no form, hence no explicit
                        // credential selection.
                        self.queued_profile = template.spec.env_profile.clone();
                        self.queued_credential = None;
                        self.view = View::Dashboard;
                        self.busy = true;
                        self.set_status(format!("Creating {label}…"));
                        self.queued_create = Some(spec);
                        Action::Render
                    }
                    Err(e) => {
                        self.set_status(format!("Template error: {e:#}"));
                        self.create_state = Some(CreateState::Select { selected });
                        Action::Render
                    }
                }
            }
            KeyCode::Char('e') => {
                let Some(template) = self.templates.get(tpl_index) else {
                    self.set_status("template list changed — select again");
                    self.create_state = Some(CreateState::Select { selected: 0 });
                    return Action::Render;
                };
                let mut form =
                    crate::ui::create::CreateForm::from_template(template, self.create_cwd());
                // I3: the e-path form sees the same cached image list as `n`.
                form.images = self.images.clone();
                self.create_state = Some(CreateState::Form(Box::new(form)));
                Action::Render
            }
            KeyCode::Char('n') => self.open_scratch_form(),
            KeyCode::Char('d') => {
                let Some(template) = self.templates.get(tpl_index) else {
                    self.set_status("template list changed — select again");
                    self.create_state = Some(CreateState::Select { selected: 0 });
                    return Action::Render;
                };
                if template.built_in {
                    self.set_status(
                        "built-in template — create a file with the same id in the template dir to override it",
                    );
                    self.create_state = Some(CreateState::Select { selected });
                } else {
                    let id = template.id.clone();
                    self.confirm = Some(Op::DeleteTemplate(id));
                    self.create_state = Some(CreateState::Select { selected });
                }
                Action::Render
            }
            _ => {
                self.create_state = Some(CreateState::Select { selected });
                Action::Continue
            }
        }
    }

    /// Open the empty create form (the "Create from scratch" row and the
    /// `n` key): no template, cached images seeded, CWD mount prefilled
    /// from the dashboard process CWD.
    fn open_scratch_form(&mut self) -> Action {
        let mut form = crate::ui::create::CreateForm::new(self.images.clone());
        form.cwd = self.create_cwd();
        form.workdir = form.cwd.clone();
        self.create_state = Some(CreateState::Form(Box::new(form)));
        Action::Render
    }

    /// The CWD captured for the workspace mount when the create flow
    /// opened (the dashboard's process CWD).
    fn create_cwd(&self) -> String {
        std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
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
        // The inline publish form swallows all keys while open.
        if let Some(form) = self
            .ports_state
            .as_mut()
            .and_then(|s| s.publish_form.as_mut())
        {
            match form.handle_key(key) {
                crate::ui::ports::PublishFormAction::Submit(port) => {
                    // The port belongs to the SELECTED matrix row's sandbox
                    // — the matrix is a global overview, but each binding
                    // belongs to exactly one sandbox.
                    let target = crate::ui::ports::selected_binding(
                        self.ports_state.as_ref().expect("state checked above"),
                        &self.sandboxes,
                        &self.ports,
                    )
                    .map(|(name, _)| name.to_string())
                    .or_else(|| self.ports_state.as_ref().map(|s| s.sandbox_name.clone()));
                    if let Some(state) = self.ports_state.as_mut() {
                        state.publish_form = None;
                    }
                    if let Some(name) = target {
                        self.confirm = Some(Op::PublishPort { name, port });
                    }
                    return Action::Render;
                }
                crate::ui::ports::PublishFormAction::Cancel => {
                    if let Some(state) = self.ports_state.as_mut() {
                        state.publish_form = None;
                    }
                    return Action::Render;
                }
                crate::ui::ports::PublishFormAction::Continue => return Action::Render,
            }
        }
        // Publish/unpublish target the currently focused matrix binding.
        if let Some(op) = self.pending_port_op(key) {
            self.confirm = Some(op);
            return Action::Render;
        }
        let Some(state) = &mut self.ports_state else {
            self.detail = DetailTab::Overview;
            return Action::Render;
        };
        match key.code {
            // Back to the rail keys (the ports tab is embedded in the
            // dashboard; number keys must still switch detail tabs).
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('1') => {
                self.detail = DetailTab::Overview;
                Action::Render
            }
            KeyCode::Char('2') => {
                self.detail = DetailTab::Logs;
                self.open_logs()
            }
            KeyCode::Char('3') => Action::Render, // already here
            KeyCode::Tab => {
                self.detail = self.detail.next();
                Action::Render
            }
            KeyCode::Char('+') | KeyCode::Char('p') => {
                state.publish_form = Some(crate::ui::ports::PublishForm::new());
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

    /// Open the create flow: the template picker (master-detail).
    fn open_create_templates(&mut self) {
        self.create_state = Some(CreateState::Select { selected: 0 });
        self.view = View::Create;
    }

    /// Open the environments & credentials manager. The view state is
    /// created on the first entry only (it reads both config files — no
    /// disk I/O before `E` is ever used).
    fn open_environments(&mut self) -> Action {
        if self.environments.is_none() {
            self.environments = Some(crate::ui::environments::EnvironmentsView::new());
        }
        self.view = View::Environments;
        Action::Render
    }

    /// Keys on the environments & credentials manager. The view consumes
    /// its keys first and is already "live": profile saves/deletes and
    /// credential metadata changes are fully settled by the view itself.
    /// This only translates its actions into the event loop's queues —
    /// the two keyring-carrying credential actions spawn work the loop
    /// runs off-thread ([`AppEvent::KeyringResult`] reports back).
    fn handle_environments_key(&mut self, key: KeyEvent) -> Action {
        let Some(view) = self.environments.as_mut() else {
            self.view = View::Dashboard;
            return Action::Render;
        };
        match view.handle_key(key) {
            crate::ui::environments::EnvAction::Done => {
                self.environments = None;
                self.view = View::Dashboard;
            }
            crate::ui::environments::EnvAction::SaveCredential(cred, Some(secret)) => {
                // New/changed secret: store it in the OS keyring off-thread.
                self.queued_keyring = Some(KeyringOp::Set {
                    name: cred.name,
                    secret,
                });
            }
            crate::ui::environments::EnvAction::DeleteCredential(name) => {
                // Metadata already persisted by the view; the keyring
                // secret goes too (idempotent when missing).
                self.queued_keyring = Some(KeyringOp::Delete { name });
            }
            // SaveProfile/DeleteProfile: fully persisted by the view.
            // SaveCredential(_, None): metadata saved, secret unchanged —
            // nothing further to do.
            _ => {}
        }
        Action::Render
    }

    /// Open the logs view for the selected sandbox.
    pub fn open_logs(&mut self) -> Action {
        let Some(sbx) = self.selected_sandbox() else {
            return Action::Continue;
        };
        self.logs_state = Some(LogsState::new(&sbx.name));
        self.view = View::Dashboard;
        Action::Render
    }

    /// Open the ports detail for the selected sandbox.
    fn open_ports(&mut self) -> Action {
        let Some(sbx) = self.selected_sandbox() else {
            return Action::Continue;
        };
        self.ports_state = Some(PortsState::new(&sbx.name));
        self.view = View::Dashboard;
        // Always refetch on entry: the recreate flow may have changed ports
        // since the last visit.
        self.ports_cache_dirty = true;
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

    /// Begin confirming `op` for the selected sandbox, but only if the
    /// sandbox's current state offers `action` (see
    /// [`crate::actions::available_actions`]). Unavailable actions get a
    /// status message instead of a dialog.
    fn confirm_gated(&mut self, action: CardAction, make_op: impl FnOnce(String) -> Op) -> Action {
        match self.gated_state(action) {
            Some(_) => self.confirm_named(make_op),
            None => self.reject_unavailable(action),
        }
    }

    /// The selected sandbox's state if `action` is currently available for
    /// it, `None` otherwise (also `None` when nothing is selected).
    fn gated_state(&self, action: CardAction) -> Option<&SandboxState> {
        let sbx = self.selected_sandbox()?;
        crate::actions::available_actions(&sbx.status)
            .contains(&action)
            .then_some(&sbx.status)
    }

    /// The `s` start/stop toggle applied to the selected sandbox's state:
    /// startable → [`CardAction::Start`], stoppable → [`CardAction::Stop`],
    /// `None` when nothing is selected (Docker-sbx parity).
    fn gated_state_toggle(&self) -> Option<crate::actions::CardAction> {
        let sbx = self.selected_sandbox()?;
        crate::actions::toggle_action(&sbx.status)
    }

    /// Reject a key press for an unavailable action: no dialog, a short
    /// status message explaining the current state instead.
    fn reject_unavailable(&mut self, action: CardAction) -> Action {
        let Some(sbx) = self.selected_sandbox() else {
            return Action::Continue;
        };
        let verb = match action {
            CardAction::Start => "start",
            CardAction::Stop => "stop",
            CardAction::Restart => "restart",
            CardAction::Shell => "shell",
            CardAction::Destroy => "destroy", // unreachable: always available
        };
        self.set_status(format!(
            "'{}' is {} — cannot {}",
            sbx.name, sbx.status, verb
        ));
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

    /// Set the transient status message, restarting its TTL clock.
    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status = Some(msg.into());
        self.status_age_ticks = 0;
    }

    /// Set the transient error message, restarting its TTL clock.
    pub fn set_error(&mut self, msg: impl Into<String>) {
        self.error = Some(msg.into());
        self.error_age_ticks = 0;
    }

    /// Age the transient status/error messages on each tick; returns whether
    /// one of them expired (→ re-render). While an op is in flight
    /// ([`App::busy`]), the in-progress status is kept alive (spinner).
    fn expire_transient_messages(&mut self) -> bool {
        let mut expired = false;
        if !self.busy {
            if self.status.is_some() {
                self.status_age_ticks += 1;
                if self.status_age_ticks >= STATUS_TTL_TICKS {
                    self.status = None;
                    expired = true;
                }
            }
            if self.error.is_some() {
                self.error_age_ticks += 1;
                if self.error_age_ticks >= STATUS_TTL_TICKS {
                    self.error = None;
                    expired = true;
                }
            }
        }
        expired
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
        // Prune autostart marks whose sandbox vanished from the fleet
        // (removed via `Del`, or dropped on the CLI side). A mark without
        // a sandbox is dead weight: the boot pass would only report it
        // missing. An empty list wipes every mark — a wiped DB must not
        // leave ghost marks around.
        let before = self.autostart.len();
        let known: std::collections::HashSet<&str> =
            self.sandboxes.iter().map(|s| s.name.as_str()).collect();
        self.autostart.retain(|n| known.contains(n.as_str()));
        if self.autostart.len() != before {
            self.autostart_dirty = true;
        }
    }

    /// Replace the metrics map, keyed by sandbox name.
    pub fn update_metrics(&mut self, samples: Vec<Metrics>) {
        self.metrics = samples.into_iter().map(|m| (m.name.clone(), m)).collect();
    }

    /// Take the pending interactive-shell request, if any.
    pub fn take_shell_request(&mut self) -> bool {
        std::mem::take(&mut self.shell_suspended)
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

    /// Seed the boot-autostart state (fresh read at startup). The main
    /// loop owns the file — this takes plain data, App stays I/O-free.
    pub fn init_autostart(&mut self, marked: Vec<String>, installed: bool) {
        self.autostart = marked
            .into_iter()
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .collect();
        self.autostart_installed = installed;
        self.autostart_dirty = false;
    }

    /// Toggle the boot-autostart mark for the selected sandbox (dashboard
    /// key `a`). Marks survive regardless of the sandbox's current state:
    /// marking a stopped VM is the whole point ("starts next boot").
    pub fn toggle_autostart(&mut self) -> Action {
        let Some(sbx) = self.selected_sandbox().map(|s| s.name.clone()) else {
            // Empty rail: nothing to mark, nothing to queue.
            return Action::Render;
        };
        if self.autostart.remove(&sbx) {
            self.set_status(format!("{sbx}: removed from boot autostart"));
        } else {
            self.autostart.insert(sbx.clone());
            self.set_status(format!("{sbx}: start at boot (autostart)"));
        }
        self.autostart_dirty = true;
        Action::Render
    }

    /// Consume the pending autostart change for the main loop's writer:
    /// a full-set snapshot (rapid toggles may write more than once — the
    /// file is tiny and idempotent). `None` when nothing changed since
    /// the last take.
    pub fn take_autostart_change(&mut self) -> Option<crate::autostart::AutostartChange> {
        if !self.autostart_dirty {
            return None;
        }
        self.autostart_dirty = false;
        Some(crate::autostart::AutostartChange {
            names: self.autostart.iter().cloned().collect(),
        })
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

/// Split a command line into argv on whitespace (no quoting support yet —
/// documented; the SDK exec takes argv, not a shell line).
pub(crate) fn shell_split(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_string).collect()
}

/// Whether `cred` injects for a pending create (issue #6 semantics —
/// explicit attachment ONLY): the create form selected it by name, or it
/// is scoped to the effective profile. An empty `profiles` list is
/// UNASSIGNED — never "all". The `sandboxes` scope does not gate
/// injection (the sandbox has no name yet); created names are appended
/// to the injected credentials afterwards.
fn credential_matches(
    cred: &crate::credentials::Credential,
    selected: Option<&str>,
    profile: Option<&str>,
) -> bool {
    selected.is_some_and(|sel| cred.name == sel)
        || profile.is_some_and(|p| cred.profiles.iter().any(|x| x == p))
}

/// The credentials injecting for this create, in application order (the
/// SDK applies last-wins on duplicate keys): the explicitly selected
/// credential first — when present in the file — then credentials.toml
/// order, deduped by name.
fn injection_set(
    file: &crate::credentials::CredentialsFile,
    selected: Option<&str>,
    profile: Option<&str>,
) -> Vec<crate::credentials::Credential> {
    let mut creds = file.credentials.clone();
    if let Some(sel) = selected
        && let Some(pos) = creds.iter().position(|c| c.name == sel)
    {
        let sel_cred = creds.remove(pos);
        creds.insert(0, sel_cred);
    }
    let mut out: Vec<crate::credentials::Credential> = Vec::new();
    for cred in creds {
        if !out.iter().any(|c| c.name == cred.name) && credential_matches(&cred, selected, profile)
        {
            out.push(cred);
        }
    }
    out
}

/// Outcome of [`create_with_credentials`].
#[derive(Debug)]
pub struct CreateOutcome {
    /// The created sandbox's name.
    pub name: String,
    /// Names of the credentials injected into the spec's `env`.
    pub injected: Vec<String>,
    /// Non-fatal post-create bookkeeping error: the sandbox exists, but
    /// recording it in a credential's `sandboxes` scope failed.
    pub record_error: Option<String>,
    /// The form-selected credential vanished from the file while the form
    /// was open: nothing was injected for it, reported loudly (the missing
    /// profile case errors out; this one still proceeds).
    pub stale_selection: Option<String>,
}

/// The create task's pipeline (issues #6/#7): resolve credential
/// injection strictly BEFORE the create and fail closed — a metadata
/// (`credentials.toml`) or keyring error aborts with no sandbox, never a
/// secretless create. Each resolved secret is appended to `spec.env`
/// AFTER the spec's own entries (the SDK applies last-wins on duplicate
/// keys, so credentials override profile/form values). After the create,
/// the sandbox name is appended to every injected credential's
/// `sandboxes` scope (persist error is non-fatal and comes back as
/// [`CreateOutcome::record_error`]).
pub async fn create_with_credentials(
    backend: &dyn crate::backend::MsbBackend,
    spec: crate::backend::CreateSpec,
    selected: Option<&str>,
    profile: Option<&str>,
    file: &crate::credentials::CredentialsFile,
) -> anyhow::Result<CreateOutcome> {
    let set = injection_set(file, selected, profile);
    // Loud stale-selection policy: a name the form armed but the file no
    // longer contains must not fail the create, but must not pass silently
    // either (the matching profile case errors out outright).
    let stale_selection =
        selected.and_then(|sel| (!set.iter().any(|c| c.name == sel)).then(|| sel.to_string()));
    let mut spec = spec;
    let mut resolved = Vec::with_capacity(set.len());
    for cred in &set {
        // Fail closed: an explicitly assigned credential with no keyring
        // secret (or an unreadable metadata file) is an error, never a
        // secretless create.
        let secret = crate::credentials::keyring_get(&cred.name).await?;
        resolved.push((cred.name.clone(), cred.env_key.clone(), secret));
    }
    let injected = apply_credentials(&mut spec, &resolved);
    let name = crate::actions::create_sandbox(backend, &spec).await?;
    if !injected.is_empty() {
        // Fresh read: the file may have been edited while the create ran.
        let record_error = record_credential_sandboxes(&injected, &name)
            .err()
            .map(|e| format!("{e:#}"));
        return Ok(CreateOutcome {
            name,
            injected,
            record_error,
            stale_selection,
        });
    }
    Ok(CreateOutcome {
        name,
        injected,
        record_error: None,
        stale_selection,
    })
}

/// Append resolved credential entries to the spec's env, after the spec's
/// own entries, in `resolved` order — the SDK applies last-wins on
/// duplicate keys, so credentials override profile/form values. Returns
/// the injected credential names.
fn apply_credentials(
    spec: &mut crate::backend::CreateSpec,
    resolved: &[(String, String, String)],
) -> Vec<String> {
    let mut injected = Vec::with_capacity(resolved.len());
    for (name, env_key, secret) in resolved {
        spec.env
            .push(crate::credentials::secret_env_entry(env_key, secret));
        injected.push(name.clone());
    }
    injected
}

/// Append the created sandbox's name to every injected credential's
/// `sandboxes` scope and persist the credential metadata.
fn record_credential_sandboxes(injected: &[String], sandbox: &str) -> anyhow::Result<()> {
    let mut file = crate::credentials::load_credentials()?;
    if mark_sandboxes_in_file(&mut file, injected, sandbox) {
        crate::credentials::save_credentials(&file)?;
    }
    Ok(())
}

/// Append `sandbox` to every credential in `injected`'s `sandboxes`
/// scope, deduped by name. Returns whether anything changed (no change →
/// the caller skips the persist).
fn mark_sandboxes_in_file(
    file: &mut crate::credentials::CredentialsFile,
    injected: &[String],
    sandbox: &str,
) -> bool {
    let mut changed = false;
    for cred in &mut file.credentials {
        if injected.iter().any(|name| name == &cred.name)
            && !cred.sandboxes.iter().any(|s| s == sandbox)
        {
            cred.sandboxes.push(sandbox.to_string());
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::LogLine;
    use crate::models::SandboxState;
    use crate::ui::create::CreateForm;
    use chrono::{DateTime, Utc};

    fn summary(name: &str) -> SandboxSummary {
        summary_state(name, SandboxState::Running)
    }

    fn summary_state(name: &str, state: SandboxState) -> SandboxSummary {
        SandboxSummary {
            created_at: DateTime::<Utc>::UNIX_EPOCH,
            image: "alpine".into(),
            name: name.into(),
            status: state,
            workdir: Some("/app".into()),
            mounts: Vec::new(),
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

    /// A credential metadata row for the injection tests.
    fn credential(name: &str, env_key: &str, profiles: &[&str]) -> crate::credentials::Credential {
        crate::credentials::Credential {
            name: name.to_string(),
            description: "test".to_string(),
            env_key: env_key.to_string(),
            profiles: profiles.iter().map(|p| (*p).to_string()).collect(),
            sandboxes: Vec::new(),
        }
    }

    fn key(code: KeyCode) -> AppEvent {
        AppEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    /// Raw key event for form-level handlers.
    fn key_event(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
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

    // -- template select state (create rework) --

    fn app_with_templates() -> App {
        let mut app = App::new();
        app.templates = crate::template::load_builtins();
        app
    }

    #[test]
    fn c_key_opens_template_select() {
        let mut app = App::new();
        assert_eq!(app.handle_event(key(KeyCode::Char('c'))), Action::Render);
        assert_eq!(app.view, View::Create);
        assert!(matches!(
            app.create_state,
            Some(CreateState::Select { selected: 0 })
        ));
    }

    #[test]
    fn select_enter_queues_create_with_resolved_name() {
        let mut app = app_with_templates();
        // A sandbox named "shell" already exists → auto suffix.
        app.sandboxes = vec![summary("shell")];
        app.view = View::Create;
        let shell = app.templates.iter().position(|t| t.id == "shell").unwrap() + 1;
        app.create_state = Some(CreateState::Select { selected: shell });
        assert_eq!(app.handle_event(key(KeyCode::Enter)), Action::Render);
        let spec = app.queued_create.take().unwrap();
        assert_eq!(spec.name.as_deref(), Some("shell-2"));
        assert!(app.create_state.is_none());
        assert_eq!(app.view, View::Dashboard);
        assert!(app.busy);
    }

    #[test]
    fn select_enter_while_busy_ignored() {
        // Review Focus 3: kein zweites Create, solange eines läuft.
        let mut app = app_with_templates();
        app.busy = true;
        app.view = View::Create;
        let shell = app.templates.iter().position(|t| t.id == "shell").unwrap() + 1;
        app.create_state = Some(CreateState::Select { selected: shell });
        assert_eq!(app.handle_event(key(KeyCode::Enter)), Action::Continue);
        assert!(app.queued_create.is_none());
    }

    #[test]
    fn select_e_opens_prefilled_form() {
        let mut app = app_with_templates();
        app.view = View::Create;
        let opencode = app
            .templates
            .iter()
            .position(|t| t.id == "opencode")
            .unwrap()
            + 1; // + 1 scratch row
        app.create_state = Some(CreateState::Select { selected: opencode });
        assert_eq!(app.handle_event(key(KeyCode::Char('e'))), Action::Render);
        let Some(CreateState::Form(form)) = &app.create_state else {
            panic!("expected Form state");
        };
        assert_eq!(form.image, "sbx/opencode-image");
        assert_eq!(form.name, "opencode");
    }

    #[test]
    fn select_enter_on_scratch_opens_empty_form() {
        // The picker's first row is "Create from scratch": Enter opens the
        // empty form (NOT a create — there is nothing to create yet).
        let mut app = app_with_templates();
        app.view = View::Create;
        app.create_state = Some(CreateState::Select { selected: 0 });
        assert_eq!(app.handle_event(key(KeyCode::Enter)), Action::Render);
        let Some(CreateState::Form(form)) = &app.create_state else {
            panic!("expected Form state");
        };
        assert!(form.image.is_empty(), "scratch starts with no image");
        assert!(form.mount_cwd);
        assert!(app.queued_create.is_none());
        assert!(!app.busy);
        assert_eq!(app.view, View::Create);
    }

    #[test]
    fn select_e_on_scratch_opens_empty_form_seeded_with_images() {
        // Like `n`, the scratch path must see the cached image list (sizes
        // in the form's picker).
        let mut app = app_with_templates();
        app.images = vec![crate::models::Image {
            architecture: "amd64".into(),
            created_at: chrono::Utc::now(),
            digest: "sha256:x".into(),
            layer_count: 1,
            os: "linux".into(),
            reference: "alpine".into(),
            size_bytes: 1024,
        }];
        app.view = View::Create;
        app.create_state = Some(CreateState::Select { selected: 0 });
        assert_eq!(app.handle_event(key(KeyCode::Char('e'))), Action::Render);
        let Some(CreateState::Form(form)) = &app.create_state else {
            panic!("expected Form state");
        };
        assert!(form.image.is_empty());
        assert_eq!(form.images.len(), 1, "cached images seeded like `n`");
    }

    #[test]
    fn select_d_on_scratch_is_rejected() {
        // The scratch entry is not a template — nothing to delete.
        let mut app = app_with_templates();
        app.view = View::Create;
        app.create_state = Some(CreateState::Select { selected: 0 });
        app.handle_event(key(KeyCode::Char('d')));
        assert!(app.confirm.is_none());
        assert!(matches!(
            app.create_state,
            Some(CreateState::Select { selected: 0 })
        ));
    }

    #[test]
    fn template_rows_shift_past_scratch() {
        // Row 1 is the FIRST template (shell leads the built-ins); Enter
        // on it creates directly, exactly like the pre-scratch row 0 did.
        let mut app = app_with_templates();
        app.view = View::Create;
        let shell = app.templates.iter().position(|t| t.id == "shell").unwrap() + 1;
        app.create_state = Some(CreateState::Select { selected: shell });
        assert_eq!(app.handle_event(key(KeyCode::Enter)), Action::Render);
        let spec = app.queued_create.take().unwrap();
        let shell_tpl = app.templates.iter().find(|t| t.id == "shell").unwrap();
        assert_eq!(spec.image, shell_tpl.spec.image);
        assert_eq!(spec.name.as_deref(), Some("shell"));
    }

    #[test]
    fn down_and_up_navigate_all_picker_rows() {
        // Regression: der Scratch-Zweig (Zeile 0) schluckte Up/Down — der
        // Picker öffnet auf Zeile 0 und war damit eingesperrt.
        let mut app = app_with_templates();
        app.view = View::Create;
        app.create_state = Some(CreateState::Select { selected: 0 });
        app.handle_event(key(KeyCode::Down));
        assert!(
            matches!(app.create_state, Some(CreateState::Select { selected: 1 })),
            "Down leaves the scratch row"
        );
        let last = app.templates.len(); // scratch + templates → last row
        for _ in 0..last {
            app.handle_event(key(KeyCode::Down));
        }
        assert!(matches!(
            app.create_state,
            Some(CreateState::Select { selected: s }) if s == last
        ));
        // Two Ups land two rows above the last (clamped at 0 further up).
        app.handle_event(key(KeyCode::Up));
        app.handle_event(key(KeyCode::Up));
        assert!(matches!(
            app.create_state,
            Some(CreateState::Select { selected: s }) if s == last - 2
        ));
    }

    #[test]
    fn scratch_row_works_with_zero_templates() {
        // The scratch pseudo-row makes the picker usable with no templates
        // at all: Enter opens the empty form, not the dashboard.
        let mut app = App::new(); // no templates loaded
        app.view = View::Create;
        app.create_state = Some(CreateState::Select { selected: 0 });
        assert_eq!(app.handle_event(key(KeyCode::Enter)), Action::Render);
        assert!(matches!(app.create_state, Some(CreateState::Form(_))));
        assert_eq!(app.view, View::Create);
    }

    #[test]
    fn select_n_opens_empty_form() {
        let mut app = app_with_templates();
        app.view = View::Create;
        app.create_state = Some(CreateState::Select { selected: 0 });
        assert_eq!(app.handle_event(key(KeyCode::Char('n'))), Action::Render);
        let Some(CreateState::Form(form)) = &app.create_state else {
            panic!("expected Form state");
        };
        assert!(form.image.is_empty());
        assert!(form.mount_cwd);
    }

    #[test]
    fn form_esc_returns_to_select_not_dashboard() {
        let mut app = app_with_templates();
        app.view = View::Create;
        app.create_state = Some(CreateState::Form(Box::new(CreateForm::from_template(
            &app.templates[0],
            "/tmp".into(),
        ))));
        assert_eq!(app.handle_event(key(KeyCode::Esc)), Action::Render);
        assert_eq!(app.view, View::Create);
        assert!(matches!(app.create_state, Some(CreateState::Select { .. })));
    }

    #[test]
    fn select_d_on_builtin_shows_hint_only() {
        let mut app = app_with_templates();
        app.view = View::Create;
        let shell = app.templates.iter().position(|t| t.id == "shell").unwrap() + 1;
        app.create_state = Some(CreateState::Select { selected: shell }); // built-in
        assert_eq!(app.handle_event(key(KeyCode::Char('d'))), Action::Render);
        assert!(app.confirm.is_none());
        assert!(
            app.status
                .as_deref()
                .unwrap_or_default()
                .contains("built-in")
        );
    }

    #[test]
    fn select_d_on_user_template_asks_confirm() {
        let mut app = app_with_templates();
        let mut t = crate::template::parse(
            "mine",
            "[meta]\nname = \"Mine\"\ndescription = \"d\"\n\n[spec]\nimage = \"alpine\"\n",
        )
        .unwrap();
        t.id = "mine".into();
        app.templates.insert(0, t);
        app.view = View::Create;
        app.create_state = Some(CreateState::Select { selected: 1 }); // user template (after scratch)
        assert_eq!(app.handle_event(key(KeyCode::Char('d'))), Action::Render);
        assert!(matches!(
            app.confirm.as_ref(),
            Some(Op::DeleteTemplate(id)) if id == "mine"
        ));
    }

    #[test]
    fn save_dialog_asks_on_collision() {
        // Review Focus 4: existierender Dateiname → Confirm, kein stilles
        // Überschreiben. Der Dialog emittiert SaveTemplate; die App
        // entscheidet anhand der geladenen Templates (id = Slug).
        let mut app = app_with_templates();
        app.view = View::Create;
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        form.open_save_dialog("Shell".into(), "eigene Shell-Variante".into());
        assert!(form.save_dialog.is_some());
        // "Shell" → slug "shell" → kollidiert mit dem Built-in. Ein Enter
        // genügt: App → Form (Dialog offen) → SaveTemplate.
        app.create_state = Some(CreateState::Form(Box::new(form)));
        assert_eq!(app.handle_event(key(KeyCode::Enter)), Action::Render);
        // take_save_template bleibt leer — stattdessen Confirm-Dialog.
        assert!(app.take_save_template.is_none());
        assert!(matches!(app.confirm, Some(Op::OverwriteTemplate(_))));
    }

    #[test]
    fn save_without_collision_goes_straight_to_disk() {
        let mut app = app_with_templates();
        app.view = View::Create;
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        form.open_save_dialog("Meine Shell".into(), "d".into());
        app.create_state = Some(CreateState::Form(Box::new(form)));
        assert_eq!(app.handle_event(key(KeyCode::Enter)), Action::Render);
        assert!(app.confirm.is_none());
        assert_eq!(
            app.take_save_template
                .as_ref()
                .map(|t| t.meta.name.as_str()),
            Some("Meine Shell")
        );
    }

    #[test]
    fn smoke_picker_and_form_render_offscreen() {
        // Throwaway render smoke (UI-Rendering ist nicht unit-getestet;
        // hier geht es nur um Panics in den Render-Pfäden mit echten
        // Templates — Master-Detail, Stacked, ListOnly, Formular, Dialog).
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        fn render_app(app: &App, w: u16, h: u16) {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal
                .draw(|f| crate::render_view(f, app, f.area(), None))
                .unwrap();
        }

        let mut app = app_with_templates();

        // Picker: master-detail (≥72/≥20), stacked (<72), list-only (<20).
        app.view = View::Create;
        app.create_state = Some(CreateState::Select { selected: 0 });
        render_app(&app, 120, 34);
        render_app(&app, 60, 24);
        render_app(&app, 80, 18);

        // Leere Template-Liste rendert (Hinweis-Zeile).
        app.templates.clear();
        render_app(&app, 120, 34);

        // Gruppiertes Formular mit Picker + Dialog.
        app.templates = crate::template::load_builtins();
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        app.create_state = Some(CreateState::Form(Box::new(form)));
        render_app(&app, 120, 34);
        render_app(&app, 50, 22);

        let mut form = CreateForm::new(Vec::new());
        form.open_save_dialog("Test".into(), "Beschreibung".into());
        app.create_state = Some(CreateState::Form(Box::new(form)));
        render_app(&app, 120, 34);
    }

    #[test]
    fn templates_loaded_clamps_select_index() {
        // C2: list shrank (delete/reload) → the Select index must clamp,
        // otherwise the next Enter/e/d indexes out of bounds. Rows:
        // scratch (0) + one template (1) — a stale 2 clamps to 1.
        let mut app = app_with_templates();
        app.view = View::Create;
        app.create_state = Some(CreateState::Select { selected: 2 });
        let one = vec![app.templates[0].clone()];
        app.handle_event(crate::event::AppEvent::TemplatesLoaded(one));
        assert!(matches!(
            app.create_state,
            Some(CreateState::Select { selected: 1 })
        ));
    }

    #[test]
    fn template_select_guards_stale_index() {
        // C2 (belt): a stale index never panics — Enter is a no-op with a
        // status hint instead of an index-out-of-bounds.
        let mut app = app_with_templates();
        app.view = View::Create;
        app.create_state = Some(CreateState::Select { selected: 500 });
        app.handle_event(key(KeyCode::Enter));
        assert!(app.queued_create.is_none());
        app.create_state = Some(CreateState::Select { selected: 500 });
        app.handle_event(key(KeyCode::Char('e')));
        app.create_state = Some(CreateState::Select { selected: 500 });
        app.handle_event(key(KeyCode::Char('d')));
    }

    #[test]
    fn e_path_seeds_cached_images() {
        // I3: the e-path form must see the cached image list (sizes in the
        // picker), like `n` does.
        let mut app = app_with_templates();
        app.images = vec![crate::models::Image {
            architecture: "amd64".into(),
            created_at: chrono::Utc::now(),
            digest: "sha256:x".into(),
            layer_count: 1,
            os: "linux".into(),
            reference: "alpine".into(),
            size_bytes: 1024,
        }];
        app.view = View::Create;
        app.create_state = Some(CreateState::Select { selected: 0 });
        app.handle_event(key(KeyCode::Char('e')));
        let Some(CreateState::Form(form)) = &app.create_state else {
            panic!("expected Form");
        };
        assert_eq!(form.images.len(), 1);
    }

    #[test]
    fn templates_loaded_replaces_list() {
        let mut app = App::new();
        let ts = crate::template::load_builtins();
        assert_eq!(
            app.handle_event(crate::event::AppEvent::TemplatesLoaded(ts.clone())),
            Action::Render
        );
        assert_eq!(app.templates.len(), ts.len());
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
        for view in [View::Create, View::Help, View::Environments] {
            let mut app = App::new();
            app.update_sandboxes(vec![summary("a")]);
            app.view = view;
            assert_eq!(app.handle_event(key(KeyCode::Esc)), Action::Render);
            assert_eq!(app.view, View::Dashboard);
        }
        // Logs/Ports/Inspect are detail tabs in the 2.9 IA — Esc on the
        // dashboard is a no-op (there is no separate view to leave).
    }

    #[test]
    fn esc_on_dashboard_is_a_noop() {
        let mut app = App::new();
        assert_eq!(app.handle_event(key(KeyCode::Esc)), Action::Continue);
        assert_eq!(app.view, View::Dashboard);
    }

    #[test]
    fn detail_tabs_need_a_selection_but_overview_is_free() {
        let mut app = App::new();

        // Empty list: Enter switches to Overview (always allowed).
        assert_eq!(app.handle_event(key(KeyCode::Enter)), Action::Render);
        assert_eq!(app.detail, DetailTab::Overview);
        // l/p with no sandbox fall through to open_logs/open_ports → Continue.
        assert_eq!(app.handle_event(key(KeyCode::Char('l'))), Action::Continue);
        assert_eq!(app.handle_event(key(KeyCode::Char('p'))), Action::Continue);
        assert_eq!(app.view, View::Dashboard);
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

        // `s` (the sbx start/stop toggle) opens the confirm dialog for a
        // running sandbox; the op is not queued yet.
        assert_eq!(app.handle_event(key(KeyCode::Char('s'))), Action::Render);
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
    fn x_requests_interactive_shell_with_selection() {
        let mut app = App::new();
        // Empty list: x is a no-op.
        assert_eq!(app.handle_event(key(KeyCode::Char('x'))), Action::Render);
        assert!(!app.shell_suspended);

        app.update_sandboxes(vec![summary("api")]);
        assert_eq!(app.handle_event(key(KeyCode::Char('x'))), Action::Render);
        assert!(app.shell_suspended, "x requests the shell window");
        assert!(app.take_shell_request(), "request consumed once");
        assert!(!app.shell_suspended);
    }

    #[test]
    fn s_and_s_both_start_a_stopped_sandbox() {
        let mut app = App::new();
        app.update_sandboxes(vec![summary_state("api", SandboxState::Stopped)]);
        for k in ['s', 'S'] {
            app.handle_event(key(KeyCode::Char(k)));
            let op = app.confirm.take().expect("s opens the start confirm");
            assert_eq!(op, Op::Start("api".into()), "key {k}");
        }
    }

    // ---- root-disk resize (`g`) ----

    #[test]
    fn g_opens_size_prompt_and_enter_confirms_resize() {
        let mut app = App::new();
        app.update_sandboxes(vec![summary("web")]);
        // 16G typed through the prompt → confirm dialog carries the op.
        assert_eq!(app.handle_event(key(KeyCode::Char('g'))), Action::Render);
        let name = app
            .resize_prompt
            .as_ref()
            .expect("prompt open")
            .name
            .clone();
        assert_eq!(name, "web");
        app.handle_event(key(KeyCode::Char('1')));
        app.handle_event(key(KeyCode::Char('6')));
        app.handle_event(key(KeyCode::Char('G')));
        app.handle_event(key(KeyCode::Enter));
        assert!(app.resize_prompt.is_none(), "prompt closed on submit");
        assert_eq!(
            app.confirm,
            Some(Op::ResizeDisk {
                name: "web".into(),
                mib: 16 * 1024
            })
        );
    }

    #[test]
    fn resize_prompt_rejects_bad_size_and_esc_cancels() {
        let mut app = App::new();
        app.update_sandboxes(vec![summary("web")]);
        app.handle_event(key(KeyCode::Char('g')));
        // Bad size: prompt stays open with an error line.
        app.handle_event(key(KeyCode::Char('h')));
        app.handle_event(key(KeyCode::Enter));
        assert!(app.resize_prompt.is_some(), "prompt stays open");
        assert!(app.confirm.is_none());
        assert!(
            app.error
                .as_deref()
                .unwrap_or_default()
                .contains("Disk size")
        );
        // Esc cancels cleanly.
        app.handle_event(key(KeyCode::Esc));
        assert!(app.resize_prompt.is_none());
        assert!(app.confirm.is_none());
    }

    // ---- state-gated lifecycle keys (Docker-sbx parity) ----

    #[test]
    fn s_toggle_rejects_unavailable_start_on_unknown_state() {
        let mut app = App::new();
        // Unknown state: neither start nor stop applies.
        app.update_sandboxes(vec![summary_state(
            "web",
            SandboxState::Unknown("weird".into()),
        )]);
        assert_eq!(app.handle_event(key(KeyCode::Char('s'))), Action::Render);
        assert!(app.confirm.is_none(), "no confirm for unknown state");
        assert!(
            app.status.as_deref().unwrap_or_default().contains("weird"),
            "status explains why: {status:?}",
            status = app.status
        );
    }

    #[test]
    fn s_toggle_stops_stoppable_states() {
        // Paused/Running/Stalled: `s` toggles to Stop confirm.
        for state in [
            SandboxState::Running,
            SandboxState::Paused,
            SandboxState::Stalled,
        ] {
            let mut app = App::new();
            app.update_sandboxes(vec![summary_state("db", state.clone())]);
            app.handle_event(key(KeyCode::Char('s')));
            assert_eq!(app.confirm, Some(Op::Stop("db".into())), "state: {state}");
        }
    }

    #[test]
    fn restart_key_is_gated_to_running_and_stalled() {
        let mut app = App::new();
        app.update_sandboxes(vec![summary_state("web", SandboxState::Stopped)]);
        app.handle_event(key(KeyCode::Char('r')));
        assert!(app.confirm.is_none(), "no restart confirm for stopped");

        let mut app = App::new();
        app.update_sandboxes(vec![summary_state("web", SandboxState::Running)]);
        app.handle_event(key(KeyCode::Char('r')));
        assert_eq!(app.confirm, Some(Op::Restart("web".into())));
    }

    #[test]
    fn shell_key_is_gated_to_running() {
        let mut app = App::new();
        app.update_sandboxes(vec![summary_state("api", SandboxState::Stopped)]);
        assert_eq!(app.handle_event(key(KeyCode::Char('x'))), Action::Render);
        assert!(!app.shell_suspended, "no shell request for stopped sandbox");

        let mut app = App::new();
        app.update_sandboxes(vec![summary_state("api", SandboxState::Running)]);
        app.handle_event(key(KeyCode::Char('x')));
        assert!(app.shell_suspended);
    }

    #[test]
    fn destroy_key_is_always_available() {
        for state in [
            SandboxState::Running,
            SandboxState::Stopped,
            SandboxState::Paused,
            SandboxState::Stalled,
            SandboxState::Unknown("weird".into()),
        ] {
            let mut app = App::new();
            app.update_sandboxes(vec![summary_state("old", state.clone())]);
            app.handle_event(key(KeyCode::Delete));
            assert_eq!(
                app.confirm,
                Some(Op::Remove("old".into())),
                "state: {state}"
            );
        }
    }

    #[test]
    fn lifecycle_keys_require_a_selection() {
        let mut app = App::new();
        // `r` and `s` are no-ops (Continue) on an empty list.
        for k in ['r', 's'] {
            app.view = View::Dashboard;
            assert_eq!(app.handle_event(key(KeyCode::Char(k))), Action::Continue);
            assert!(
                app.confirm.is_none(),
                "key {k} must not open confirm on empty list"
            );
        }
        // `x` (shell) renders but queues nothing on an empty list.
        assert_eq!(app.handle_event(key(KeyCode::Char('x'))), Action::Render);
        assert!(!app.shell_suspended);
        // Delete on an empty list is also a no-op.
        assert_eq!(app.handle_event(key(KeyCode::Delete)), Action::Continue);

        // U works even with an empty list (runtime may be missing entirely).
        assert_eq!(app.handle_event(key(KeyCode::Char('U'))), Action::Render);
        assert_eq!(app.confirm, Some(Op::InstallRuntime));
    }

    // ---- self-update flow (startup check → banner → V key → confirm) ----

    fn update_info() -> crate::selfupdate::UpdateInfo {
        crate::selfupdate::UpdateInfo {
            current: semver::Version::new(0, 2, 0),
            latest: semver::Version::new(0, 2, 1),
            tarball_url: "https://example.com/t.tar.gz".into(),
            sha256_url: "https://example.com/t.tar.gz.sha256".into(),
        }
    }

    #[test]
    fn update_available_event_sets_the_prompt_state() {
        let mut app = App::new();
        assert!(app.update.is_none());
        assert_eq!(
            app.handle_event(AppEvent::UpdateAvailable(update_info())),
            Action::Render
        );
        assert!(app.update.is_some(), "the update drives banner + V key");
    }

    #[test]
    fn v_key_opens_the_update_confirm_only_when_an_update_is_known() {
        let mut app = App::new();
        app.view = View::Dashboard;
        // No update detected yet: inert.
        assert_eq!(app.handle_event(key(KeyCode::Char('v'))), Action::Render);
        assert!(app.confirm.is_none());
        // Caps parity with s/S and a/A.
        app.handle_event(AppEvent::UpdateAvailable(update_info()));
        assert_eq!(app.handle_event(key(KeyCode::Char('V'))), Action::Render);
        assert!(
            matches!(app.confirm, Some(Op::UpdateSelf(_))),
            "V opens the update confirm: {:?}",
            app.confirm
        );
    }

    #[test]
    fn confirming_the_update_queues_the_op_for_the_main_loop() {
        let mut app = App::new();
        app.handle_event(AppEvent::UpdateAvailable(update_info()));
        app.handle_event(key(KeyCode::Char('v')));
        app.handle_event(key(KeyCode::Char('y')));
        let op = app.take_op().expect("queued op");
        match op {
            Op::UpdateSelf(u) => {
                assert_eq!(u.latest, semver::Version::new(0, 2, 1));
                assert_eq!(u.current, semver::Version::new(0, 2, 0));
            }
            other => panic!("expected UpdateSelf, got {other:?}"),
        }
        assert!(app.busy, "Working… until the update task reports back");
    }

    #[test]
    fn update_installed_clears_the_prompt_and_reports_relaunch() {
        let mut app = App::new();
        app.handle_event(AppEvent::UpdateAvailable(update_info()));
        app.busy = true;
        app.handle_event(AppEvent::UpdateInstalled("0.2.1".into()));
        assert!(app.update.is_none(), "no stale banner after the swap");
        assert!(!app.busy);
        let status = app.status.clone().unwrap_or_default();
        assert!(
            status.contains("0.2.1") && status.contains("relaunch"),
            "status must tell the user to relaunch: {status}"
        );
    }

    #[test]
    fn opdone_marks_ports_cache_dirty_for_refetch() {
        // Publish/unpublish recreate sandboxes → ports must be refetched.
        let mut app = App::new();
        app.ports_cache_dirty = false;
        app.busy = true;
        app.handle_event(AppEvent::OpDone("published 8888:80 on Test".into()));
        assert!(
            app.ports_cache_dirty,
            "OpDone must mark the ports cache dirty"
        );
    }

    // ---- ports fetch policy (pure; smoke finding 2026-09-24) ----

    #[test]
    fn ports_fetch_triggers_on_ports_tab_and_initial_list() {
        // Entering the PORTS detail tab marks the cache dirty → fetch.
        let mut app = App::new();
        app.update_sandboxes(vec![summary("a")]);
        app.detail = DetailTab::Ports;
        app.ports_state = None;
        app.ports_primed = true;
        app.ports_cache_dirty = true; // set by open_ports()
        assert!(
            ports_fetch_needed(&app),
            "ports tab entry must trigger a fetch"
        );

        // The first list load primes the cache too (cards show ports even
        // before the tab is ever opened) — only once.
        let mut app = App::new();
        app.update_sandboxes(vec![summary("a")]);
        app.detail = DetailTab::Overview;
        app.ports_cache_dirty = false;
        app.ports_primed = false;
        assert!(
            ports_fetch_needed(&app),
            "initial list load must prime the ports cache"
        );
        // Once primed and clean: no more fetches.
        app.ports_primed = true;
        assert!(
            !ports_fetch_needed(&app),
            "primed cache must not refetch on every loop"
        );
    }

    #[test]
    fn ports_fetch_skips_empty_list_and_already_primed() {
        // Nothing to fetch with no sandboxes.
        let app = App::new();
        assert!(!ports_fetch_needed(&app));

        // Already primed and clean: no fetch.
        let mut app = App::new();
        app.update_sandboxes(vec![summary("a")]);
        app.detail = DetailTab::Overview;
        app.ports_cache_dirty = false;
        app.ports_primed = true;
        assert!(!ports_fetch_needed(&app));
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

    // ---- transient status/error auto-clear (smoke finding 2026-09-24) ----

    #[test]
    fn status_expires_after_timeout() {
        let mut app = App::new();
        app.handle_event(AppEvent::OpDone("published 8888:80 on Test".into()));
        assert!(app.status.is_some());

        // Ticks before the timeout keep the message.
        for _ in 0..(STATUS_TTL_TICKS - 1) {
            app.handle_event(AppEvent::Tick);
        }
        assert!(
            app.status.is_some(),
            "status must survive until the TTL expires"
        );

        // The TTL-th tick clears it (250ms cadence → ~5s).
        app.handle_event(AppEvent::Tick);
        assert!(app.status.is_none(), "status must expire after the TTL");
    }

    #[test]
    fn error_expires_after_timeout() {
        let mut app = App::new();
        app.handle_event(AppEvent::Error("something failed".into()));
        assert!(app.error.is_some());
        for _ in 0..STATUS_TTL_TICKS {
            app.handle_event(AppEvent::Tick);
        }
        assert!(app.error.is_none(), "error must expire after the TTL");
    }

    #[test]
    fn busy_spinner_keeps_working_status_alive() {
        let mut app = App::new();
        app.busy = true;
        app.status = Some("Working…".into());
        // While an op is in flight, ticks animate the spinner and must NOT
        // expire the in-progress status.
        for _ in 0..(STATUS_TTL_TICKS + 5) {
            app.handle_event(AppEvent::Tick);
        }
        assert_eq!(app.status.as_deref(), Some("Working…"));
        // OpDone replaces it and restarts the TTL clock.
        app.busy = false;
        app.handle_event(AppEvent::OpDone("done".into()));
        assert_eq!(app.status.as_deref(), Some("done"));
        for _ in 0..STATUS_TTL_TICKS {
            app.handle_event(AppEvent::Tick);
        }
        assert!(app.status.is_none());
    }

    #[test]
    fn new_status_restarts_the_ttl() {
        let mut app = App::new();
        app.handle_event(AppEvent::OpDone("first".into()));
        for _ in 0..(STATUS_TTL_TICKS - 2) {
            app.handle_event(AppEvent::Tick);
        }
        // A new message arrives before the old one expires → full new TTL.
        app.handle_event(AppEvent::OpDone("second".into()));
        for _ in 0..(STATUS_TTL_TICKS - 1) {
            app.handle_event(AppEvent::Tick);
        }
        assert_eq!(app.status.as_deref(), Some("second"));
        app.handle_event(AppEvent::Tick);
        assert!(app.status.is_none());
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

    #[test]
    fn logs_detail_tab_routes_keys_to_the_logs_widget() {
        // Key `2` opens the LOGS detail tab and creates its state.
        let mut app = App::new();
        app.update_sandboxes(vec![summary("web")]);
        assert_eq!(app.handle_event(key(KeyCode::Char('2'))), Action::Render);
        assert_eq!(app.detail, DetailTab::Logs);
        assert!(app.logs_state.is_some());

        // The dashboard handler must not swallow the widget's keys (same
        // class of bug as the Phase 2.9 ports dispatch): `f` toggles
        // follow, `s` cycles the source filter instead of falling through
        // to the dashboard's start/stop confirm, and `g` enters the
        // widget's grep mode.
        assert_eq!(app.handle_event(key(KeyCode::Char('f'))), Action::Render);
        assert!(!app.logs_state.as_ref().unwrap().follow);

        assert_eq!(app.handle_event(key(KeyCode::Char('s'))), Action::Render);
        assert!(app.confirm.is_none());
        assert_eq!(
            app.logs_state.as_ref().unwrap().source_filter.as_deref(),
            Some("stdout")
        );

        assert_eq!(app.handle_event(key(KeyCode::Char('g'))), Action::Render);
        assert!(app.logs_state.as_ref().unwrap().grep_mode);
    }

    // ---- boot autostart (dashboard `a` toggle) ----

    #[test]
    fn a_key_marks_selected_sandbox_for_boot_autostart() {
        let mut app = App::new();
        app.update_sandboxes(vec![summary("claude"), summary("devin")]);
        app.handle_event(key(KeyCode::Char('a')));
        assert!(app.autostart.contains("claude"), "{:?}", app.autostart);
        assert_eq!(app.autostart.len(), 1);
        assert_eq!(
            app.take_autostart_change(),
            Some(crate::autostart::AutostartChange {
                names: vec!["claude".to_string()]
            }),
            "the full new set is queued for the main loop's writer"
        );
        assert!(app.take_autostart_change().is_none(), "consumed once");
        assert!(
            app.status.as_deref().unwrap_or("").contains("claude"),
            "status names the toggle: {:?}",
            app.status
        );
    }

    #[test]
    fn a_key_toggles_the_mark_off_again() {
        let mut app = App::new();
        app.update_sandboxes(vec![summary("claude")]);
        app.handle_event(key(KeyCode::Char('a')));
        app.take_autostart_change();
        app.handle_event(key(KeyCode::Char('a')));
        assert!(!app.autostart.contains("claude"), "{:?}", app.autostart);
        assert_eq!(
            app.take_autostart_change(),
            Some(crate::autostart::AutostartChange { names: vec![] }),
            "empty set is persisted too"
        );
    }

    #[test]
    fn a_key_without_selection_is_a_no_op() {
        let mut app = App::new();
        app.handle_event(key(KeyCode::Char('a')));
        assert!(app.autostart.is_empty());
        assert!(app.take_autostart_change().is_none());
    }

    #[test]
    fn uppercase_a_toggles_too() {
        // Caps/Shift must not hide the feature (s/S parity).
        let mut app = App::new();
        app.update_sandboxes(vec![summary("claude")]);
        app.handle_event(key(KeyCode::Char('A')));
        assert!(app.autostart.contains("claude"), "{:?}", app.autostart);
        app.handle_event(key(KeyCode::Char('A')));
        assert!(!app.autostart.contains("claude"), "{:?}", app.autostart);
    }

    #[test]
    fn init_autostart_seeds_marks_and_install_flag() {
        let mut app = App::new();
        assert!(app.autostart.is_empty());
        assert!(!app.autostart_installed);
        app.init_autostart(vec!["claude".into()], false);
        assert_eq!(
            app.autostart.iter().cloned().collect::<Vec<_>>(),
            vec!["claude".to_string()]
        );
        assert!(!app.autostart_installed);
        // Re-init (the main loop re-reads at start) replaces the set.
        app.init_autostart(vec![], true);
        assert!(app.autostart.is_empty(), "{:?}", app.autostart);
        assert!(app.autostart_installed);
    }

    #[test]
    fn update_sandboxes_prunes_marks_of_removed_sandboxes() {
        let mut app = App::new();
        app.init_autostart(vec!["a".into(), "gone".into(), "keep".into()], true);
        app.update_sandboxes(vec![summary("a"), summary("keep")]);
        assert!(
            app.autostart.contains("a") && app.autostart.contains("keep"),
            "{:?}",
            app.autostart
        );
        assert!(
            !app.autostart.contains("gone"),
            "mark of a removed sandbox goes"
        );
        let change = app
            .take_autostart_change()
            .expect("prune must persist the new set");
        // HashSet snapshot order is process-random — compare as sets.
        let mut names = change.names;
        names.sort();
        assert_eq!(names, vec!["a".to_string(), "keep".to_string()]);
        // A list without disappearances does not persist again.
        app.update_sandboxes(vec![summary("a"), summary("keep")]);
        assert_eq!(app.take_autostart_change(), None);
    }

    #[test]
    fn update_sandboxes_keeps_all_marks_for_fleets_larger_than_the_sdk_page() {
        // Review fix C1: the SDK's list endpoint paginates (20-row first
        // page). Pruning against a sliced fleet deletes marks of older
        // (but real) sandboxes — permanent data loss. A 25-sandbox list
        // must keep a mark on the oldest record.
        let mut app = App::new();
        app.init_autostart(vec!["sbx-24".into(), "first".into()], true);
        let fleet: Vec<SandboxSummary> = (0..25)
            .map(|i| {
                if i == 0 {
                    summary("first")
                } else {
                    summary(&format!("sbx-{i:02}"))
                }
            })
            .collect();
        app.update_sandboxes(fleet);
        assert!(app.autostart.contains("first"), "{:?}", app.autostart);
        assert!(app.autostart.contains("sbx-24"));
        assert_eq!(app.autostart.len(), 2);
        assert_eq!(app.take_autostart_change(), None, "nothing vanished");
    }

    #[test]
    fn update_sandboxes_with_an_empty_list_wipes_all_marks() {
        // Documented policy: an empty fleet means the DB was wiped;
        // keeping marks would leave ghosts the boot pass reports as
        // missing. Pin the deliberate wipe + persist.
        let mut app = App::new();
        app.init_autostart(vec!["one".into(), "two".into()], true);
        app.update_sandboxes(vec![]);
        assert!(app.autostart.is_empty());
        let change = app
            .take_autostart_change()
            .expect("the wipe must be persisted");
        assert_eq!(change.names, Vec::<String>::new());
    }

    // ---- environments & credentials manager (issues #7/#6 integration) ----

    #[test]
    fn credential_matching_is_explicit_attachment_only() {
        // Matching is (a) equal to the form-selected name OR (b) scoped to
        // the effective profile. An empty `profiles` list is UNASSIGNED —
        // never "all".
        let gh = crate::credentials::Credential {
            name: "gh".into(),
            description: "t".into(),
            env_key: "GITHUB_TOKEN".into(),
            profiles: vec!["dev".into()],
            sandboxes: Vec::new(),
        };
        let unscoped = crate::credentials::Credential {
            name: "plain".into(),
            description: "t".into(),
            env_key: "KEY".into(),
            profiles: Vec::new(),
            sandboxes: Vec::new(),
        };
        // (a) direct selection wins regardless of (empty) profile scoping.
        assert!(credential_matches(&unscoped, Some("plain"), Some("dev")));
        assert!(credential_matches(&gh, Some("gh"), None));
        // (b) profile membership.
        assert!(credential_matches(&gh, None, Some("dev")));
        // OR semantics: both routes are independent.
        assert!(credential_matches(&gh, Some("other"), Some("dev")));
        assert!(credential_matches(&gh, Some("gh"), Some("dev")));
        // Never match: no context, empty profile list, non-member profile.
        assert!(!credential_matches(&gh, None, None));
        assert!(!credential_matches(&unscoped, None, Some("dev")));
        assert!(!credential_matches(&gh, None, Some("prod")));
    }

    #[test]
    fn injection_set_selected_first_then_file_order_deduped() {
        // credentials.toml order, dedup by name, and the form-selected
        // credential moved to the front (first in `env`, where the SDK's
        // last-wins gives it no priority advantage — order documents
        // intent and keeps repeated keys of the same credential coherent).
        let file = crate::credentials::CredentialsFile {
            credentials: vec![
                crate::credentials::Credential {
                    name: "one".into(),
                    description: String::new(),
                    env_key: "A".into(),
                    profiles: vec!["dev".into()],
                    sandboxes: Vec::new(),
                },
                crate::credentials::Credential {
                    name: "two".into(),
                    description: String::new(),
                    env_key: "B".into(),
                    profiles: vec!["dev".into()],
                    sandboxes: Vec::new(),
                },
                crate::credentials::Credential {
                    name: "sel".into(),
                    description: String::new(),
                    env_key: "C".into(),
                    profiles: vec!["dev".into()],
                    sandboxes: Vec::new(),
                },
            ],
        };
        // "sel" matches via BOTH routes (selected + profile) → deduped,
        // once, at the front.
        let set = injection_set(&file, Some("sel"), Some("dev"));
        let names: Vec<&str> = set.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["sel", "one", "two"]);
        // Profile only: file order.
        let profile_names: Vec<String> = injection_set(&file, None, Some("dev"))
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(
            profile_names,
            vec!["one", "two", "sel"]
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>()
        );
        // A selected name missing from the file injects nothing by itself.
        assert!(injection_set(&file, Some("missing"), None).is_empty());
        assert!(injection_set(&file, None, None).is_empty());
    }

    #[test]
    fn apply_credentials_pins_credentials_last_merge_order() {
        // The core #6 deliverable: resolved credential entries land AFTER
        // the spec's own (profile+form) entries in resolved order, with
        // `injected` carrying the names.
        let mut spec = crate::backend::CreateSpec {
            env: vec!["PROFILE_VAR=x".into(), "FORM_VAR=y".into()],
            ..Default::default()
        };
        let resolved = vec![
            ("sel".to_string(), "SELECTED".to_string(), "s1".to_string()),
            (
                "prof".to_string(),
                "PROFILE_KEY".to_string(),
                "s2".to_string(),
            ),
        ];
        let injected = apply_credentials(&mut spec, &resolved);
        assert_eq!(
            spec.env,
            vec![
                "PROFILE_VAR=x",
                "FORM_VAR=y",
                "SELECTED=s1",
                "PROFILE_KEY=s2"
            ]
        );
        assert_eq!(injected, vec!["sel", "prof"]);
        // Empty resolution: no entries, no names.
        let mut empty = crate::backend::CreateSpec {
            env: vec!["K=v".into()],
            ..Default::default()
        };
        assert!(apply_credentials(&mut empty, &[]).is_empty());
        assert_eq!(empty.env, vec!["K=v"]);
    }

    #[tokio::test]
    async fn create_without_a_keyring_secret_fails_closed() {
        // CI has no Secret Service — the keyring error is the reachable
        // fail-closed path. Resolution must abort BEFORE the backend runs:
        // FakeBackend's call log shows no Create.
        // "gh" would collide: if this dev machine has a REAL "gh" credential
        // under service "msb-tui", keyring_get succeeds, the create proceeds,
        // and the record step appends to the user's real credentials.toml.
        // A unique name guarantees the missing-secret path on every machine.
        use crate::backend::fake::{Call, FakeBackend};
        let name = "msb-tui-keyring-absent-fixture";
        let backend = FakeBackend::new();
        let file = crate::credentials::CredentialsFile {
            credentials: vec![credential(name, "GITHUB_TOKEN", &[])],
        };
        let res = create_with_credentials(
            &backend,
            crate::backend::CreateSpec {
                image: "alpine".into(),
                ..Default::default()
            },
            Some(name),
            None,
            &file,
        )
        .await;
        let err = res.expect_err("missing keyring secret must abort the create");
        assert!(
            err.to_string().contains("keyring"),
            "the UI-verbatim keyring error is shown: {err:#}"
        );
        let calls = backend.calls().await;
        assert!(
            !calls.iter().any(|c| matches!(c, Call::Create(_))),
            "fail closed: no sandbox created — {calls:?}"
        );
    }

    #[tokio::test]
    async fn create_without_injection_context_is_byte_identical() {
        // No selected credential, no profile → no keyring reads, no env
        // injection: the spec the backend sees is the form's spec as-is.
        use crate::backend::fake::{Call, FakeBackend};
        let backend = FakeBackend::new();
        let file = crate::credentials::CredentialsFile {
            credentials: vec![credential("gh", "GITHUB_TOKEN", &["dev"])],
        };
        let spec = crate::backend::CreateSpec {
            image: "alpine".into(),
            env: vec!["FORM=1".to_string()],
            ..Default::default()
        };
        let outcome = create_with_credentials(&backend, spec.clone(), None, None, &file)
            .await
            .unwrap();
        assert!(outcome.injected.is_empty() && outcome.record_error.is_none());
        let Call::Create(sent) = &backend.calls().await[0] else {
            panic!("expected a Create call");
        };
        assert_eq!(sent.env, spec.env, "byte-identical env");
    }

    #[tokio::test]
    async fn stale_selected_credential_reports_but_does_not_block_the_create() {
        // The form armed "gone"; the file no longer has it. Nothing is
        // injected (and no keyring read happens), the create proceeds,
        // and the outcome says so loudly — same policy family as the
        // missing-profile error, without blocking an unrelated create.
        use crate::backend::fake::FakeBackend;
        let backend = FakeBackend::new();
        let file = crate::credentials::CredentialsFile {
            credentials: vec![credential("gh", "GITHUB_TOKEN", &["dev"])],
        };
        let outcome = create_with_credentials(
            &backend,
            crate::backend::CreateSpec {
                image: "alpine".into(),
                ..Default::default()
            },
            Some("gone"),
            None,
            &file,
        )
        .await
        .unwrap();
        assert_eq!(outcome.stale_selection.as_deref(), Some("gone"));
        assert!(outcome.injected.is_empty());
        assert!(outcome.record_error.is_none());
        // An existing selection is never stale — `credential_matches` pins
        // that (only the vanished name is reported).
    }

    #[test]
    fn recording_a_sandbox_appends_deduped_and_flags_changes() {
        let mut file = crate::credentials::CredentialsFile {
            credentials: vec![credential("gh", "GITHUB_TOKEN", &[])],
        };
        assert!(mark_sandboxes_in_file(
            &mut file,
            &["gh".to_string()],
            "web"
        ));
        assert_eq!(file.credentials[0].sandboxes, vec!["web".to_string()]);
        // Same name again → no change (the caller skips the persist).
        assert!(!mark_sandboxes_in_file(
            &mut file,
            &["gh".to_string()],
            "web"
        ));
        // A credential outside the set stays untouched; an injected one
        // is appended.
        file.credentials.push(credential("openai", "KEY", &[]));
        assert!(mark_sandboxes_in_file(
            &mut file,
            &["openai".to_string()],
            "api"
        ));
        assert_eq!(
            file.credentials[0].sandboxes,
            vec!["web".to_string()],
            "'gh' is not in this injection set"
        );
        assert_eq!(file.credentials[1].sandboxes, vec!["api".to_string()]);
        // Multiple injections append in one pass, deduped per credential.
        assert!(mark_sandboxes_in_file(
            &mut file,
            &["gh".to_string(), "openai".to_string()],
            "api"
        ));
        assert_eq!(
            file.credentials[0].sandboxes,
            vec!["web".to_string(), "api".to_string()]
        );
        assert_eq!(
            file.credentials[1].sandboxes,
            vec!["api".to_string()],
            "'api' was already recorded for openai — no duplicate"
        );
    }

    #[test]
    fn form_submit_captures_no_credential_without_a_selection() {
        // The form's cycle lists come from the config dir; without a
        // selection nothing is captured and the queued spec is identical
        // to a pre-injection create (env untouched at App level — the
        // injection happens on the create task alone).
        let mut app = App::new();
        app.view = View::Create;
        app.create_state = Some(CreateState::Form(Box::new(CreateForm::new(Vec::new()))));
        let Some(CreateState::Form(form)) = &mut app.create_state else {
            panic!("expected form");
        };
        form.image = "alpine".into();
        form.cwd = std::env::temp_dir().to_string_lossy().into_owned();
        assert_eq!(form.selected_credential(), None);

        // Ctrl+Enter: create from anywhere (the form's escape hatch).
        let ctrl_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL);
        assert_eq!(app.handle_event(AppEvent::Key(ctrl_enter)), Action::Render);
        assert_eq!(app.view, View::Dashboard);
        assert!(app.busy);
        let (spec, credential, profile) = app.take_create().unwrap();
        assert_eq!(credential, None);
        assert_eq!(profile, None);
        assert!(spec.env.is_empty(), "no credential selected → no injection");
        assert!(app.take_create().is_none(), "context consumed together");
    }

    #[test]
    fn take_create_consumes_the_credential_context_together() {
        let mut app = App::new();
        app.queued_create = Some(crate::backend::CreateSpec {
            image: "alpine".into(),
            name: Some("web".into()),
            ..Default::default()
        });
        app.queued_credential = Some("gh".into());
        app.queued_profile = Some("dev".into());
        let (spec, credential, profile) = app.take_create().unwrap();
        assert_eq!(spec.name.as_deref(), Some("web"));
        assert_eq!(credential.as_deref(), Some("gh"));
        assert_eq!(profile.as_deref(), Some("dev"));
        assert!(app.queued_credential.is_none() && app.queued_profile.is_none());
        assert!(app.take_create().is_none());
    }

    #[test]
    fn template_enter_captures_the_profile_and_no_credential() {
        // Direct template create: the credential context is the template's
        // `env_profile`; there is never an explicit selection.
        let mut app = app_with_templates();
        app.view = View::Create;
        let shell = app.templates.iter().position(|t| t.id == "shell").unwrap() + 1;
        app.create_state = Some(CreateState::Select { selected: shell });
        app.handle_event(key(KeyCode::Enter));
        assert!(app.queued_create.is_some());
        assert!(app.queued_credential.is_none());
        assert!(
            app.queued_profile.is_none(),
            "the shell built-in has no env_profile"
        );
    }

    #[test]
    fn e_key_opens_environments_manager_and_esc_closes_it() {
        let mut app = App::new();
        assert_eq!(app.handle_event(key(KeyCode::Char('E'))), Action::Render);
        assert_eq!(app.view, View::Environments);
        assert!(
            app.environments.is_some(),
            "view state created on first open"
        );

        // Esc on the master list: Done → dashboard, state dropped.
        assert_eq!(app.handle_event(key(KeyCode::Esc)), Action::Render);
        assert_eq!(app.view, View::Dashboard);
        assert!(app.environments.is_none());
    }

    #[test]
    fn environments_manager_renders_offscreen_in_all_layouts() {
        // The render dispatcher arm must hold for master-detail, stacked
        // and list-only layouts (same offscreen smoke as the picker).
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = App::new();
        app.environments = Some(crate::ui::environments::EnvironmentsView::new());
        app.view = View::Environments;
        for (w, h) in [(120u16, 34u16), (60, 24), (80, 18)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal
                .draw(|f| crate::render_view(f, &app, f.area(), None))
                .unwrap();
        }
    }

    #[test]
    fn keyring_result_reports_to_status_and_the_open_view() {
        let mut app = App::new();
        app.environments = Some(crate::ui::environments::EnvironmentsView::new());
        app.view = View::Environments;
        assert_eq!(
            app.handle_event(AppEvent::KeyringResult {
                ok: true,
                msg: "saved".into()
            }),
            Action::Render
        );
        assert_eq!(app.status.as_deref(), Some("saved"));
        app.handle_event(AppEvent::KeyringResult {
            ok: false,
            msg: "boom".into(),
        });
        assert_eq!(app.error.as_deref(), Some("boom"));
        // The outcome survives the view closing before the task lands.
        app.handle_event(key(KeyCode::Esc));
        let mut app2 = App::new();
        app2.handle_event(AppEvent::KeyringResult {
            ok: true,
            msg: "late".into(),
        });
        assert_eq!(app2.status.as_deref(), Some("late"));
    }
}
