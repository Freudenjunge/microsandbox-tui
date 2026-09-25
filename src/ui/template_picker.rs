//! Template picker: the master-detail create entry (variant C from the
//! templates spec). Left: template list with descriptions; right: the
//! effective values the template would create, defaults flagged. Small
//! terminals stack the panes; very short ones show the list only.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::template::{LayoutKind, Template, preview_values, template_layout};
use crate::ui::theme::THEME;

/// Render the template picker for the create flow.
pub fn render(frame: &mut Frame, area: Rect, templates: &[Template], selected: usize) {
    let runtime_version = match crate::runtime::detect() {
        crate::runtime::RuntimeStatus::Installed { runtime, .. } => {
            runtime.version.as_ref().map(|v| v.to_string())
        }
        crate::runtime::RuntimeStatus::Missing => None,
    };
    let title = crate::ui::chrome::TitleInfo {
        runtime_version,
        size: format!("{}x{}", area.width, area.height),
        pid: std::process::id(),
        live: true,
    };
    let areas = crate::ui::chrome::chrome_layout(area, None, None);

    let tabs = vec![
        crate::ui::chrome::Tab {
            key: '1',
            label: "SANDBOXES",
            active: false,
            count: None,
        },
        crate::ui::chrome::Tab {
            key: 'C',
            label: "CREATE",
            active: true,
            count: None,
        },
    ];
    crate::ui::chrome::render_chrome(frame, &areas, &title, &tabs);

    match template_layout(areas.body.width, areas.body.height) {
        LayoutKind::MasterDetail => {
            let [list, preview] = split_h(areas.body, 40);
            render_list(frame, list, templates, selected);
            render_preview(frame, preview, templates, selected);
        }
        LayoutKind::Stacked => {
            let [list, preview] = split_v(areas.body, 40);
            render_list(frame, list, templates, selected);
            render_preview(frame, preview, templates, selected);
        }
        LayoutKind::ListOnly => render_list(frame, areas.body, templates, selected),
    }

    let hints = vec![
        crate::ui::chrome::FooterHint {
            key: "[Enter]",
            label: "erstellen",
            role: crate::ui::chrome::FooterRole::Accent,
        },
        crate::ui::chrome::FooterHint {
            key: "[e]",
            label: "anpassen",
            role: crate::ui::chrome::FooterRole::Plain,
        },
        crate::ui::chrome::FooterHint {
            key: "[n]",
            label: "neu",
            role: crate::ui::chrome::FooterRole::Plain,
        },
        crate::ui::chrome::FooterHint {
            key: "[d]",
            label: "löschen",
            role: crate::ui::chrome::FooterRole::Warn,
        },
        crate::ui::chrome::FooterHint {
            key: "[Esc]",
            label: "←",
            role: crate::ui::chrome::FooterRole::Err,
        },
    ];
    crate::ui::chrome::render_chrome_footer(frame, &areas, &hints);
}

fn split_h(area: Rect, percent: u16) -> [Rect; 2] {
    let [a, b] =
        Layout::horizontal([Constraint::Percentage(percent), Constraint::Fill(1)]).areas(area);
    [a, b]
}

fn split_v(area: Rect, percent: u16) -> [Rect; 2] {
    let [a, b] =
        Layout::vertical([Constraint::Percentage(percent), Constraint::Fill(1)]).areas(area);
    [a, b]
}

/// The template list: name (bold when selected) + description below, with
/// a `(built-in)` marker. No panic on an empty list — a hint is shown.
fn render_list(frame: &mut Frame, area: Rect, templates: &[Template], selected: usize) {
    let t = &THEME;
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " Templates ",
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(t.accent))
        .style(Style::default().bg(t.panel));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if templates.is_empty() {
        let p = Paragraph::new(vec![Line::from(Span::styled(
            "  keine Templates geladen",
            Style::default().fg(t.muted),
        ))])
        .block(Block::default().style(Style::default().bg(t.panel)));
        frame.render_widget(p, inner);
        return;
    }

    let mut lines = Vec::with_capacity(templates.len() * 2);
    for (i, tpl) in templates.iter().enumerate() {
        let is_sel = i == selected;
        let marker = if is_sel { "▶ " } else { "  " };
        let name_style = if is_sel {
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.fg)
        };
        let mut name_spans = vec![
            Span::raw("  "),
            Span::styled(format!("{marker}{}", tpl.meta.name), name_style),
        ];
        if tpl.built_in {
            name_spans.push(Span::styled("  (built-in)", Style::default().fg(t.muted)));
        }
        lines.push(Line::from(name_spans));
        lines.push(Line::from(Span::styled(
            format!("    {}", tpl.meta.description),
            Style::default().fg(t.muted),
        )));
    }
    let p = Paragraph::new(lines)
        .block(Block::default().style(Style::default().bg(t.panel)))
        .wrap(Wrap { trim: true });
    frame.render_widget(p, inner);
}

/// The preview pane: effective values with `(default)` markers (Spec §3).
fn render_preview(frame: &mut Frame, area: Rect, templates: &[Template], selected: usize) {
    let t = &THEME;
    let name = templates
        .get(selected)
        .map(|tpl| tpl.meta.name.as_str())
        .unwrap_or("—");
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            format!(" Vorschau: {name} "),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(t.accent))
        .style(Style::default().bg(t.panel));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(tpl) = templates.get(selected) else {
        return;
    };
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut lines = Vec::new();
    for row in preview_values(tpl, &cwd) {
        let mut spans = vec![
            Span::raw("  "),
            Span::styled(format!("{:<7}", row.label), Style::default().fg(t.muted)),
            Span::styled(row.value.clone(), Style::default().fg(t.text)),
        ];
        if row.is_default {
            spans.push(Span::styled("  (default)", Style::default().fg(t.muted)));
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "  Enter erstellt die Sandbox direkt.",
        Style::default().fg(t.muted),
    )));
    let p = Paragraph::new(lines)
        .block(Block::default().style(Style::default().bg(t.panel)))
        .wrap(Wrap { trim: false });
    frame.render_widget(p, inner);
}
