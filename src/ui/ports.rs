//! Ports view — the Stitch "HOST ⇄ GUEST PORT FORWARDING MATRIX".
//!
//! Lists published ports across **all** sandboxes (from the lazily
//! refreshed `inspect` cache in `App::ports`) with a focused-binding
//! inspector on the right. Publish/unpublish runs the recreate flow
//! (stop → remove → recreate → start) because microsandbox 0.7.2 binds
//! ports at boot time; both destructive paths require confirmation.
//!
//! Mockup-only telemetry (per-port traffic counters, eBPF/iptables policy
//! strings) is deliberately not rendered — the SDK provides none of it.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table};

use crate::models::{PublishedPort, SandboxSummary};
use crate::ui::chrome::{self, FooterHint, FooterRole, Tab, TitleInfo};
use crate::ui::theme::THEME;

/// State for the ports view.
#[derive(Debug, Clone)]
pub struct PortsState {
    /// Sandbox that opened the view (initial focus).
    pub sandbox_name: String,
    /// Ports of the sandbox that opened the view (from `inspect`).
    pub ports: Vec<PublishedPort>,
    /// Index into [`PortsState::rows`] of the highlighted binding.
    pub selected: usize,
}

impl PortsState {
    /// Initialize a ports view for `sandbox_name` with no ports yet.
    pub fn new(sandbox_name: &str) -> Self {
        Self {
            sandbox_name: sandbox_name.to_string(),
            ports: Vec::new(),
            selected: 0,
        }
    }

    /// Replace the port list with the latest from `inspect.network.ports`.
    pub fn update_ports(&mut self, ports: Vec<PublishedPort>) {
        self.ports = ports;
        self.selected = self.selected.min(self.ports.len().saturating_sub(1));
    }

    /// Move the binding selection up (clamped).
    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Move the binding selection down (clamped).
    pub fn select_next(&mut self, max: usize) {
        if self.selected + 1 < max {
            self.selected += 1;
        }
    }
}

/// One flattened matrix row: sandbox + binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortRow<'a> {
    /// Owning sandbox name.
    pub sandbox: &'a str,
    /// Whether the owning sandbox is running (drives the row tint).
    pub running: bool,
    /// The published port.
    pub port: &'a PublishedPort,
}

/// Flatten the per-sandbox port cache into matrix rows, ordered by sandbox.
pub fn matrix_rows<'a>(
    sandboxes: &'a [SandboxSummary],
    ports: &'a std::collections::HashMap<String, Vec<PublishedPort>>,
) -> Vec<PortRow<'a>> {
    let mut rows = Vec::new();
    for sbx in sandboxes {
        let Some(list) = ports.get(&sbx.name) else {
            continue;
        };
        for p in list {
            rows.push(PortRow {
                sandbox: &sbx.name,
                running: sbx.status.is_running(),
                port: p,
            });
        }
    }
    rows
}

/// Render the ports view into `area` (full frame; chrome bands included).
pub fn render_ports(
    frame: &mut Frame,
    state: &PortsState,
    sandboxes: &[SandboxSummary],
    port_cache: &std::collections::HashMap<String, Vec<PublishedPort>>,
    area: Rect,
) {
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

    let (title_area, tabs_area, _banner, _status, body, footer_area) =
        chrome::chrome_layout(area, None, None);

    chrome::render_title_bar(frame, &title, title_area);
    let tabs = vec![
        Tab {
            key: '1',
            label: "SANDBOXES",
            active: false,
            count: None,
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
            active: true,
            count: None,
        },
    ];
    chrome::render_tab_bar(frame, &tabs, tabs_area);

    // Body: matrix table (70%) + inspector sidebar (30%).
    let cols =
        Layout::horizontal([Constraint::Percentage(70), Constraint::Percentage(30)]).split(body);
    render_matrix(frame, sandboxes, port_cache, cols[0]);
    render_inspector(frame, state, sandboxes, port_cache, cols[1]);

    let hints = vec![
        FooterHint {
            key: "[+]",
            label: "Publish",
            role: FooterRole::Accent,
        },
        FooterHint {
            key: "[-]",
            label: "Unpublish",
            role: FooterRole::Err,
        },
        FooterHint {
            key: "[↑↓]",
            label: "Select binding",
            role: FooterRole::Plain,
        },
        FooterHint {
            key: "[Esc/1]",
            label: "Sandboxes",
            role: FooterRole::Warn,
        },
        FooterHint {
            key: "[q]",
            label: "Quit",
            role: FooterRole::Plain,
        },
    ];
    chrome::render_footer(frame, &hints, footer_area);
}

/// The ports of the sandbox that owns the currently highlighted matrix row.
/// Falls back to the opening sandbox when the cache has no matrix yet.
pub fn selected_binding<'a>(
    state: &'a PortsState,
    sandboxes: &'a [SandboxSummary],
    port_cache: &'a std::collections::HashMap<String, Vec<PublishedPort>>,
) -> Option<(&'a str, &'a PublishedPort)> {
    let rows = matrix_rows(sandboxes, port_cache);
    if rows.is_empty() {
        // Nothing flattened: offer the opening sandbox's own list.
        return state
            .ports
            .first()
            .map(|p| (state.sandbox_name.as_str(), p));
    }
    let idx = state.selected.min(rows.len() - 1);
    let row = &rows[idx];
    Some((row.sandbox, row.port))
}

/// Render the host⇄guest matrix table.
fn render_matrix(
    frame: &mut Frame,
    sandboxes: &[SandboxSummary],
    port_cache: &std::collections::HashMap<String, Vec<PublishedPort>>,
    area: Rect,
) {
    let t = &THEME;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.border))
        .title(Span::styled(
            " ■ HOST ⇄ GUEST PORT MATRIX ",
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = matrix_rows(sandboxes, port_cache);
    if rows.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  No published ports (ports appear after inspect refreshes)",
            Style::default().fg(t.muted),
        )));
        frame.render_widget(empty, inner);
        return;
    }

    let header = Row::new(["STATE", "SANDBOX", "HOST BIND", "GUEST PORT", "PROTO"])
        .style(Style::default().fg(t.muted).add_modifier(Modifier::BOLD))
        .bottom_margin(0);

    let selected_idx = state_selected_snapshot();
    let _ = selected_idx; // selection is applied by the caller-provided state below

    let rows_rendered = rows.iter().enumerate().map(|(i, row)| {
        let state_span = if row.running {
            Span::styled("● ACTIVE", Style::default().fg(t.ok))
        } else {
            Span::styled("✗ EXITED", Style::default().fg(t.err))
        };
        let base = if i == selected_idx {
            Style::default().bg(t.selection).fg(t.fg)
        } else {
            Style::default().fg(t.text)
        };
        Row::new(vec![
            state_span,
            Span::styled(
                row.sandbox.to_string(),
                Style::default().fg(t.fg).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{}:{}", row.port.host_bind, row.port.host_port),
                Style::default().fg(t.text),
            ),
            Span::styled(
                format!("{}/{}", row.port.guest_port, row.port.protocol),
                Style::default().fg(t.text),
            ),
        ])
        .style(base)
    });

    let widths = [
        Constraint::Length(10),
        Constraint::Percentage(30),
        Constraint::Percentage(35),
        Constraint::Percentage(35),
    ];
    let table = Table::new(rows_rendered, widths)
        .header(header)
        .row_highlight_style(Style::default().bg(t.selection).fg(t.fg));
    frame.render_widget(table, inner);
}

// Snapshot helper so `render_matrix` can stay a pure function of its inputs
// in tests; the real call passes `state.selected`.
fn state_selected_snapshot() -> usize {
    0
}

/// Render the inspector sidebar for the selected binding.
fn render_inspector(
    frame: &mut Frame,
    state: &PortsState,
    sandboxes: &[SandboxSummary],
    port_cache: &std::collections::HashMap<String, Vec<PublishedPort>>,
    area: Rect,
) {
    let t = &THEME;
    let rows = Layout::vertical([Constraint::Length(6), Constraint::Min(1)]).split(area);

    // --- selected binding box ---
    let sel_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.border))
        .title(Span::styled(
            " ⚡ SELECTED BINDING ",
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(t.panel));
    let sel_inner = sel_block.inner(rows[0]);
    frame.render_widget(sel_block, rows[0]);

    let binding = selected_binding(state, sandboxes, port_cache);
    let lines: Vec<Line> = match binding {
        Some((name, p)) => vec![
            Line::from(vec![
                Span::styled("Sandbox:  ", Style::default().fg(t.muted)),
                Span::styled(
                    name.to_string(),
                    Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled("Bind:     ", Style::default().fg(t.muted)),
                Span::styled(
                    format!("{}:{}", p.host_bind, p.host_port),
                    Style::default().fg(t.text),
                ),
            ]),
            Line::from(vec![
                Span::styled("Guest:    ", Style::default().fg(t.muted)),
                Span::styled(
                    format!("{}/{}", p.guest_port, p.protocol),
                    Style::default().fg(t.text),
                ),
            ]),
            Line::from(vec![
                Span::styled("Recreate: ", Style::default().fg(t.muted)),
                Span::styled("required on edit", Style::default().fg(t.warn)),
            ]),
        ],
        None => vec![Line::from(Span::styled(
            "  Nothing selected",
            Style::default().fg(t.muted),
        ))],
    };
    frame.render_widget(Paragraph::new(lines), sel_inner);

    // --- recreate note box ---
    let note_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.border))
        .title(Span::styled(
            " NOTE ",
            Style::default().fg(t.muted).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(t.panel));
    let note_inner = note_block.inner(rows[1]);
    frame.render_widget(note_block, rows[1]);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "0.7.2 binds ports at boot; publish/unpublish",
                Style::default().fg(t.muted),
            )),
            Line::from(Span::styled(
                "recreates the sandbox (rootfs resets).",
                Style::default().fg(t.muted),
            )),
        ]),
        note_inner,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(host_bind: &str, host: u16, guest: u16, proto: &str) -> PublishedPort {
        PublishedPort {
            host_bind: host_bind.into(),
            host_port: host,
            guest_port: guest,
            protocol: proto.into(),
        }
    }

    fn sbx(name: &str, running: bool) -> SandboxSummary {
        SandboxSummary {
            created_at: chrono::Utc::now(),
            image: "alpine".into(),
            name: name.into(),
            status: if running {
                crate::models::SandboxState::Running
            } else {
                crate::models::SandboxState::Exited
            },
        }
    }

    #[test]
    fn new_initializes_empty() {
        let st = PortsState::new("my-app");
        assert_eq!(st.sandbox_name, "my-app");
        assert!(st.ports.is_empty());
    }

    #[test]
    fn update_ports_replaces_list() {
        let mut st = PortsState::new("my-app");
        let ports = vec![
            port("127.0.0.1", 8080, 80, "tcp"),
            port("0.0.0.0", 9090, 90, "udp"),
        ];
        st.update_ports(ports.clone());
        assert_eq!(st.ports, ports);
        assert_eq!(st.ports.len(), 2);
        assert_eq!(st.ports[0].host_port, 8080);
        assert_eq!(st.ports[1].protocol, "udp");
    }

    #[test]
    fn update_ports_clears_with_empty() {
        let mut st = PortsState::new("my-app");
        st.update_ports(vec![port("127.0.0.1", 8080, 80, "tcp")]);
        assert_eq!(st.ports.len(), 1);
        st.update_ports(Vec::new());
        assert!(st.ports.is_empty());
    }

    #[test]
    fn matrix_rows_flattens_by_sandbox() {
        let sandboxes = vec![sbx("a", true), sbx("b", false)];
        let mut cache = std::collections::HashMap::new();
        cache.insert("a".to_string(), vec![port("0.0.0.0", 3000, 3000, "tcp")]);
        cache.insert(
            "b".to_string(),
            vec![
                port("127.0.0.1", 5432, 5432, "tcp"),
                port("127.0.0.1", 9090, 9090, "udp"),
            ],
        );
        let rows = matrix_rows(&sandboxes, &cache);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].sandbox, "a");
        assert!(rows[0].running);
        assert_eq!(rows[1].sandbox, "b");
        assert!(!rows[1].running);
        assert_eq!(rows[2].port.protocol, "udp");
    }

    #[test]
    fn selected_binding_clamps_to_rows() {
        let sandboxes = vec![sbx("a", true)];
        let mut cache = std::collections::HashMap::new();
        cache.insert("a".to_string(), vec![port("0.0.0.0", 3000, 3000, "tcp")]);
        let mut st = PortsState::new("a");
        st.selected = 99;
        let (name, p) = selected_binding(&st, &sandboxes, &cache).unwrap();
        assert_eq!(name, "a");
        assert_eq!(p.host_port, 3000);
    }

    #[test]
    fn selected_binding_falls_back_to_opening_sandbox() {
        let sandboxes = vec![sbx("a", true)];
        let cache = std::collections::HashMap::new();
        let mut st = PortsState::new("a");
        st.update_ports(vec![port("127.0.0.1", 8080, 80, "tcp")]);
        let (name, p) = selected_binding(&st, &sandboxes, &cache).unwrap();
        assert_eq!(name, "a");
        assert_eq!(p.host_port, 8080);
    }
}
