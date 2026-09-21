//! Shared application chrome: title bar, tab bar, update banner, footer.
//!
//! Every top-level view composes these four bands around its content so the
//! frame reads like one consistent terminal application (after the Stitch
//! mockups in `design/stitch_microsandbox_tui_design_system/`).
//!
//! Rendering is intentionally thin: the span-building helpers are pure and
//! unit-tested, the render functions only split areas and draw lines.

#![allow(dead_code)]

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::ui::theme::THEME;

// ---------- title bar ----------

/// Everything the title bar renders.
#[derive(Debug, Default, Clone)]
pub struct TitleInfo {
    /// Installed (or advertised) runtime version, e.g. `0.7.2`.
    pub runtime_version: Option<String>,
    /// Terminal size string, e.g. `120x34`.
    pub size: String,
    /// Process id of the TUI.
    pub pid: u32,
    /// Whether the backend last reported a healthy daemon connection.
    pub live: bool,
}

/// Render the title bar into a 1-row `area`.
pub fn render_title_bar(frame: &mut Frame, info: &TitleInfo, area: Rect) {
    frame.render_widget(title_bar_line(info), area);
}

/// Build the title-bar line: name, version, session info.
fn title_bar_line(info: &TitleInfo) -> Line<'static> {
    let t = &THEME;
    let version = info.runtime_version.as_deref().unwrap_or("?").to_string();
    Line::from(vec![
        Span::styled(
            "microsandbox",
            Style::default().fg(t.fg).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" v{version}"), Style::default().fg(t.accent)),
        Span::styled(
            format!("  [session: tty1 • {} • pid: {}]", info.size, info.pid),
            Style::default().fg(t.muted),
        ),
    ])
}

// ---------- tab bar ----------

/// One tab in the tab bar.
pub struct Tab<'a> {
    /// Number key that switches to this view (`1`..).
    pub key: char,
    /// Label, e.g. `SANDBOXES`.
    pub label: &'a str,
    /// Whether this tab is the active view.
    pub active: bool,
    /// Optional counter rendered after the label (e.g. sandbox count).
    pub count: Option<usize>,
}

/// Render the tab bar plus right-aligned runtime/daemon info into a 3-row area.
pub fn render_tab_bar(frame: &mut Frame, tabs: &[Tab<'_>], area: Rect) {
    // Row 0: breathing room (the mockup's taller nav band).
    // Row 1: the tabs themselves.
    // Row 2: the underline rail (full-width border bottom).
    if area.height < 2 {
        frame.render_widget(tab_bar_line(tabs), area);
        return;
    }
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .split(area);
    frame.render_widget(Paragraph::new(Line::from(vec![Span::raw(" ")])), rows[0]);
    frame.render_widget(tab_bar_line(tabs), rows[1]);

    // Underline rail: accent under the active tab, muted elsewhere.
    let t = &THEME;
    let mut rail = String::new();
    for tab in tabs {
        let count = tab.count.map(|c| format!(" ({c})")).unwrap_or_default();
        let width = 2 + tab.key.len_utf8() + 1 + tab.label.len() + count.len() + 1;
        for _ in 0..width {
            rail.push('─');
        }
        rail.push(' ');
    }
    // Render rail segment-by-segment: the active tab's segment is accent.
    let mut spans = Vec::new();
    let mut offset = 0usize;
    for tab in tabs {
        let count = tab.count.map(|c| format!(" ({c})")).unwrap_or_default();
        let width = 2 + tab.key.len_utf8() + 1 + tab.label.len() + count.len() + 1 + 1; // +1 space
        let seg = &rail[offset..offset + width.min(rail.len() - offset)];
        spans.push(Span::styled(
            seg.to_string(),
            if tab.active {
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(t.border)
            },
        ));
        offset += width;
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), rows[2]);
}

/// Build the tab-bar line with the active tab highlighted as a filled pill.
fn tab_bar_line(tabs: &[Tab<'_>]) -> Line<'static> {
    let t = &THEME;
    let mut spans = Vec::new();
    for tab in tabs {
        let count = tab.count.map(|c| format!(" ({c})")).unwrap_or_default();
        let label = format!("[{}] {}", tab.key, tab.label);
        let count_str = count;
        if tab.active {
            spans.push(Span::styled(
                format!(" {label}{count_str} "),
                // Filled pill: accent on dark — mirrors the mockup's active
                // tab; `bg` spans need the reversed pair to read cleanly.
                Style::default()
                    .fg(t.bg)
                    .bg(t.accent)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                format!(" {label}{count_str} "),
                Style::default().fg(t.muted),
            ));
        }
        spans.push(Span::raw(" "));
    }
    Line::from(spans)
}

// ---------- banner ----------

/// Render the runtime update/missing banner as a full-width warn band.
pub fn render_banner(frame: &mut Frame, text: &str, area: Rect) {
    let t = &THEME;
    let line = Line::from(vec![
        Span::styled(
            " ⚠ ",
            Style::default()
                .fg(t.bg)
                .bg(t.warn)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{text}  [U] update/install"),
            Style::default().fg(t.warn),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(t.panel)),
        area,
    );
}

/// Render an error line in the error color.
pub fn render_error_line(frame: &mut Frame, text: &str, area: Rect) {
    let t = &THEME;
    let line = Line::from(vec![
        Span::styled(
            " ✗ ",
            Style::default().fg(t.err).add_modifier(Modifier::BOLD),
        ),
        Span::styled(text.to_string(), Style::default().fg(t.err)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

/// Render a transient status line (spinner text, op result).
pub fn render_status_line(frame: &mut Frame, text: &str, area: Rect) {
    let t = &THEME;
    let line = Line::from(vec![
        Span::styled(
            " ● ",
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(text.to_string(), Style::default().fg(t.text)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

// ---------- footer ----------

/// One `[key] label` hint in the footer.
pub struct FooterHint<'a> {
    /// Key label including brackets, e.g. `[c]`.
    pub key: &'a str,
    /// Action description, e.g. `Create`.
    pub label: &'a str,
    /// Color role for the key (`accent`/`err`/`warn`/default `fg`).
    pub role: FooterRole,
}

/// Color role of a footer hint key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FooterRole {
    /// Cyan interactive hint.
    #[default]
    Accent,
    /// Destructive action (red).
    Err,
    /// Caution (yellow).
    Warn,
    /// Neutral (bright text).
    Plain,
}

/// Render the footer into a 2-row area: hints + navigation legend.
pub fn render_footer(frame: &mut Frame, hints: &[FooterHint<'_>], area: Rect) {
    if area.height < 2 {
        frame.render_widget(footer_line(hints), area);
        return;
    }
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(area);
    frame.render_widget(footer_line(hints), rows[0]);
    frame.render_widget(navigation_line(), rows[1]);
}

/// The navigation legend line: `Navigation: Tab / ↑↓←→` right-aligned.
fn navigation_line() -> Line<'static> {
    let t = &THEME;
    Line::from(vec![
        Span::styled(" Navigation: ", Style::default().fg(t.muted)),
        Span::styled(
            "Tab",
            Style::default().fg(t.fg).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" / ", Style::default().fg(t.muted)),
        Span::styled(
            "↑↓←→",
            Style::default().fg(t.fg).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  •  UTF-8 • 24-bit Truecolor", Style::default().fg(t.ok)),
    ])
}

/// Build the footer hint line with per-role colored keys.
fn footer_line(hints: &[FooterHint<'_>]) -> Line<'static> {
    let t = &THEME;
    let mut spans = Vec::new();
    for (i, hint) in hints.iter().enumerate() {
        let key_style = match hint.role {
            FooterRole::Accent => Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
            FooterRole::Err => Style::default().fg(t.err).add_modifier(Modifier::BOLD),
            FooterRole::Warn => Style::default().fg(t.warn).add_modifier(Modifier::BOLD),
            FooterRole::Plain => Style::default().fg(t.fg).add_modifier(Modifier::BOLD),
        };
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(hint.key.to_string(), key_style));
        spans.push(Span::styled(
            format!(" {}", hint.label),
            Style::default().fg(t.text),
        ));
    }
    Line::from(spans)
}

// ---------- shared layout ----------

/// A zoned body: ONE bordered surface split by separator lines (the mockup's
/// panels are areas inside a common frame, not individually boxed cards).
pub struct ZonedLayout {
    /// The outer frame rect (borders drawn by [`render_zone_frame`]).
    pub frame: Rect,
    /// Interior rows of the frame (borders excluded; each `sep`-th row is a
    /// separator owned by the frame).
    pub rows: Vec<Rect>,
}

/// Split `area` into a zoned layout: outer border + 1-row horizontal
/// separators between `weights` rows (relative row heights).
pub fn zone_rows(area: Rect, weights: &[u16]) -> ZonedLayout {
    let inner = Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    // N rows need N-1 separator rows in between.
    let seps = weights.len().saturating_sub(1);
    let sep_total: u16 = seps as u16;
    let content_h = inner.height.saturating_sub(sep_total);
    let weight_sum: u16 = weights.iter().sum();
    let mut rows = Vec::with_capacity(weights.len());
    let mut y = inner.y;
    for (i, w) in weights.iter().enumerate() {
        // Proportional share of the remaining height; last row gets the rest.
        let h = if i + 1 == weights.len() {
            inner.y + inner.height - y
        } else {
            content_height_share(content_h, *w, weight_sum)
        };
        rows.push(Rect::new(inner.x, y, inner.width, h));
        y += h + 1; // +1 separator row
    }
    ZonedLayout { frame: area, rows }
}

/// Proportional share helper (u16 safe).
fn content_height_share(content: u16, weight: u16, sum: u16) -> u16 {
    if sum == 0 {
        0
    } else {
        (u32::from(content) * u32::from(weight) / u32::from(sum)) as u16
    }
}

/// Render the zoned frame: outer border + horizontal separators between the
/// given interior rows.
pub fn render_zone_frame(frame: &mut Frame, layout: &ZonedLayout, area: Rect) {
    let t = &THEME;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.border));
    frame.render_widget(block, area);
    // Horizontal separators at the top edge of each row except the first.
    for row in layout.rows.iter().skip(1) {
        let sep_y = row.y.saturating_sub(1);
        if sep_y <= area.y || sep_y >= area.y + area.height.saturating_sub(1) {
            continue;
        }
        let sep = Rect::new(area.x + 1, sep_y, area.width.saturating_sub(2), 1);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "─".repeat(sep.width as usize),
                Style::default().fg(t.border),
            ))),
            sep,
        );
    }
}

/// The chrome bands shared by every full-screen view: title bar, tab bar,
/// optional banner/error/status, then the body and the footer.
///
/// Returns `(title, tabs, banner|None, status|None, body, footer)`.
#[allow(clippy::type_complexity)]
pub fn chrome_layout(
    area: Rect,
    banner: Option<&str>,
    status: Option<&str>,
) -> (Rect, Rect, Option<Rect>, Option<Rect>, Rect, Rect) {
    let banner_h = u16::from(banner.is_some());
    let status_h = u16::from(status.is_some());
    // With both optionals present the indices are: banner=2, status=3,
    // body=4, footer=5. Each missing optional shifts the following bands up
    // one slot in the constraint list (we never emit `Length(0)` rows).
    let mut constraints = vec![
        Constraint::Length(1), // title bar
        Constraint::Length(3), // tab bar (taller nav band + underline rail)
    ];
    if banner_h > 0 {
        constraints.push(Constraint::Length(1));
    }
    if status_h > 0 {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Min(1));
    constraints.push(Constraint::Length(2)); // footer: hints + nav legend

    let chunks = Layout::vertical(constraints).split(area);

    let mut idx = 2;
    let banner_area = (banner_h > 0).then(|| {
        idx += 1;
        chunks[idx - 1]
    });
    let status_area = (status_h > 0).then(|| {
        idx += 1;
        chunks[idx - 1]
    });
    let body = chunks[idx];
    let footer = chunks[idx + 1];
    (chunks[0], chunks[1], banner_area, status_area, body, footer)
}

// ---------- pure helpers (unit-tested) ----------

/// The tab bar spans for a set of tabs (visible text, no styling).
pub fn tab_bar_text(tabs: &[Tab<'_>]) -> String {
    tab_bar_line(tabs)
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect()
}

/// The title bar spans (visible text, no styling).
pub fn title_bar_text(info: &TitleInfo) -> String {
    title_bar_line(info)
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect()
}

/// The footer spans (visible text, no styling).
pub fn footer_text(hints: &[FooterHint<'_>]) -> String {
    footer_line(hints)
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_bar_contains_name_version() {
        let info = TitleInfo {
            runtime_version: Some("0.7.2".into()),
            size: "120x34".into(),
            pid: 42,
            live: true,
        };
        let text = title_bar_text(&info);
        assert!(text.contains("microsandbox"), "{text}");
        assert!(text.contains("v0.7.2"), "{text}");
        assert!(text.contains("120x34"), "{text}");
        assert!(text.contains("pid: 42"), "{text}");
        // Fake window chrome is gone: no clock, no traffic dots.
        assert!(!text.contains("●"), "{text}");
    }

    #[test]
    fn title_bar_without_version_shows_placeholder() {
        let info = TitleInfo {
            runtime_version: None,
            size: "80x24".into(),
            pid: 1,
            live: true,
        };
        assert!(title_bar_text(&info).contains("v?"));
    }

    #[test]
    fn tab_bar_marks_active_tab_with_count() {
        let tabs = vec![
            Tab {
                key: '1',
                label: "SANDBOXES",
                active: true,
                count: Some(4),
            },
            Tab {
                key: '2',
                label: "LOGS",
                active: false,
                count: None,
            },
        ];
        let text = tab_bar_text(&tabs);
        assert!(text.contains("[1] SANDBOXES (4)"), "{text}");
        assert!(text.contains("[2] LOGS "), "{text}");
    }

    #[test]
    fn zone_rows_splits_with_separators() {
        let area = Rect::new(0, 0, 40, 12);
        let layout = zone_rows(area, &[2, 1]);
        assert_eq!(layout.rows.len(), 2);
        // Interior height is 12 - 2 (borders) - 1 (separator) = 9; split 2:1.
        assert_eq!(layout.rows[0].height, 6);
        assert_eq!(layout.rows[1].height, 3);
        // Separator row sits between the two rows.
        assert_eq!(
            layout.rows[1].y,
            layout.rows[0].y + layout.rows[0].height + 1
        );
        // Rows live inside the border.
        assert_eq!(layout.rows[0].x, area.x + 1);
        assert_eq!(layout.rows[0].width, area.width - 2);
    }

    #[test]
    fn footer_renders_hints_in_order() {
        let hints = vec![
            FooterHint {
                key: "[c]",
                label: "Create",
                role: FooterRole::Accent,
            },
            FooterHint {
                key: "[Del]",
                label: "Destroy",
                role: FooterRole::Err,
            },
        ];
        let text = footer_text(&hints);
        assert!(text.starts_with("[c] Create"), "{text}");
        assert!(text.contains("  [Del] Destroy"), "{text}");
    }

    #[test]
    fn chrome_layout_splits_optional_bands() {
        let area = Rect::new(0, 0, 80, 24);
        let (_, _, banner, status, body, footer) =
            chrome_layout(area, Some("update available"), Some("busy…"));
        assert_eq!(banner.map(|r| r.height), Some(1));
        assert_eq!(status.map(|r| r.height), Some(1));
        assert_eq!(footer.height, 2, "footer carries the nav legend");
        assert_eq!(body.height, 24 - 8);

        let (_, _, banner, status, _, _) = chrome_layout(area, None, None);
        assert!(banner.is_none());
        assert!(status.is_none());

        // Status-only layout must still find a 1-row status band.
        let (_, _, banner, status, _, _) = chrome_layout(area, None, Some("x"));
        assert!(banner.is_none());
        assert_eq!(status.map(|r| r.height), Some(1));
    }
}
