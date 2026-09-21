//! Help overlay: centered keybinding table shown with `?`.
//!
//! NOTE(dead_code): consumers arrive in Task 10.

#![allow(dead_code)]

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph};

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
        keys: "Tab",
        action: "cycle views (dashboard → volumes → snapshots)",
    },
    KeyRow {
        keys: "?",
        action: "help overlay",
    },
];

static DASHBOARD_KEYS: &[KeyRow] = &[
    KeyRow {
        keys: "↑ ↓",
        action: "select sandbox card",
    },
    KeyRow {
        keys: "Enter",
        action: "inspect selected sandbox",
    },
    KeyRow {
        keys: "c",
        action: "create sandbox form",
    },
    KeyRow {
        keys: "e",
        action: "exec command in sandbox",
    },
    KeyRow {
        keys: "l",
        action: "logs panel",
    },
    KeyRow {
        keys: "s",
        action: "SSH into sandbox",
    },
    KeyRow {
        keys: "p",
        action: "port forwards view",
    },
    KeyRow {
        keys: "n",
        action: "network rules editor",
    },
    KeyRow {
        keys: "r",
        action: "restart sandbox",
    },
    KeyRow {
        keys: "x",
        action: "stop sandbox",
    },
    KeyRow {
        keys: "Delete",
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
        keys: "g",
        action: "grep filter",
    },
    KeyRow {
        keys: "t",
        action: "tail count",
    },
    KeyRow {
        keys: "s",
        action: "source filter (stdout/stderr/system)",
    },
    KeyRow {
        keys: "Esc",
        action: "back to dashboard",
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
];

/// Render the help overlay centered inside `area`.
pub fn render(frame: &mut Frame, area: Rect) {
    // Paint an opaque backdrop instead of `Clear`, which would reset cells
    // to a transparent `Reset` background.
    frame.render_widget(
        Block::default().style(Style::default().bg(crate::ui::BG)),
        area,
    );

    // Compute a centered box that fits the content.
    let total_rows: u16 = GROUPS
        .iter()
        .map(|g| 1 + g.rows.len() as u16 + 1) // title + rows + blank
        .sum::<u16>()
        + 1; // closing border
    let height = total_rows.min(area.height);
    let width = 56.min(area.width);
    let centered = center_rect(area, width, height);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " Keybindings ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(Color::Cyan))
        .padding(Padding::horizontal(2));

    let mut lines: Vec<Line> = Vec::new();
    for group in GROUPS {
        lines.push(Line::from(Span::styled(
            group.title,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )));
        for row in group.rows {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {:<8}", row.keys),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(row.action, Style::default().fg(Color::Gray)),
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
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  close", Style::default().fg(Color::DarkGray)),
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
