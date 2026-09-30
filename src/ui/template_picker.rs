//! Template picker: the master-detail create entry (variant C from the
//! templates spec). Left: template list with descriptions; right: the
//! effective values the template would create, defaults flagged. Small
//! terminals stack the panes; very short ones show the list only.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::template::{LayoutKind, Template, preview_values, template_layout};
use crate::ui::theme::THEME;

/// The pseudo-entry at picker row 0: open the empty form (not a template).
const SCRATCH_ROW_NAME: &str = "Create from scratch";

/// Render the template picker for the create flow. Row 0 is the
/// "Create from scratch" pseudo-entry; template rows sit at index + 1.
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
            label: "create",
            role: crate::ui::chrome::FooterRole::Accent,
        },
        crate::ui::chrome::FooterHint {
            key: "[e]",
            label: "customize",
            role: crate::ui::chrome::FooterRole::Plain,
        },
        crate::ui::chrome::FooterHint {
            key: "[n]",
            label: "empty form",
            role: crate::ui::chrome::FooterRole::Plain,
        },
        crate::ui::chrome::FooterHint {
            key: "[d]",
            label: "delete",
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

    // One line per entry — names only. Descriptions/details live in the
    // preview pane (Review 2.13: grayed description rows duplicated it).
    // The window follows the selection (same rule as the form's image
    // picker); long names truncate — Review Focus 5 decided truncate.
    let mut rows: Vec<(&str, bool)> = Vec::with_capacity(templates.len() + 1);
    rows.push((SCRATCH_ROW_NAME, false));
    for t in templates {
        rows.push((t.meta.name.as_str(), t.built_in));
    }
    let mut lines = Vec::with_capacity(rows.len());
    if templates.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (no templates loaded — only scratch available)",
            Style::default().fg(t.muted),
        )));
    }
    for (i, (name, built_in)) in rows.iter().enumerate() {
        let is_sel = i == selected;
        let marker = if is_sel { "▶ " } else { "  " };
        let name_style = if is_sel {
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.fg)
        };
        let mut name_spans = vec![
            Span::raw("  "),
            Span::styled(format!("{marker}{name}"), name_style),
        ];
        if *built_in {
            name_spans.push(Span::styled("  (built-in)", Style::default().fg(t.muted)));
        }
        lines.push(Line::from(name_spans));
    }
    let entries_visible = inner.height.max(1) as usize;
    let offset_entries = selected.saturating_sub(entries_visible.saturating_sub(1));
    let p = Paragraph::new(lines)
        .block(Block::default().style(Style::default().bg(t.panel)))
        .scroll((offset_entries as u16, 0));
    frame.render_widget(p, inner);
}

/// The preview pane: effective values with `(default)` markers (Spec §3).
/// Row 0 (scratch) has no template — a hint takes the preview's place.
fn render_preview(frame: &mut Frame, area: Rect, templates: &[Template], selected: usize) {
    let t = &THEME;
    let name = if selected == 0 {
        SCRATCH_ROW_NAME
    } else {
        templates
            .get(selected - 1)
            .map(|tpl| tpl.meta.name.as_str())
            .unwrap_or("—")
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            format!(" Preview: {name} "),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(t.accent))
        .style(Style::default().bg(t.panel));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if selected == 0 {
        let p = Paragraph::new(vec![
            Line::from(Span::styled(
                "  Empty form with the full image picker — choose any image",
                Style::default().fg(t.muted),
            )),
            Line::from(Span::styled(
                "  from your cache, type a custom reference (e.g.",
                Style::default().fg(t.muted),
            )),
            Line::from(Span::styled(
                "  python:3.12), and set name, ports and volumes by hand.",
                Style::default().fg(t.muted),
            )),
            Line::raw(""),
            Line::from(Span::styled(
                "  Enter opens the form.",
                Style::default().fg(t.muted),
            )),
        ])
        .block(Block::default().style(Style::default().bg(t.panel)));
        frame.render_widget(p, inner);
        return;
    }
    let Some(tpl) = templates.get(selected - 1) else {
        return;
    };
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut lines = Vec::new();
    // The template's own description (Hub text) — the list carries names
    // only, so this is the one place it appears.
    lines.push(Line::from(Span::styled(
        format!("  {}", tpl.meta.description),
        Style::default().fg(t.muted),
    )));
    lines.push(Line::raw(""));
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
        "  Enter creates the sandbox directly.",
        Style::default().fg(t.muted),
    )));
    let p = Paragraph::new(lines).block(Block::default().style(Style::default().bg(t.panel)));
    frame.render_widget(p, inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn user_tpl(id: &str, name: &str, description: &str) -> Template {
        crate::template::parse(
            id,
            &format!("[meta]\nname = \"{name}\"\ndescription = \"{description}\"\n\n[spec]\nimage = \"alpine\"\n"),
        )
        .unwrap()
    }

    #[test]
    fn picker_scrolls_selected_into_view_offscreen() {
        // I4: with more templates than fit, the highlighted row must be
        // visible (scrolling list, truncating lines — no wrap).
        let mut templates = crate::template::load_builtins();
        templates.push(user_tpl("t-z4", "Vierter", "vier"));
        templates.push(user_tpl("t-z5", "Fuenfter", "fuenfte Beschreibung"));
        let selected = templates.len(); // last row: scratch (0) shifts + 1

        let mut terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();
        terminal
            .draw(|f| render(f, f.area(), &templates, selected))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let text: String = (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
                    .collect::<String>()
            })
            .collect::<String>();
        assert!(
            text.contains("Fuenfter"),
            "selected template must be scrolled into view at 80x16:\n{text}"
        );
    }

    #[test]
    fn list_shows_names_only_details_live_in_the_preview() {
        // Phase 2.13 review: description lines (grayed) duplicated the
        // preview pane's job in the list. The list carries names only;
        // the selected entry's description appears once — in the preview
        // pane — and other entries' descriptions are not rendered at all.
        let templates = crate::template::load_builtins();
        let pi = templates.iter().position(|t| t.id == "pi").unwrap() + 1;
        let devin = templates.iter().position(|t| t.id == "devin").unwrap() + 1;

        let mut terminal = Terminal::new(TestBackend::new(120, 34)).unwrap();
        terminal
            .draw(|f| render(f, f.area(), &templates, pi))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let text: String = (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
                    .collect::<String>()
            })
            .collect::<String>();

        assert!(
            text.contains("Pi coding agent"),
            "the selected template's description shows in the preview pane:\n{text}"
        );
        assert!(
            !text.contains("Devin CLI"),
            "unselected entries' descriptions must not render in the list:\n{text}"
        );
        assert!(
            !text.contains("Empty form — image picker"),
            "the scratch pseudo-row carries no list description:\n{text}"
        );
        let _ = devin;
    }

    #[test]
    fn picker_truncates_long_description_in_preview_offscreen() {
        // Review Focus 5: long text truncates (no wrap, no panic) — now
        // exercised where descriptions live, the preview pane.
        let templates = vec![user_tpl(
            "x",
            "Lang",
            &"sehr lange Beschreibung ".repeat(20),
        )];
        let mut terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();
        terminal
            .draw(|f| render(f, f.area(), &templates, 1))
            .unwrap();
    }
}
