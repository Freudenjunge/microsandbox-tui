//! Entry point: bootstrap the terminal, wire the backend pollers, and run the
//! event loop.
//!
//! The loop lives here (not in `app.rs`) so that `App` stays free of I/O and
//! remains unit-testable. We spawn three background tasks that feed
//! [`AppEvent`]s into an mpsc channel:
//!
//!   - **Sandbox list poller** — `msb ls` every 5 s
//!   - **Metrics poller** — `msb metrics --all` every 1 s
//!   - **Input poller** — crossterm keys + periodic ticks ([`event::poll_events`])
//!
//! The main loop `tokio::select!`s on the channel receiver, dispatches each
//! event to [`App::handle_event`], and re-renders when the returned [`Action`]
//! says so.

use std::collections::HashMap;
use std::io::{Stdout, stdout};
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;

mod actions;
mod app;
mod backend;
mod event;
mod models;
mod ui;

use app::{App, View};
use backend::MsbBackend;
use backend::cli::CliBackend;
use event::{Action, AppEvent, poll_events};
use models::PublishedPort;

/// Polling interval for the sandbox list (`msb ls`).
const SANDBOX_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Polling interval for metrics (`msb metrics --all`).
const METRICS_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// A Docker sbx-style TUI for microsandbox.
#[derive(Parser, Debug)]
#[command(name = "msb-tui", version, about)]
struct Cli {
    /// Open the create-sandbox form directly.
    #[arg(short, long)]
    create: bool,
}

/// RAII guard that restores the terminal on drop, even if the main loop
/// panics or returns early.
struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    /// Enter raw mode, switch to the alternate screen, and create the
    /// ratatui terminal.
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        execute!(stdout(), EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout());
        let terminal = Terminal::new(backend)?;
        Ok(Self { terminal })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Verify msb is installed and on PATH (CliBackend::new does this too, but
    // we do it here first for a clean error before touching the terminal).
    CliBackend::new()?;

    // tokio runtime drives the pollers + event loop.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(run(cli))
}

/// Run the TUI: set up the terminal, spawn pollers, and enter the event loop.
async fn run(cli: Cli) -> Result<()> {
    let backend = std::sync::Arc::new(CliBackend::new()?);

    let mut guard = TerminalGuard::enter()?;
    let mut app = App::new();
    if cli.create {
        app.view = View::Create;
    }

    // Channel: background pollers + input reader → main loop.
    let (event_tx, mut event_rx) = mpsc::channel::<AppEvent>(64);

    // ---- spawn background pollers ----

    // Sandbox list poller: `msb ls` every 5 s.
    spawn_poller(
        event_tx.clone(),
        SANDBOX_POLL_INTERVAL,
        backend.clone(),
        |b| {
            Box::pin(async move {
                let list = b.list_sandboxes().await?;
                Ok(AppEvent::SandboxesUpdated(list))
            })
        },
        "sandbox list",
    );

    // Metrics poller: `msb metrics --all` every 1 s.
    spawn_poller(
        event_tx.clone(),
        METRICS_POLL_INTERVAL,
        backend.clone(),
        |b| {
            Box::pin(async move {
                let samples = b.metrics().await?;
                Ok(AppEvent::MetricsUpdated(samples))
            })
        },
        "metrics",
    );

    // Input poller: crossterm keys + ticks.
    let input_tx = event_tx;
    tokio::spawn(async move {
        if let Err(e) = poll_events(input_tx).await {
            // Best-effort: if the receiver is gone the main loop will exit
            // when the channel closes; otherwise log to stderr.
            eprintln!("input poller error: {e}");
        }
    });

    // ---- main event loop ----

    // Ports data keyed by sandbox name (populated lazily when entering the
    // Ports view). In Phase 1 this stays empty until an inspect call lands;
    // for now the view renders "No published ports".
    let ports_cache: HashMap<String, Vec<PublishedPort>> = HashMap::new();

    while let Some(event) = event_rx.recv().await {
        let action = app.handle_event(event);
        match action {
            Action::Quit => break,
            Action::Render | Action::Continue => {
                render(&mut guard.terminal, &app, &ports_cache)?;
                if app.quit {
                    break;
                }
            }
        }
    }

    // TerminalGuard::drop restores the terminal.
    Ok(())
}

/// Future returned by a poller closure.
type PollFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Result<AppEvent>> + Send>>;
/// Poller closure: takes the backend and produces an event future.
type PollFn = fn(std::sync::Arc<CliBackend>) -> PollFuture;

/// Spawn a periodic poller that calls `f` on the backend at `interval`,
/// sending the resulting `AppEvent` into `tx`. Errors are surfaced as
/// [`AppEvent::Error`].
fn spawn_poller(
    tx: mpsc::Sender<AppEvent>,
    interval: Duration,
    backend: std::sync::Arc<CliBackend>,
    f: PollFn,
    name: &'static str,
) {
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(interval);
        // Discard the immediate first tick so we don't fire before the loop is ready.
        timer.tick().await;
        loop {
            timer.tick().await;
            match f(backend.clone()).await {
                Ok(event) => {
                    if tx.send(event).await.is_err() {
                        break; // main loop gone — stop polling.
                    }
                }
                Err(e) => {
                    let msg = format!("{name}: {e}");
                    if tx.send(AppEvent::Error(msg)).await.is_err() {
                        break;
                    }
                }
            }
        }
    });
}

/// Render the current view based on `app.view`.
fn render(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &App,
    ports_cache: &HashMap<String, Vec<PublishedPort>>,
) -> Result<()> {
    terminal.draw(|frame| {
        let area = frame.area();
        match app.view {
            View::Dashboard => {
                ui::render_dashboard(
                    frame,
                    &app.sandboxes,
                    &app.metrics,
                    ports_cache,
                    app.selected,
                    app.error.as_deref(),
                    area,
                );
            }
            View::Help => {
                ui::render_help(frame, area);
            }
            View::Create => {
                ui::render_placeholder(frame, "Create form — Task 6", area);
            }
            View::Logs => {
                if let Some(sbx) = app.selected_sandbox() {
                    let state = ui::logs::LogsState::new(&sbx.name);
                    ui::render_logs_panel(frame, &state, area);
                } else {
                    ui::render_placeholder(frame, "Logs", area);
                }
            }
            View::Ports => {
                if let Some(sbx) = app.selected_sandbox() {
                    let ports = ports_cache.get(&sbx.name).cloned().unwrap_or_default();
                    let mut state = ui::ports::PortsState::new(&sbx.name);
                    state.update_ports(ports);
                    ui::render_ports(frame, &state, area);
                } else {
                    ui::render_placeholder(frame, "Ports", area);
                }
            }
            View::Inspect => {
                let title = match app.selected_sandbox() {
                    Some(sbx) => format!("Inspect: {}", sbx.name),
                    None => "Inspect".to_string(),
                };
                ui::render_placeholder(frame, &title, area);
            }
        }
    })?;
    Ok(())
}
