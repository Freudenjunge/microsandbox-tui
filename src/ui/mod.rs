//! UI rendering — dashboard cards, help overlay, and dispatch.
//!
//! The render functions take explicit data arguments (not the whole `App`)
//! so this module has no dependency on `src/app.rs`. The event loop in
//! Task 10 wires `App` fields into these calls.
//!
//! NOTE(dead_code): consumers arrive in Task 10.

#![allow(dead_code)]

pub mod chrome;
pub mod create;
pub mod dashboard;
pub mod help;
pub mod logs;
pub mod ports;
pub mod theme;

use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::models::{Metrics, PublishedPort, SandboxSummary};
use crate::ui::theme::THEME;

/// Render the dashboard view (rail + detail tabs; chrome included).
#[allow(clippy::too_many_arguments)]
pub fn render_dashboard(
    frame: &mut Frame,
    sandboxes: &[SandboxSummary],
    metrics: &HashMap<String, Metrics>,
    ports: &HashMap<String, Vec<PublishedPort>>,
    selected: usize,
    status: dashboard::StatusLines<'_>,
    detail: crate::app::DetailTab,
    logs_state: Option<&logs::LogsState>,
    ports_state: Option<&ports::PortsState>,
    preview: &[crate::backend::LogLine],
    initial_loading: bool,
    area: Rect,
) {
    dashboard::render(
        frame,
        sandboxes,
        metrics,
        ports,
        selected,
        status,
        detail,
        logs_state,
        ports_state,
        preview,
        initial_loading,
        area,
    );
}

/// Render the help overlay (centered keybinding table).
pub fn render_help(frame: &mut Frame, area: Rect) {
    help::render(frame, area);
}

/// Render a modal confirmation dialog for a pending operation.
pub fn render_confirm(frame: &mut Frame, message: &str, area: Rect) {
    let t = &THEME;
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
    frame.render_widget(
        Block::default().style(Style::default().bg(THEME.bg)),
        horiz[1],
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " Confirm ",
            Style::default().fg(t.err).add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(t.err));
    let text = vec![
        Line::from(Span::raw(message)),
        Line::from(""),
        Line::from(vec![
            Span::styled("y", Style::default().fg(t.ok).add_modifier(Modifier::BOLD)),
            Span::raw(" = yes   "),
            Span::styled(
                "n/Esc",
                Style::default().fg(t.err).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" = no"),
        ]),
    ];
    frame.render_widget(Paragraph::new(text).block(block), horiz[1]);
}

/// Render a "Not implemented" placeholder for views not yet built.
pub fn render_placeholder(frame: &mut Frame, title: &str, area: Rect) {
    let t = &THEME;
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "))
        .border_style(Style::default().fg(t.muted));
    let text = Line::from(vec![
        Span::styled(
            "Not implemented yet",
            Style::default().fg(t.warn).add_modifier(Modifier::DIM),
        ),
        Span::raw("  —  press "),
        Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" to go back"),
    ]);
    let para = Paragraph::new(text).centered().block(block);
    frame.render_widget(para, area);
}
