//! Dashboard view: fleet stats bar, responsive sandbox card grid, and the
//! status/event panel — framed by the shared chrome (title/tab/banner/footer).
//!
//! Layout after the Stitch mockup
//! (`design/stitch_microsandbox_tui_design_system/microsandbox_tui_terminal_dashboard_120x34_desktop_grid/`):
//! fleet stats bar, 2–3-column card grid with status pill, image chip, CPU
//! gauge, memory, net I/O, ports, uptime; below it the event panel. Mockup
//! telemetry the SDK does not provide (per-port traffic, security policies)
//! is deliberately not rendered.
//!
//! All formatting helpers are pure and unit-tested.

use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::models::{Metrics, PublishedPort, SandboxState, SandboxSummary};
use crate::ui::chrome::{self, FooterHint, FooterRole, Tab, TitleInfo};
use crate::ui::theme::THEME;

/// Vertical separator column drawn between two side-by-side zones inside a
/// common surface (the mockup's `divide-x` line).
fn draw_vertical_separator(frame: &mut Frame, x: u16, row: Rect) {
    let t = &THEME;
    for y in row.y..row.y.saturating_add(row.height) {
        let cell = Rect::new(x, y, 1, 1);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled("│", Style::default().fg(t.border)))),
            cell,
        );
    }
}

/// Height in rows of a single card (including its border).
const CARD_HEIGHT: u16 = 8;

/// Render the dashboard into `area` (the full frame; chrome bands included).
#[allow(clippy::too_many_arguments)]
pub fn render(
    frame: &mut Frame,
    sandboxes: &[SandboxSummary],
    metrics: &HashMap<String, Metrics>,
    ports: &HashMap<String, Vec<PublishedPort>>,
    selected: usize,
    status: StatusLines<'_>,
    quick: Option<&crate::ui::create::CreateForm>,
    preview: &[crate::backend::LogLine],
    area: Rect,
) {
    let t = &THEME;
    let runtime_version = match crate::runtime::detect() {
        crate::runtime::RuntimeStatus::Installed { runtime, .. } => {
            runtime.version.as_ref().map(|v| v.to_string())
        }
        crate::runtime::RuntimeStatus::Missing => None,
    };
    let title = TitleInfo {
        runtime_version,
        size: format!("{}x{}", area.width, area.height),
        pid: std::process::id(),
        live: true,
    };

    let (title_area, tabs_area, banner_area, status_area, body, footer_area) =
        chrome::chrome_layout(area, status.banner, status.error.or(status.status_text));

    chrome::render_title_bar(frame, &title, title_area);

    let running = sandboxes.iter().filter(|s| s.status.is_running()).count();
    let stopped = sandboxes.len() - running;
    let tabs = vec![
        Tab {
            key: '1',
            label: "SANDBOXES",
            active: true,
            count: Some(sandboxes.len()),
        },
        Tab {
            key: '2',
            label: "LOGS",
            active: false,
            count: None,
        },
        Tab {
            key: '3',
            label: "PORTS",
            active: false,
            count: None,
        },
    ];
    chrome::render_tab_bar(frame, &tabs, tabs_area);
    let _ = (running, stopped);

    if let Some(banner) = status.banner {
        chrome::render_banner(frame, banner, banner_area.expect("banner area"));
    }
    let status_msg = status.error.or(status.status_text);
    if let Some(msg) = status_msg {
        if status.error.is_some() {
            chrome::render_error_line(frame, msg, status_area.expect("status area"));
        } else {
            chrome::render_status_line(frame, msg, status_area.expect("status area"));
        }
    }

    // Two-pane body on wide terminals (cards 65% / sidebar 35%) like the
    // mockup: ONE surface, split by a vertical separator line — not two
    // separate boxes. The sidebar hides while the full create form is open
    // (the form covers the frame) and on narrow terminals.
    if body.width >= 100 && quick.is_none() {
        // Draw the shared surface border ONCE, then split the interior.
        let surface_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t.border));
        let inner = surface_block.inner(body);
        frame.render_widget(surface_block, body);

        let cols = Layout::horizontal([
            Constraint::Percentage(65),
            Constraint::Length(1),
            Constraint::Percentage(35),
        ])
        .split(inner);
        let cards = cols[0];
        let divider_x = cols[1].x;
        let sidebar = cols[2];
        draw_vertical_separator(frame, divider_x, inner);
        render_body(frame, sandboxes, metrics, ports, selected, cards);
        render_sidebar(frame, sandboxes, selected, preview, sidebar);
    } else {
        render_body(frame, sandboxes, metrics, ports, selected, body);
    }

    let hints = vec![
        FooterHint {
            key: "[c]",
            label: "Create",
            role: FooterRole::Accent,
        },
        FooterHint {
            key: "[s]",
            label: "Start",
            role: FooterRole::Plain,
        },
        FooterHint {
            key: "[x]",
            label: "Stop",
            role: FooterRole::Plain,
        },
        FooterHint {
            key: "[r]",
            label: "Restart",
            role: FooterRole::Warn,
        },
        FooterHint {
            key: "[l]",
            label: "Logs",
            role: FooterRole::Plain,
        },
        FooterHint {
            key: "[p]",
            label: "Ports",
            role: FooterRole::Plain,
        },
        FooterHint {
            key: "[e]",
            label: "Exec",
            role: FooterRole::Plain,
        },
        FooterHint {
            key: "[Del]",
            label: "Destroy",
            role: FooterRole::Err,
        },
        FooterHint {
            key: "[?]",
            label: "Help",
            role: FooterRole::Accent,
        },
        FooterHint {
            key: "[q]",
            label: "Quit",
            role: FooterRole::Plain,
        },
    ];
    chrome::render_footer(frame, &hints, footer_area);
    let _ = t;
}

/// Status/message lines drawn around the card grid.
#[derive(Debug, Default, Clone, Copy)]
pub struct StatusLines<'a> {
    /// Runtime banner (update available / missing runtime), yellow.
    pub banner: Option<&'a str>,
    /// Last error, red.
    pub error: Option<&'a str>,
    /// Transient status (spinner text, op result).
    pub status_text: Option<&'a str>,
}

/// Render the card grid / empty state into the body area.
fn render_body(
    frame: &mut Frame,
    sandboxes: &[SandboxSummary],
    metrics: &HashMap<String, Metrics>,
    ports: &HashMap<String, Vec<PublishedPort>>,
    selected: usize,
    area: Rect,
) {
    if sandboxes.is_empty() {
        render_empty(frame, area);
    } else {
        render_grid(frame, sandboxes, metrics, ports, selected, area);
    }
}

/// Render the right sidebar as zones inside the (already bordered) surface:
/// a horizontal separator line divides the quick-create zone from the
/// stream zone below — no second border of its own.
fn render_sidebar(
    frame: &mut Frame,
    sandboxes: &[SandboxSummary],
    selected: usize,
    preview: &[crate::backend::LogLine],
    area: Rect,
) {
    if area.height == 0 {
        return;
    }
    // First `quick_h` rows: quick create; separator; rest: stream preview.
    let quick_h = area.height.min(4);
    let sep_y = area.y + quick_h;
    render_quick_create(frame, Rect::new(area.x, area.y, area.width, quick_h));
    let stream_h = area.height.saturating_sub(quick_h + 1);
    if stream_h == 0 {
        return;
    }
    let stream = Rect::new(area.x, sep_y + 1, area.width, stream_h);
    // Horizontal separator across the sidebar column.
    let t = &THEME;
    let sep = Rect::new(area.x, sep_y, area.width, 1);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(sep.width as usize),
            Style::default().fg(t.border),
        ))),
        sep,
    );
    render_stream_preview(frame, sandboxes, selected, preview, stream);
}

/// The mockup's `⚡ QUICK CREATE MICROVM` zone header + collapsed content.
/// `c`/`Ctrl+A` open the full form.
fn render_quick_create(frame: &mut Frame, area: Rect) {
    let t = &THEME;
    let mut lines = vec![Line::from(vec![
        Span::styled(" ⚡ ", Style::default().fg(t.warn)),
        Span::styled(
            "QUICK CREATE MICROVM",
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled("[Ctrl+A]", Style::default().fg(t.muted)),
        Span::styled(" Advanced", Style::default().fg(t.muted)),
    ])];
    if area.height > 1 {
        lines.push(Line::from(vec![
            Span::styled(" Image:  ", Style::default().fg(t.muted)),
            Span::styled("<pick with c>", Style::default().fg(t.text)),
        ]));
    }
    if area.height > 2 {
        lines.push(Line::from(vec![
            Span::styled(" Name:   ", Style::default().fg(t.muted)),
            Span::styled("(auto)", Style::default().fg(t.text)),
        ]));
    }
    if area.height > 3 {
        lines.push(Line::from(vec![
            Span::styled(
                "[c]",
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" open form   ", Style::default().fg(t.muted)),
            Span::styled(
                "[^a]",
                Style::default().fg(t.warn).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" advanced", Style::default().fg(t.muted)),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// The mockup's `STREAM: <sandbox>` zone: last few log lines of the
/// selected sandbox, `tail -n 6 -f` style.
fn render_stream_preview(
    frame: &mut Frame,
    sandboxes: &[SandboxSummary],
    selected: usize,
    preview: &[crate::backend::LogLine],
    area: Rect,
) {
    let t = &THEME;
    let name = sandboxes
        .get(selected)
        .map(|s| s.name.as_str())
        .unwrap_or("—");
    let mut lines = vec![Line::from(vec![
        Span::styled(" ● ", Style::default().fg(t.accent)),
        Span::styled(
            format!("STREAM: {name}"),
            Style::default().fg(t.fg).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled("tail -n 6 -f", Style::default().fg(t.muted)),
    ])];
    if preview.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (no output yet — tail running)",
            Style::default().fg(t.muted),
        )));
    } else {
        let max = (area.width as usize).saturating_sub(12);
        for l in preview.iter().take(area.height as usize) {
            let ts = l.timestamp.format("%H:%M:%S").to_string();
            let data: String = l.data.chars().take(max).collect();
            lines.push(Line::from(vec![
                Span::styled(format!("{ts} "), Style::default().fg(t.muted)),
                Span::styled(data, Style::default().fg(t.text)),
            ]));
        }
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// Centered empty-state message.
fn render_empty(frame: &mut Frame, area: Rect) {
    let t = &THEME;
    // No border of its own: the empty state lives inside the dashboard's
    // zoned surface, which already draws the frame.
    let para = Paragraph::new(Line::from(vec![
        Span::styled("No sandboxes. ", Style::default().fg(t.text)),
        Span::styled("Press ", Style::default().fg(t.muted)),
        Span::styled(
            "[c]",
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" to create one.", Style::default().fg(t.muted)),
    ]))
    .centered();
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

/// Render a single sandbox card (Stitch style: pill badge, image chip, gauge).
fn render_card(
    frame: &mut Frame,
    sbx: &SandboxSummary,
    metrics: Option<&Metrics>,
    ports: Option<&Vec<PublishedPort>>,
    selected: bool,
    area: Rect,
) {
    let t = &THEME;
    let border_style = if selected {
        Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(t.border)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .style(Style::default().bg(t.panel));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Title row: ▶/○ name [image-chip] ... RUNNING-pill (right-aligned by
    // padding to a fixed slot; card widths vary so we left-pack and accept
    // truncation on narrow terminals).
    let (symbol, state_color) = state_indicator(&sbx.status);
    let mut title = vec![
        Span::styled(format!("{symbol} "), Style::default().fg(state_color)),
        Span::styled(
            sbx.name.clone(),
            Style::default().fg(t.fg).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(sbx.image.clone(), Style::default().fg(t.muted)),
    ];
    // Right-side status pill, padded so it hugs the right edge on typical
    // card widths (>= 24 cols). Paragraph clips rather than wraps.
    let status_label = status_pill_label(&sbx.status);
    let used: usize = symbol.len() + sbx.name.chars().count() + 1 + sbx.image.chars().count();
    let pad = (usize::from(inner.width)).saturating_sub(used + status_label.len() + 1);
    title.push(Span::raw(" ".repeat(pad + 1)));
    title.push(Span::styled(
        status_label.clone(),
        Style::default()
            .fg(t.bg)
            .bg(state_color)
            .add_modifier(Modifier::BOLD),
    ));

    let mut lines = vec![Line::from(title)];

    match metrics {
        Some(m) => {
            lines.push(Line::from(vec![
                Span::styled("CPU ", Style::default().fg(t.muted)),
                Span::styled(
                    format!("{:>5.1}%", m.cpu_percent),
                    Style::default().fg(t.text),
                ),
                Span::raw(" "),
                Span::styled(cpu_gauge(m.cpu_percent), Style::default().fg(t.accent)),
            ]));
            lines.push(Line::from(vec![
                Span::styled("MEM ", Style::default().fg(t.muted)),
                Span::styled(
                    format!(
                        "{} / {} ({:.1}%)",
                        format_bytes(m.memory_bytes),
                        format_bytes(m.memory_limit_bytes),
                        mem_percent(m)
                    ),
                    Style::default().fg(t.text),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Net ", Style::default().fg(t.muted)),
                Span::styled("↓", Style::default().fg(t.ok)),
                Span::styled(
                    format!("{} ", format_bytes(m.net_rx_bytes)),
                    Style::default().fg(t.text),
                ),
                Span::styled("↑", Style::default().fg(t.accent)),
                Span::styled(format_bytes(m.net_tx_bytes), Style::default().fg(t.text)),
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
        }
    }
    lines.push(Line::from(vec![
        Span::styled("Ports ", Style::default().fg(t.muted)),
        Span::styled(ports_short(ports), Style::default().fg(t.text)),
    ]));
    let uptime = metrics
        .map(|m| format_duration(m.uptime_secs))
        .unwrap_or_else(|| "--".into());
    lines.push(Line::from(vec![
        Span::styled("Uptime ", Style::default().fg(t.muted)),
        Span::styled(uptime, Style::default().fg(t.text)),
    ]));

    frame.render_widget(Paragraph::new(lines), inner);
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

/// 10-cell CPU gauge like the mockup's `[■■■□□□□□□□]`.
pub fn cpu_gauge(percent: f64) -> String {
    let cells = 10;
    let filled = ((percent.clamp(0.0, 100.0) / 100.0) * cells as f64).round() as usize;
    let filled = filled.min(cells);
    let mut gauge = String::with_capacity(cells * 3 + 2);
    gauge.push('[');
    for i in 0..cells {
        if i < filled {
            gauge.push('■');
        } else {
            gauge.push('□');
        }
    }
    gauge.push(']');
    gauge
}

/// Memory usage percentage (0 if the limit is unknown/zero).
pub fn mem_percent(m: &Metrics) -> f64 {
    if m.memory_limit_bytes == 0 {
        0.0
    } else {
        (m.memory_bytes as f64 / m.memory_limit_bytes as f64) * 100.0
    }
}

/// Uppercase pill label for a sandbox state, e.g. `RUNNING`.
pub fn status_pill_label(state: &SandboxState) -> String {
    match state {
        SandboxState::Unknown(v) => v.to_uppercase(),
        other => other.to_string().to_uppercase(),
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

/// Short, comma-separated port list for a card, e.g. `8080→80, 9090→90/udp`.
pub fn ports_short(ports: Option<&Vec<PublishedPort>>) -> String {
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

/// Number of card columns for a given content width.
pub fn card_columns(width: u16) -> usize {
    match width {
        0..=39 => 1,
        40..=89 => 2,
        _ => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn cpu_gauge_fills_proportionally() {
        assert_eq!(cpu_gauge(0.0), "[□□□□□□□□□□]");
        assert_eq!(cpu_gauge(30.0), "[■■■□□□□□□□]");
        assert_eq!(cpu_gauge(100.0), "[■■■■■■■■■■]");
        assert_eq!(cpu_gauge(150.0), "[■■■■■■■■■■]");
        assert_eq!(cpu_gauge(-5.0), "[□□□□□□□□□□]");
    }

    #[test]
    fn mem_percent_handles_zero_limit() {
        let m = Metrics {
            cpu_percent: 0.0,
            cpus: 1,
            disk_read_bytes: 0,
            disk_write_bytes: 0,
            memory_available_bytes: 0,
            memory_bytes: 100,
            memory_host_resident_bytes: 0,
            memory_limit_bytes: 0,
            name: "x".into(),
            net_rx_bytes: 0,
            net_tx_bytes: 0,
            state: SandboxState::Running,
            timestamp: chrono::Utc::now(),
            upper_free_bytes: None,
            upper_host_allocated_bytes: None,
            upper_used_bytes: None,
            uptime_secs: 0.0,
            vcpu_time_ns: 0,
        };
        assert_eq!(mem_percent(&m), 0.0);
    }

    #[test]
    fn status_pill_label_uppercases() {
        assert_eq!(status_pill_label(&SandboxState::Running), "RUNNING");
        assert_eq!(status_pill_label(&SandboxState::Exited), "EXITED");
        assert_eq!(
            status_pill_label(&SandboxState::Unknown("weird".into())),
            "WEIRD"
        );
    }

    #[test]
    fn dashboard_renders_visible_content_offscreen() {
        // Regression: the frame rendered (cursor-hide emitted) but the
        // buffer stayed empty on some terminal sizes. Render off-screen via
        // TestBackend and assert real chrome content is in the cells.
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(120, 34)).unwrap();
        let sandboxes = vec![SandboxSummary {
            created_at: chrono::Utc::now(),
            image: "alpine".into(),
            name: "probe".into(),
            status: SandboxState::Running,
        }];
        terminal
            .draw(|f| {
                render(
                    f,
                    &sandboxes,
                    &HashMap::new(),
                    &HashMap::new(),
                    0,
                    StatusLines {
                        banner: Some("msb v0.7.1 installed, v0.7.2 available — press [U]"),
                        error: None,
                        status_text: None,
                    },
                    None,
                    &[],
                    f.area(),
                );
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
        assert!(text.contains("microsandbox"), "title missing:\n{text}");
        assert!(text.contains("SANDBOXES"), "tabs missing:\n{text}");
        assert!(text.contains("probe"), "card missing:\n{text}");
        assert!(text.contains("QUICK CREATE"), "sidebar missing:\n{text}");
        assert!(text.contains("STREAM"), "preview missing:\n{text}");
        assert!(text.contains("Navigation:"), "nav legend missing:\n{text}");
        assert!(text.contains("press [U]"), "banner missing:\n{text}");
    }
}
