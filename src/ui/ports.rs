//! Port Forwards view (read-only in Phase 1).
//!
//! Lists the published ports for a sandbox (from `inspect` →
//! `network.ports`). Publish/unpublish requires the recreate flow
//! (stop → remove → recreate → start) because microsandbox 0.7.2 binds ports
//! at boot time. The view shows a note explaining this.

#![allow(dead_code)]

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table};

use crate::models::PublishedPort;
use crate::ui::theme::THEME;

/// State for the ports view.
#[derive(Debug, Clone)]
pub struct PortsState {
    /// Sandbox whose ports are being shown.
    pub sandbox_name: String,
    /// Current published ports (from `inspect.network.ports`).
    pub ports: Vec<PublishedPort>,
}

impl PortsState {
    /// Initialize a ports view for `sandbox_name` with no ports yet.
    pub fn new(sandbox_name: &str) -> Self {
        Self {
            sandbox_name: sandbox_name.to_string(),
            ports: Vec::new(),
        }
    }

    /// Replace the port list with the latest from `inspect.network.ports`.
    pub fn update_ports(&mut self, ports: Vec<PublishedPort>) {
        self.ports = ports;
    }
}

/// Render the ports view into `area`.
pub fn render_ports(frame: &mut Frame, state: &PortsState, area: Rect) {
    let t = &THEME;
    let chunks = Layout::vertical([
        Constraint::Min(3),    // table / empty state
        Constraint::Length(2), // footer hints + note
    ])
    .split(area);

    let title = format!(" Ports: {} ", state.sandbox_name);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            title,
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(t.muted));

    let inner = block.inner(chunks[0]);
    frame.render_widget(block, chunks[0]);

    if state.ports.is_empty() {
        let empty = Paragraph::new(Line::from(vec![
            Span::raw("  "),
            Span::styled("No published ports", Style::default().fg(t.warn)),
        ]))
        .style(Style::default().fg(t.muted));
        frame.render_widget(empty, inner);
    } else {
        let header = Row::new(["HOST BIND", "HOST PORT", "GUEST PORT", "PROTOCOL"])
            .style(Style::default().fg(t.fg).add_modifier(Modifier::BOLD))
            .bottom_margin(0);

        let rows = state.ports.iter().map(|p| {
            Row::new([
                p.host_bind.clone(),
                p.host_port.to_string(),
                p.guest_port.to_string(),
                p.protocol.clone(),
            ])
            .style(Style::default().fg(t.text))
        });

        let widths = [
            Constraint::Percentage(30),
            Constraint::Percentage(20),
            Constraint::Percentage(20),
            Constraint::Percentage(30),
        ];

        let table = Table::new(rows, widths).header(header);
        frame.render_widget(table, inner);
    }

    // Footer: keybinding hints + recreate note.
    let footer = Paragraph::new(vec![
        Line::from(Span::styled(
            "[+] Publish  [-] Unpublish  [Esc] back",
            Style::default().fg(t.text),
        )),
        Line::from(Span::styled(
            "Publish/unpublish requires recreate in msb 0.7.2",
            Style::default().fg(t.muted).add_modifier(Modifier::DIM),
        )),
    ]);
    frame.render_widget(footer, chunks[1]);
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
    fn update_ports_replaces_not_appends() {
        let mut st = PortsState::new("my-app");
        st.update_ports(vec![port("127.0.0.1", 8080, 80, "tcp")]);
        st.update_ports(vec![port("0.0.0.0", 9090, 90, "udp")]);
        assert_eq!(st.ports.len(), 1);
        assert_eq!(st.ports[0].host_port, 9090);
    }
}
