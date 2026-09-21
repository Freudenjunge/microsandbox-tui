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
use ratatui::widgets::Paragraph;

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

/// Render the tab bar plus right-aligned runtime/daemon info into a 1-row area.
pub fn render_tab_bar(frame: &mut Frame, tabs: &[Tab<'_>], area: Rect) {
    frame.render_widget(tab_bar_line(tabs), area);
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

/// Render the footer keyhint bar into a 1-row area.
pub fn render_footer(frame: &mut Frame, hints: &[FooterHint<'_>], area: Rect) {
    frame.render_widget(footer_line(hints), area);
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
        Constraint::Length(1), // tab bar
    ];
    if banner_h > 0 {
        constraints.push(Constraint::Length(1));
    }
    if status_h > 0 {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Min(1));
    constraints.push(Constraint::Length(1));

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
        assert_eq!(footer.height, 1);
        assert_eq!(body.height, 24 - 5);

        let (_, _, banner, status, _, _) = chrome_layout(area, None, None);
        assert!(banner.is_none());
        assert!(status.is_none());

        // Status-only layout must still find a 1-row status band.
        let (_, _, banner, status, _, _) = chrome_layout(area, None, Some("x"));
        assert!(banner.is_none());
        assert_eq!(status.map(|r| r.height), Some(1));
    }
}
