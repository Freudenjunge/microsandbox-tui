//! Help overlay: centered keybinding table shown with `?`.
//!
//! NOTE(dead_code): consumers arrive in Task 10.

#![allow(dead_code)]

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph};

use crate::ui::theme::THEME;

/// One row of the keybinding table.
struct KeyRow {
    keys: &'static str,
    action: &'static str,
}

/// One context group in the help overlay.
struct KeyGroup {
    title: &'static str,
    rows: &'static [KeyRow],
}

static GLOBAL_KEYS: &[KeyRow] = &[
    KeyRow {
        keys: "q",
        action: "quit",
    },
    KeyRow {
        keys: "?",
        action: "help overlay",
    },
];

static DASHBOARD_KEYS: &[KeyRow] = &[
    KeyRow {
        keys: "↑ ↓",
        action: "select sandbox card (left rail)",
    },
    KeyRow {
        keys: "1 / 2 / 3",
        action: "detail tabs: overview / logs / ports",
    },
    KeyRow {
        keys: "Tab",
        action: "cycle detail tabs",
    },
    KeyRow {
        keys: "c",
        action: "create sandbox (form)",
    },
    KeyRow {
        keys: "x",
        action: "exec: interactive shell into the sandbox (new window)",
    },
    KeyRow {
        keys: "s",
        action: "start/stop the selected sandbox (state-dependent)",
    },
    KeyRow {
        keys: "r",
        action: "restart sandbox",
    },
    KeyRow {
        keys: "Del",
        action: "remove sandbox (confirm)",
    },
    KeyRow {
        keys: "U",
        action: "install/update microsandbox runtime",
    },
];

static LOGS_KEYS: &[KeyRow] = &[
    KeyRow {
        keys: "f",
        action: "toggle follow",
    },
    KeyRow {
        keys: "g / /",
        action: "grep filter",
    },
    KeyRow {
        keys: "s",
        action: "source filter (stdout/stderr/system)",
    },
    KeyRow {
        keys: "↑ ↓ / PgUp PgDn",
        action: "scroll buffer",
    },
];

static PORTS_KEYS: &[KeyRow] = &[
    KeyRow {
        keys: "+",
        action: "publish a port on THIS sandbox (confirm; recreates)",
    },
    KeyRow {
        keys: "-",
        action: "unpublish selected binding (confirm; recreates)",
    },
    KeyRow {
        keys: "↑ ↓",
        action: "select binding",
    },
    KeyRow {
        keys: "r",
        action: "refresh bindings from inspect",
    },
];

static GROUPS: &[KeyGroup] = &[
    KeyGroup {
        title: "Global",
        rows: GLOBAL_KEYS,
    },
    KeyGroup {
        title: "Dashboard",
        rows: DASHBOARD_KEYS,
    },
    KeyGroup {
        title: "Logs",
        rows: LOGS_KEYS,
    },
    KeyGroup {
        title: "Ports",
        rows: PORTS_KEYS,
    },
];

/// Render the help overlay centered inside `area`.
pub fn render(frame: &mut Frame, area: Rect) {
    // Paint an opaque backdrop instead of `Clear`, which would reset cells
    // to a transparent `Reset` background.
    let t = &THEME;
    frame.render_widget(Block::default().style(Style::default().bg(THEME.bg)), area);

    // Compute a centered box that fits the content.
    let total_rows: u16 = GROUPS
        .iter()
        .map(|g| 1 + g.rows.len() as u16 + 1) // title + rows + blank
        .sum::<u16>()
        + 1; // closing border
    let height = total_rows.min(area.height);
    let width = 60.min(area.width);
    let centered = center_rect(area, width, height);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " Keybindings ",
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(t.accent))
        .padding(Padding::horizontal(2));

    let mut lines: Vec<Line> = Vec::new();
    for group in GROUPS {
        lines.push(Line::from(Span::styled(
            group.title,
            Style::default()
                .fg(t.warn)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )));
        for row in group.rows {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {:<8}", row.keys),
                    Style::default().fg(t.fg).add_modifier(Modifier::BOLD),
                ),
                Span::styled(row.action, Style::default().fg(t.text)),
            ]));
        }
        lines.push(Line::raw(""));
    }
    // Replace last blank with closing hint.
    if let Some(last) = lines.last_mut() {
        *last = Line::from(vec![
            Span::raw("  "),
            Span::styled(
                "Esc / ?",
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  close", Style::default().fg(t.muted)),
        ]);
    }

    let para = Paragraph::new(lines).block(block);
    frame.render_widget(para, centered);
}

/// Return a `Rect` of `w`×`h` centered inside `area`.
fn center_rect(area: Rect, w: u16, h: u16) -> Rect {
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}
