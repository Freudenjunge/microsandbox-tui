//! UI rendering — dashboard cards, help overlay, and dispatch.
//!
//! The render functions take explicit data arguments (not the whole `App`)
//! so this module has no dependency on `src/app.rs`. The event loop in
//! Task 10 wires `App` fields into these calls.
//!
//! NOTE(dead_code): consumers arrive in Task 10.

#![allow(dead_code)]

pub mod create;
pub mod dashboard;
pub mod help;
pub mod logs;
pub mod ports;

use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::models::{Metrics, PublishedPort, SandboxSummary};

/// Opaque base background for every view. Solid by design — the terminal's
/// own background (often translucent in fish/zsh setups) must not bleed
/// through ratatui's `Reset` default.
pub const BG: Color = Color::Rgb(16, 16, 20);

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
    let banner = crate::runtime::banner_text();
    let status = dashboard::StatusLines {
        banner: banner.as_deref(),
        error,
    };
    dashboard::render(frame, sandboxes, metrics, ports, selected, status, area);
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

/// Render a modal confirmation dialog for a pending operation.
pub fn render_confirm(frame: &mut Frame, message: &str, area: Rect) {
    let width = message.len().clamp(40, 60) as u16 + 8;
    let vert = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(5),
        Constraint::Fill(1),
    ])
    .split(area);
    let horiz = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(width),
        Constraint::Fill(1),
    ])
    .split(vert[1]);

    // Paint the dialog background instead of `Clear`, which would reset
    // cells to a transparent `Reset` background.
    frame.render_widget(Block::default().style(Style::default().bg(BG)), horiz[1]);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " Confirm ",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(Color::Red));
    let text = vec![
        Line::from(Span::raw(message)),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "y",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" = yes   "),
            Span::styled(
                "n/Esc",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" = no"),
        ]),
    ];
    frame.render_widget(Paragraph::new(text).block(block), horiz[1]);
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
