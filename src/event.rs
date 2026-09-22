//! Event types and the crossterm input polling task.
//!
//! The main event loop (`src/app.rs`) receives [`AppEvent`]s on an
//! `mpsc` channel and dispatches them to [`crate::app::App::handle_event`].
//! This module owns the producer side: [`poll_events`] reads crossterm
//! key events from stdin and emits periodic [`AppEvent::Tick`]s for UI
//! re-render cadence.
//!
//! NOTE(dead_code): types here gain runtime callers in Task 10 (main
//! wiring). Remove this allow once the event loop is wired into main.

#![allow(dead_code)]

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event as CrosstermEvent, KeyEvent};
use tokio::sync::mpsc;
use tokio::task;
use tokio::time;

use crate::backend::{ExecOutput, LogLine};
use crate::models::{Metrics, PublishedPort, SandboxSummary};

/// Events the app cares about (abstracted over crossterm + background pollers).
#[derive(Debug)]
pub enum AppEvent {
    /// A key was pressed.
    Key(KeyEvent),
    /// Periodic UI tick (re-render / poll cadence).
    Tick,
    /// The sandbox list was refreshed by a background poller.
    SandboxesUpdated(Vec<SandboxSummary>),
    /// Metrics were refreshed by a background poller.
    MetricsUpdated(Vec<Metrics>),
    /// An async error occurred (shown on the status line).
    Error(String),
    /// A long-running SDK operation finished successfully.
    OpDone(String),
    /// New log lines arrived for the logs view.
    LogLines(Vec<LogLine>),
    /// The log stream ended (child exited).
    LogsEnded,
    /// Images list was refreshed (used by the create form autocomplete).
    ImagesUpdated(Vec<String>),
    /// Ports for a sandbox were refreshed (used by the ports view).
    PortsUpdated {
        name: String,
        ports: Vec<PublishedPort>,
    },
    /// Output of a completed exec command.
    ExecDone {
        name: String,
        args: Vec<String>,
        res: std::result::Result<ExecOutput, String>,
    },
}

/// What the main loop should do after handling an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Continue running, no re-render needed.
    Continue,
    /// Re-render the UI.
    Render,
    /// Exit the application.
    Quit,
}

/// Interval between [`AppEvent::Tick`] events (UI responsiveness cadence).
pub const TICK_INTERVAL: Duration = Duration::from_millis(250);

/// Read crossterm key events from stdin and emit [`AppEvent`]s to `event_tx`.
///
/// Emits [`AppEvent::Key`] for each terminal key event and a
/// [`AppEvent::Tick`] every [`TICK_INTERVAL`]. Returns `Ok(())` once the
/// receiver is dropped (i.e. the app is shutting down).
///
/// crossterm's async `EventStream` requires the `event-stream` Cargo feature,
/// which this crate does not enable, so stdin is read with the blocking
/// `event::poll`/`event::read` API on a dedicated worker thread, while ticks
/// are driven by a lightweight async interval task.
pub async fn poll_events(event_tx: mpsc::Sender<AppEvent>) -> Result<()> {
    // Tick task: periodic UI re-render cadence.
    let tick_tx = event_tx.clone();
    let tick_task = tokio::spawn(async move {
        let mut timer = time::interval(TICK_INTERVAL);
        // Discard the immediate first tick.
        timer.tick().await;
        loop {
            timer.tick().await;
            if tick_tx.send(AppEvent::Tick).await.is_err() {
                break;
            }
        }
    });

    // Key task: blocking crossterm reads on a worker thread.
    let key_task = task::spawn_blocking(move || -> Result<()> {
        loop {
            // `event::poll` blocks until input is available; we cap the wait
            // so the task can observe shutdown between key presses.
            if event::poll(Duration::from_millis(250))?
                && let CrosstermEvent::Key(key) = event::read()?
            {
                // `blocking_send` returns Err when the receiver is gone.
                if event_tx.blocking_send(AppEvent::Key(key)).is_err() {
                    return Ok(());
                }
            }
        }
    });

    // The key reader returns once the receiver is dropped; abort the tick
    // task and surface any error from the reader.
    let key_res = key_task.await;
    tick_task.abort();
    key_res??;
    Ok(())
}
