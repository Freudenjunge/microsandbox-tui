//! UI rendering — dashboard cards, help overlay, and dispatch.
//!
//! The render functions take explicit data arguments (not the whole `App`)
//! so this module has no dependency on `src/app.rs`. The event loop in
//! Task 10 wires `App` fields into these calls.
//!
//! NOTE(dead_code): consumers arrive in Task 10.

#![allow(dead_code)]

pub mod dashboard;
pub mod help;
pub mod logs;
pub mod ports;

use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::models::{Metrics, PublishedPort, SandboxSummary};

/// Render the dashboard view (header + cards + footer).
pub fn render_dashboard(
    frame: &mut Frame,
    sandboxes: &[SandboxSummary],
    metrics: &HashMap<String, Metrics>,
    ports: &HashMap<String, Vec<PublishedPort>>,
    selected: usize,
    error: Option<&str>,
    area: Rect,
) {
    dashboard::render(frame, sandboxes, metrics, ports, selected, error, area);
}

/// Render the help overlay (centered keybinding table).
pub fn render_help(frame: &mut Frame, area: Rect) {
    help::render(frame, area);
}

/// Render the port-forwards view for a sandbox.
pub fn render_ports(frame: &mut Frame, state: &ports::PortsState, area: Rect) {
    ports::render_ports(frame, state, area);
}

/// Render the logs panel for a sandbox.
pub fn render_logs_panel(frame: &mut Frame, state: &logs::LogsState, area: Rect) {
    logs::render_logs(frame, state, area);
}

/// Render a "Not implemented" placeholder for views not yet built.
pub fn render_placeholder(frame: &mut Frame, title: &str, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "))
        .border_style(Style::default().fg(Color::DarkGray));
    let text = Line::from(vec![
        Span::styled(
            "Not implemented yet",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::DIM),
        ),
        Span::raw("  —  press "),
        Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" to go back"),
    ]);
    let para = Paragraph::new(text).centered().block(block);
    frame.render_widget(para, area);
}
