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
    let (width, height) = confirm_size(message, area);
    let vert = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(height),
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
    // Word-wrap the message so long ops (publish/unpublish with the
    // snapshot explanation) are fully visible instead of clipped.
    let wrapped = textwrap(message, usize::from(width).saturating_sub(4));
    let mut text = Vec::with_capacity(wrapped.len() + 2);
    for line in wrapped {
        text.push(Line::from(Span::raw(line)));
    }
    text.push(Line::from(""));
    text.push(Line::from(vec![
        Span::styled("y", Style::default().fg(t.ok).add_modifier(Modifier::BOLD)),
        Span::raw(" = yes   "),
        Span::styled(
            "n/Esc",
            Style::default().fg(t.err).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" = no"),
    ]));
    frame.render_widget(Paragraph::new(text).block(block), horiz[1]);
}

/// Dialog dimensions for a confirm message: wraps `message` into
/// `CONFIRM_MAX_WIDTH`-wide lines (border + padding subtracted), so the
/// whole text is visible. Height grows with the wrapped line count and
/// clamps to the terminal.
///
/// Pure helper (unit-tested); `render_confirm` draws from these numbers.
pub fn confirm_size(message: &str, area: Rect) -> (u16, u16) {
    const MIN_WIDTH: u16 = 40;
    let max_w = CONFIRM_MAX_WIDTH.min(area.width.saturating_sub(2));
    let inner = max_w.saturating_sub(4); // borders + padding
    let lines = textwrap(message, inner.max(8) as usize);
    let longest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let width = (longest as u16 + 4).clamp(MIN_WIDTH, max_w);
    // wrapped lines + blank + key hints + 2 border rows
    let height = (lines.len() as u16 + 4).min(area.height.saturating_sub(2));
    (width, height.max(5))
}

/// Target dialog width for confirm messages (inner text area).
pub const CONFIRM_MAX_WIDTH: u16 = 64;

/// Greedy word-wrap `text` to `width` columns (pure helper, unit-tested).
pub fn textwrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let wlen = word.chars().count();
        let clen = current.chars().count();
        if !current.is_empty() && clen + 1 + wlen > width {
            out.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        if wlen > width {
            // A single token longer than the line: hard-split it.
            let chars: Vec<char> = word.chars().collect();
            for chunk in chars.chunks(width.max(1)) {
                let s: String = chunk.iter().collect();
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
                current = s;
            }
        } else {
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    // ---- confirm dialog sizing (smoke finding: message was clipped) ----

    fn term() -> Rect {
        Rect::new(0, 0, 120, 34)
    }

    #[test]
    fn confirm_size_fits_long_publish_message() {
        // The real publish message (≈105 chars) must fully fit: it wraps
        // instead of clipping, and the dialog grows in height.
        let msg = "Publish 127.0.0.1:8888→80/tcp on 'Test'? (snapshots the disk, recreates from it — your data is preserved, processes restart)";
        let (w, h) = confirm_size(msg, term());
        assert!(w <= CONFIRM_MAX_WIDTH, "width {w} exceeds max");
        assert!(h >= 7, "wrapped message needs more rows: {h}");
        // Every wrapped line fits the inner width.
        let inner = w.saturating_sub(4) as usize;
        for line in textwrap(msg, inner) {
            assert!(line.chars().count() <= inner, "line too wide: {line:?}");
        }
    }

    #[test]
    fn confirm_size_short_message_stays_compact() {
        let (w, h) = confirm_size("Stop sandbox 'web'?", term());
        assert_eq!(h, 5, "short message = 3 text rows + 2 borders");
        assert!(w >= 40, "keeps a minimum width: {w}");
    }

    #[test]
    fn confirm_size_clamps_to_small_terminals() {
        let small = Rect::new(0, 0, 50, 12);
        let msg = "Publish 127.0.0.1:8888→80/tcp on 'Test'? (snapshots the disk, recreates from it — your data is preserved, processes restart)";
        let (w, h) = confirm_size(msg, small);
        assert!(w <= 48, "dialog must fit the terminal: {w}");
        assert!(h <= 10, "height must clamp: {h}");
    }

    #[test]
    fn textwrap_breaks_on_words() {
        let lines = textwrap("alpha beta gamma delta", 11);
        assert_eq!(
            lines,
            vec!["alpha beta", "gamma delta"],
            "greedy wrap at word boundaries"
        );
        // A word longer than the width is hard-split, never lost.
        let lines = textwrap("supercalifragilistic", 8);
        assert_eq!(lines, vec!["supercal", "ifragili", "stic"]);
        // Whitespace-only text yields one empty line, not zero lines.
        assert_eq!(textwrap("   ", 10), vec![String::new()]);
    }

    #[test]
    fn confirm_dialog_renders_full_message_offscreen() {
        // Regression: the publish confirm's explanation must appear in the
        // rendered buffer — before the fix the Paragraph clipped it.
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(120, 34)).unwrap();
        let msg = "Publish 127.0.0.1:8888→80/tcp on 'Test'? (snapshots the disk, recreates from it — your data is preserved, processes restart)";
        terminal
            .draw(|f| {
                render_confirm(f, msg, f.area());
            })
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let text = (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains("your data is preserved"),
            "message tail missing from the dialog:\n{text}"
        );
    }
}
