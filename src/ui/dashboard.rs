//! Dashboard view: responsive grid of sandbox cards.
//!
//! Layout: header line, card grid, footer keybinding hints (and an optional
//! red error line when the app reports one). Rendering is intentionally thin —
//! all formatting helpers below are pure and unit-tested.
//!
//! NOTE(dead_code): consumers arrive in Task 10.

#![allow(dead_code)]

use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::models::{Metrics, PublishedPort, SandboxState, SandboxSummary};
use crate::ui::theme::THEME;

/// Height in rows of a single card (including its border).
const CARD_HEIGHT: u16 = 8;

/// Footer keybinding hints (dashboard context).
const FOOTER_HINTS: &str =
    "[c] Create  [e] exec  [l] logs  [p] ports  [r] restart  [x] stop  [Del] rm  [q] quit";
const FOOTER_HINTS_2: &str = "[enter] inspect  [U] runtime  [↑↓] select  [?] help";

/// Status/message lines drawn under the card grid.
#[derive(Debug, Default, Clone, Copy)]
pub struct StatusLines<'a> {
    /// Runtime banner (update available / missing runtime), yellow.
    pub banner: Option<&'a str>,
    /// Last error, red.
    pub error: Option<&'a str>,
}

/// Render the dashboard into `area`.
pub fn render(
    frame: &mut Frame,
    sandboxes: &[SandboxSummary],
    metrics: &HashMap<String, Metrics>,
    ports: &HashMap<String, Vec<PublishedPort>>,
    selected: usize,
    status: StatusLines<'_>,
    area: Rect,
) {
    let t = &THEME;
    let mut constraints = vec![
        Constraint::Length(1), // header
        Constraint::Min(3),    // cards
        Constraint::Length(2), // footer hints
    ];
    if status.banner.is_some() {
        constraints.push(Constraint::Length(1)); // runtime banner
    }
    if status.error.is_some() {
        constraints.push(Constraint::Length(1)); // error line
    }
    let chunks = Layout::vertical(constraints).split(area);

    render_header(frame, sandboxes.len(), chunks[0]);

    if sandboxes.is_empty() {
        render_empty(frame, chunks[1]);
    } else {
        render_grid(frame, sandboxes, metrics, ports, selected, chunks[1]);
    }

    let footer = Paragraph::new(vec![
        Line::from(Span::styled(FOOTER_HINTS, Style::default().fg(t.text))),
        Line::from(Span::styled(FOOTER_HINTS_2, Style::default().fg(t.muted))),
    ]);
    frame.render_widget(footer, chunks[2]);

    // Runtime banner (update available / missing runtime), then error line.
    let mut next = 3;
    if let Some(text) = status.banner {
        let line = Paragraph::new(Line::from(vec![
            Span::styled("⬆ ", Style::default().fg(t.warn)),
            Span::styled(text.to_owned(), Style::default().fg(t.warn)),
        ]));
        frame.render_widget(line, chunks[next]);
        next += 1;
    }
    if let Some(msg) = status.error {
        let line = Paragraph::new(Line::from(vec![
            Span::styled("✗ ", Style::default().fg(t.err)),
            Span::styled(msg.to_owned(), Style::default().fg(t.err)),
        ]));
        frame.render_widget(line, chunks[next]);
    }
}

/// Header line: sandbox count on the left, quick actions on the right.
fn render_header(frame: &mut Frame, count: usize, area: Rect) {
    let t = &THEME;
    let line = Line::from(vec![
        Span::styled(
            format!(" Sandboxes ({count})"),
            Style::default().fg(t.fg).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled("[c] Create  [q] Quit", Style::default().fg(t.muted)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

/// Centered empty-state message.
fn render_empty(frame: &mut Frame, area: Rect) {
    let t = &THEME;
    let para = Paragraph::new(Line::from(vec![
        Span::styled("No sandboxes. ", Style::default().fg(t.text)),
        Span::styled("Press ", Style::default().fg(t.muted)),
        Span::styled(
            "[c]",
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" to create one.", Style::default().fg(t.muted)),
    ]))
    .centered()
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t.muted)),
    );
    frame.render_widget(para, area);
}

/// Lay out cards in a responsive grid and render each one.
fn render_grid(
    frame: &mut Frame,
    sandboxes: &[SandboxSummary],
    metrics: &HashMap<String, Metrics>,
    ports: &HashMap<String, Vec<PublishedPort>>,
    selected: usize,
    area: Rect,
) {
    let cols = card_columns(area.width);
    let total_rows = sandboxes.len().div_ceil(cols);
    // How many rows fit, and which slice to show so the selection is visible.
    let max_rows = usize::from((area.height / CARD_HEIGHT).max(1));
    let sel_row = selected / cols;
    let start_row = sel_row.saturating_sub(max_rows.saturating_sub(1));
    let visible_rows = total_rows.saturating_sub(start_row).min(max_rows).max(1);

    let row_constraints = vec![Constraint::Length(CARD_HEIGHT); visible_rows];
    let row_areas = Layout::vertical(row_constraints).split(area);

    let col_constraints = vec![Constraint::Ratio(1, u32::try_from(cols).unwrap_or(1)); cols];

    for vis in 0..visible_rows {
        let row = start_row + vis;
        let col_areas = Layout::horizontal(col_constraints.clone()).split(row_areas[vis]);
        for col in 0..cols {
            let idx = row * cols + col;
            let Some(sbx) = sandboxes.get(idx) else {
                continue;
            };
            render_card(
                frame,
                sbx,
                metrics.get(&sbx.name),
                ports.get(&sbx.name),
                idx == selected,
                col_areas[col],
            );
        }
    }
}

/// Render a single sandbox card.
fn render_card(
    frame: &mut Frame,
    sbx: &SandboxSummary,
    metrics: Option<&Metrics>,
    ports: Option<&Vec<PublishedPort>>,
    selected: bool,
    area: Rect,
) {
    let t = &THEME;
    let (symbol, color) = state_indicator(&sbx.status);
    let border_style = if selected {
        Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(t.muted)
    };
    let title = if selected { " ▶ " } else { "   " };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(title, border_style));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let name_style = if selected {
        Style::default().fg(t.fg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(t.fg)
    };

    let mut lines = vec![
        Line::from(vec![
            Span::styled(format!("{symbol} "), Style::default().fg(color)),
            Span::styled(sbx.name.clone(), name_style),
            Span::raw(" "),
            Span::styled(sbx.status.to_string(), Style::default().fg(color)),
        ]),
        Line::from(Span::styled(
            sbx.image.clone(),
            Style::default().fg(t.muted),
        )),
    ];

    match metrics {
        Some(m) => {
            lines.push(Line::from(vec![
                Span::styled("CPU ", Style::default().fg(t.muted)),
                Span::styled(
                    format!("{:.0}%", m.cpu_percent),
                    Style::default().fg(t.text),
                ),
                Span::raw("  "),
                Span::styled("MEM ", Style::default().fg(t.muted)),
                Span::styled(format_bytes(m.memory_bytes), Style::default().fg(t.text)),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Net ", Style::default().fg(t.muted)),
                Span::styled(
                    format!(
                        "↓{} ↑{}",
                        format_bytes(m.net_rx_bytes),
                        format_bytes(m.net_tx_bytes)
                    ),
                    Style::default().fg(t.text),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Ports ", Style::default().fg(t.muted)),
                Span::styled(ports_short(ports), Style::default().fg(t.text)),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Uptime ", Style::default().fg(t.muted)),
                Span::styled(format_duration(m.uptime_secs), Style::default().fg(t.text)),
            ]));
        }
        None => {
            lines.push(Line::from(Span::styled(
                "CPU --   MEM --",
                Style::default().fg(t.muted),
            )));
            lines.push(Line::from(Span::styled(
                "Net --",
                Style::default().fg(t.muted),
            )));
            lines.push(Line::from(vec![
                Span::styled("Ports ", Style::default().fg(t.muted)),
                Span::styled(ports_short(ports), Style::default().fg(t.text)),
            ]));
            lines.push(Line::from(Span::styled(
                "Uptime --",
                Style::default().fg(t.muted),
            )));
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

/// Number of card columns for a given content width.
fn card_columns(width: u16) -> usize {
    match width {
        0..=39 => 1,
        40..=89 => 2,
        _ => 3,
    }
}

/// Short, comma-separated port list for a card, e.g. `8080→80, 9090→90/udp`.
fn ports_short(ports: Option<&Vec<PublishedPort>>) -> String {
    match ports {
        None => "—".to_string(),
        Some(list) if list.is_empty() => "—".to_string(),
        Some(list) => list
            .iter()
            .map(|p| {
                let proto = if p.protocol == "tcp" {
                    String::new()
                } else {
                    format!("/{}", p.protocol)
                };
                format!("{}→{}{proto}", p.host_port, p.guest_port)
            })
            .collect::<Vec<_>>()
            .join(", "),
    }
}

// ---------- pure formatting helpers (unit-tested) ----------

/// Human-readable byte count using binary units, e.g. `1.2K`, `3.4M`, `5.6G`.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "K", "M", "G", "T", "P"];
    if bytes < 1024 {
        return format!("{bytes}B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let rendered = format!("{value:.1}");
    let rendered = rendered.strip_suffix(".0").unwrap_or(&rendered);
    format!("{rendered}{}", UNITS[unit])
}

/// Human-readable duration, e.g. `12s`, `5m 32s`, `1h 23m`, `2d 3h`.
pub fn format_duration(secs: f64) -> String {
    let total = secs.max(0.0) as u64;
    if total < 60 {
        format!("{total}s")
    } else if total < 3600 {
        format!("{}m {}s", total / 60, total % 60)
    } else if total < 86_400 {
        format!("{}h {}m", total / 3600, (total % 3600) / 60)
    } else {
        format!("{}d {}h", total / 86_400, (total % 86_400) / 3600)
    }
}

/// Symbol + color for a sandbox state indicator.
pub fn state_indicator(state: &SandboxState) -> (&'static str, Color) {
    let t = &THEME;
    match state {
        SandboxState::Running => ("●", t.ok),
        SandboxState::Stopped => ("○", t.text),
        SandboxState::Paused => ("⏸", t.warn),
        SandboxState::Exited => ("✗", t.err),
        SandboxState::Created => ("○", t.accent),
        SandboxState::Crashed => ("✗", t.err),
        SandboxState::Stalled => ("⏸", t.warn),
        SandboxState::Unknown(_) => ("?", t.muted),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::THEME;

    #[test]
    fn format_bytes_scales_and_trim() {
        assert_eq!(format_bytes(0), "0B");
        assert_eq!(format_bytes(512), "512B");
        assert_eq!(format_bytes(1024), "1K");
        assert_eq!(format_bytes(1229), "1.2K");
        assert_eq!(format_bytes(3 * 1024 * 1024), "3M");
        assert_eq!(format_bytes(3_565_158), "3.4M");
        assert_eq!(format_bytes(6 * 1024 * 1024 * 1024), "6G");
    }

    #[test]
    fn format_duration_buckets() {
        assert_eq!(format_duration(0.0), "0s");
        assert_eq!(format_duration(12.0), "12s");
        assert_eq!(format_duration(59.9), "59s");
        assert_eq!(format_duration(60.0), "1m 0s");
        assert_eq!(format_duration(332.0), "5m 32s");
        assert_eq!(format_duration(4980.0), "1h 23m");
        assert_eq!(format_duration(180_000.0), "2d 2h");
        assert_eq!(format_duration(-5.0), "0s");
    }

    #[test]
    fn state_indicator_maps_states() {
        let t = &THEME;
        assert_eq!(state_indicator(&SandboxState::Running), ("●", t.ok));
        assert_eq!(state_indicator(&SandboxState::Stopped), ("○", t.text));
        assert_eq!(state_indicator(&SandboxState::Paused), ("⏸", t.warn));
        assert_eq!(state_indicator(&SandboxState::Exited), ("✗", t.err));
        assert_eq!(
            state_indicator(&SandboxState::Unknown("weird".into())),
            ("?", t.muted)
        );
    }

    #[test]
    fn ports_short_renders_and_defaults() {
        assert_eq!(ports_short(None), "—");
        assert_eq!(ports_short(Some(&Vec::new())), "—");
        let ports = vec![
            PublishedPort {
                host_bind: "127.0.0.1".into(),
                host_port: 8080,
                guest_port: 80,
                protocol: "tcp".into(),
            },
            PublishedPort {
                host_bind: "0.0.0.0".into(),
                host_port: 9090,
                guest_port: 90,
                protocol: "udp".into(),
            },
        ];
        assert_eq!(ports_short(Some(&ports)), "8080→80, 9090→90/udp");
    }

    #[test]
    fn card_columns_is_responsive() {
        assert_eq!(card_columns(30), 1);
        assert_eq!(card_columns(60), 2);
        assert_eq!(card_columns(120), 3);
    }
}
