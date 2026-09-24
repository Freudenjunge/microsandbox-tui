//! Entry point: bootstrap the terminal, wire the backend pollers, and run the
//! event loop.
//!
//! The loop lives here (not in `app.rs`) so that `App` stays free of I/O and
//! remains unit-testable. We spawn three background tasks that feed
//! [`AppEvent`]s into an mpsc channel:
//!
//!   - **Sandbox list poller** — SDK list every 5 s
//!   - **Metrics poller** — SDK fleet metric reports every 1 s
//!   - **Input poller** — crossterm keys + periodic ticks ([`event::poll_events`])
//!
//! The main loop `tokio::select!`s on the channel receiver, dispatches each
//! event to [`App::handle_event`], and re-renders when the returned [`Action`]
//! says so.

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
use ratatui::style::Style;
use ratatui::widgets::Block;
use tokio::sync::mpsc;
use tokio_stream::StreamExt as _;

mod actions;
mod app;
mod backend;
mod event;
mod models;
mod runtime;
mod shell_window;
mod ui;

use app::{App, Op, View, ports_fetch_needed};
use backend::MsbBackend;
use backend::sdk::SdkBackend;
use event::{Action, AppEvent, poll_events};
use std::sync::Arc;

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

    // Fail cleanly before touching the terminal if the SDK cannot initialize.
    if let Err(e) = SdkBackend::new() {
        eprintln!("failed to initialize microsandbox SDK: {e:#}");
        std::process::exit(1);
    }

    // tokio runtime drives the pollers + event loop.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(run(cli))?;
    // The SDK's LocalBackend parks blocking threads (sqlx/SQLite workers,
    // migration locks) that never complete on their own. A plain drop of
    // `rt` would wait on them forever and the TUI would never exit; shut
    // down without draining them — the OS reaps the threads on exit.
    rt.shutdown_background();
    Ok(())
}

/// Run the TUI: set up the terminal, spawn pollers, and enter the event loop.
async fn run(cli: Cli) -> Result<()> {
    let backend = Arc::new(SdkBackend::new()?);

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
    let input_tx = event_tx.clone();
    tokio::spawn(async move {
        if let Err(e) = poll_events(input_tx).await {
            // Best-effort: if the receiver is gone the main loop will exit
            // when the channel closes; otherwise log to stderr.
            eprintln!("input poller error: {e}");
        }
    });

    // ---- main event loop ----

    // Abort handle + identity of the running log-tail task (one at a time).
    let mut log_task: Option<(String, tokio::sync::oneshot::Sender<()>)> = None;
    // Whether the image list was fetched for the create form this session.
    let mut images_fetched = false;

    while let Some(event) = event_rx.recv().await {
        let action = app.handle_event(event);
        if action == Action::Quit {
            break;
        }

        // Resolve queued ops / create specs by spawning backend tasks.
        if let Some(op) = app.take_op() {
            run_op(op, &backend, event_tx.clone());
        }
        if let Some(spec) = app.take_create_spec() {
            run_create(spec, &backend, event_tx.clone());
        }
        // (The captured-exec path from the removed EXEC tab is gone; `e`
        // now opens the interactive shell window below.)
        // Interactive shell (`e`): spawn a NEW terminal window with the
        // sandbox shell — the dashboard keeps running in parallel
        // (maintainer: parallel use, no TUI suspension).
        if app.view == View::Dashboard && app.take_shell_request() {
            let Some(sbx) = app.selected_sandbox().map(|s| s.name.clone()) else {
                continue;
            };
            app.set_status(match crate::shell_window::spawn_shell_window(&sbx) {
                Ok(()) => format!("shell window opened for {sbx}"),
                Err(e) => format!("shell window failed: {e}"),
            });
            render(&mut guard.terminal, &app)?;
        }

        // Log tail task: runs for the Logs view AND for the dashboard's
        // sidebar preview of the selected sandbox (one task at a time).
        let logs_name = app.logs_state.as_ref().map(|s| s.sandbox_name.clone());
        let preview_name = if app.view == View::Dashboard {
            app.preview_for.clone()
        } else {
            None
        };
        // Target sandbox for the active tail task: logs view wins, else the
        // dashboard preview target.
        let tail_target = logs_name.or(preview_name);
        let running_name = log_task.as_ref().map(|(n, _)| n.clone());
        if tail_target.is_some() && running_name.as_ref() != tail_target.as_ref() {
            // Different sandbox than the running task: abort and respawn.
            if let Some((_, abort)) = log_task.take() {
                let _ = abort.send(());
            }
            let (abort_tx, abort_rx) = tokio::sync::oneshot::channel::<()>();
            let tx = event_tx.clone();
            let backend = backend.clone();
            let task_name = tail_target.clone().expect("checked above");
            let spawn_name = task_name.clone();
            tokio::spawn(async move {
                let mut abort_rx = abort_rx;
                match backend.logs_follow(&spawn_name).await {
                    Ok(mut stream) => {
                        loop {
                            tokio::select! {
                                _ = &mut abort_rx => break,
                                line = stream.next() => match line {
                                    Some(line) => {
                                        if tx.send(AppEvent::LogLines(vec![line])).await.is_err() {
                                            break;
                                        }
                                    }
                                    None => break,
                                }
                            }
                        }
                        let _ = tx.send(AppEvent::LogsEnded).await;
                    }
                    Err(e) => {
                        let _ = tx.send(AppEvent::Error(format!("logs: {e:#}"))).await;
                    }
                }
            });
            log_task = Some((task_name, abort_tx));
        }

        // No logs view open and no preview target: stop the tail task.
        let tail_needed = app.view == View::Logs || tail_target.is_some();
        if !tail_needed && let Some((_, abort)) = log_task.take() {
            let _ = abort.send(());
        }

        // Entering the Create view: fetch the local image list once so the
        // picker shows pulled images.
        if app.view == View::Create && !images_fetched {
            images_fetched = true;
            let tx = event_tx.clone();
            let backend = backend.clone();
            tokio::spawn(async move {
                match backend.list_images().await {
                    Ok(list) => {
                        let _ = tx.send(AppEvent::ImagesUpdated(list)).await;
                    }
                    Err(e) => {
                        let _ = tx.send(AppEvent::Error(format!("images: {e:#}"))).await;
                    }
                }
            });
        }

        // Port cache refresh: entering the PORTS tab, `r`, a recreate op,
        // or the initial list load (cards show ports immediately) fetch
        // ports for ALL sandboxes so the global matrix reflects each
        // sandbox's own bindings. `ports_fetch_needed` is the single,
        // unit-tested trigger policy.
        if ports_fetch_needed(&app) {
            app.ports_cache_dirty = false;
            app.ports_primed = true;
            let tx = event_tx.clone();
            let backend = backend.clone();
            let names: Vec<String> = app.sandboxes.iter().map(|s| s.name.clone()).collect();
            tokio::spawn(async move {
                for name in names {
                    match backend.inspect(&name).await {
                        Ok(insp) => {
                            let _ = tx
                                .send(AppEvent::PortsUpdated {
                                    name,
                                    ports: insp.active_config.network.ports,
                                })
                                .await;
                        }
                        Err(e) => {
                            let _ = tx.send(AppEvent::Error(format!("ports: {e:#}"))).await;
                        }
                    }
                }
            });
        }

        render(&mut guard.terminal, &app)?;
        if app.quit {
            break;
        }
    }

    // Stop the log task on exit.
    if let Some((_, abort)) = log_task.take() {
        let _ = abort.send(());
    }

    // TerminalGuard::drop restores the terminal.
    Ok(())
}

/// Execute a confirmed lifecycle/exec operation on a background task,
/// reporting completion or failure through the event channel.
fn run_op(op: Op, backend: &Arc<SdkBackend>, tx: mpsc::Sender<AppEvent>) {
    let backend = backend.clone();
    let describe = op.describe();
    tokio::spawn(async move {
        let res = match &op {
            Op::Start(name) => actions::start_sandbox(backend.as_ref(), name)
                .await
                .map(|_| format!("started {name}")),
            Op::Stop(name) => actions::stop_sandbox(backend.as_ref(), name)
                .await
                .map(|_| format!("stopped {name}")),
            Op::Restart(name) => actions::restart_sandbox(backend.as_ref(), name)
                .await
                .map(|_| format!("restarted {name}")),
            Op::Remove(name) => actions::remove_sandbox(backend.as_ref(), name)
                .await
                .map(|_| format!("removed {name}")),
            Op::Exec { name, cmd } => match backend.exec(name, cmd).await {
                Ok(output) => {
                    let _ = tx
                        .send(AppEvent::ExecDone {
                            name: name.clone(),
                            args: cmd.clone(),
                            res: Ok(output),
                        })
                        .await;
                    return;
                }
                Err(e) => {
                    let _ = tx
                        .send(AppEvent::ExecDone {
                            name: name.clone(),
                            args: cmd.clone(),
                            res: Err(format!("{e:#}")),
                        })
                        .await;
                    return;
                }
            },
            Op::PublishPort { name, port } => {
                actions::publish_port(backend.as_ref(), name, port.clone())
                    .await
                    .map(|_| format!("published {}:{} on {name}", port.host_port, port.guest_port))
            }
            Op::UnpublishPort { name, port } => {
                actions::unpublish_port(backend.as_ref(), name, port)
                    .await
                    .map(|_| format!("unpublished {}:{name}", port.host_port))
            }
            Op::InstallRuntime => {
                crate::runtime::install_or_update()
                    .await
                    .map(|outcome| match outcome {
                        crate::runtime::InstallOutcome::Installed { msb_path } => {
                            format!("runtime installed: {}", msb_path.display())
                        }
                        crate::runtime::InstallOutcome::AlreadyCurrent => {
                            "runtime already current".to_string()
                        }
                    })
            }
        };
        let event = match res {
            Ok(msg) => AppEvent::OpDone(msg),
            Err(e) => AppEvent::Error(format!("{describe} — failed: {e:#}")),
        };
        let _ = tx.send(event).await;
    });
}

/// Create a sandbox from a validated form spec, reporting the outcome.
fn run_create(
    spec: crate::backend::CreateSpec,
    backend: &Arc<SdkBackend>,
    tx: mpsc::Sender<AppEvent>,
) {
    let backend = backend.clone();
    tokio::spawn(async move {
        match actions::create_sandbox(backend.as_ref(), &spec).await {
            Ok(name) => {
                let _ = tx.send(AppEvent::OpDone(format!("created {name}"))).await;
            }
            Err(e) => {
                let _ = tx
                    .send(AppEvent::Error(format!("create failed: {e:#}")))
                    .await;
            }
        }
    });
}

/// Future returned by a poller closure.
type PollFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Result<AppEvent>> + Send>>;
/// Poller closure: takes the backend and produces an event future.
type PollFn = fn(Arc<SdkBackend>) -> PollFuture;

/// Spawn a periodic poller that calls `f` on the backend at `interval`,
/// sending the resulting `AppEvent` into `tx`. Errors are surfaced as
/// [`AppEvent::Error`].
fn spawn_poller(
    tx: mpsc::Sender<AppEvent>,
    interval: Duration,
    backend: Arc<SdkBackend>,
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
                    let msg = format!("{name}: {e:#}");
                    if tx.send(AppEvent::Error(msg)).await.is_err() {
                        break;
                    }
                }
            }
        }
    });
}

/// Render the current view based on `app.view`.
fn render(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &App) -> Result<()> {
    terminal.draw(|frame| {
        let area = frame.area();
        let banner = crate::runtime::banner_text();
        // Opaque base layer: without it, transparent terminal themes
        // (e.g. milky fish/zsh setups) bleed through ratatui's Reset
        // background. Widgets only patch cells they touch, so this solid
        // backdrop persists behind every view.
        frame.render_widget(
            Block::default().style(Style::default().bg(ui::theme::THEME.bg)),
            area,
        );
        match app.view {
            View::Dashboard => {
                ui::render_dashboard(
                    frame,
                    &app.sandboxes,
                    &app.metrics,
                    &app.ports,
                    app.selected,
                    ui::dashboard::StatusLines {
                        banner: banner.as_deref(),
                        error: app.error.as_deref(),
                        status_text: app.status.as_deref(),
                    },
                    app.detail,
                    app.logs_state.as_ref(),
                    app.ports_state.as_ref(),
                    &app.preview_lines,
                    !app.initial_list_loaded,
                    area,
                );
            }
            View::Help => {
                ui::render_help(frame, area);
            }
            View::Create => match &app.create_form {
                Some(form) => ui::create::render_create_form(frame, form, area),
                None => ui::render_placeholder(frame, "Create", area),
            },
            View::Logs | View::Ports | View::Inspect => {
                // 2.9 IA: logs/ports/inspect are detail tabs of the selected
                // sandbox, rendered inside the dashboard; these arms only
                // fire during transition frames.
                ui::render_dashboard(
                    frame,
                    &app.sandboxes,
                    &app.metrics,
                    &app.ports,
                    app.selected,
                    ui::dashboard::StatusLines {
                        banner: banner.as_deref(),
                        error: app.error.as_deref(),
                        status_text: app.status.as_deref(),
                    },
                    app.detail,
                    app.logs_state.as_ref(),
                    app.ports_state.as_ref(),
                    &app.preview_lines,
                    !app.initial_list_loaded,
                    area,
                );
            }
        }

        // Confirmation dialog overlays any view.
        if let Some(op) = &app.confirm {
            ui::render_confirm(frame, &op.describe(), area);
        }
    })?;
    Ok(())
}
