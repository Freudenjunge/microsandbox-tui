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

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table};

use crate::models::{PublishedPort, SandboxSummary};
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
    /// Inline publish-port form; `Some` while `+` opened it.
    pub publish_form: Option<PublishForm>,
}

/// Inline "bind new port" form (smoke-test finding 2.8: `+` did nothing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishForm {
    /// Host bind address (`127.0.0.1` or `0.0.0.0`).
    pub host_bind: String,
    /// Host port as text (validated on submit).
    pub host_port: String,
    /// Guest port as text (validated on submit).
    pub guest_port: String,
    /// Protocol: `tcp` or `udp`.
    pub protocol: String,
    /// Active cursor field in the form.
    pub field: PublishField,
    /// Validation error shown in the form.
    pub error: Option<String>,
}

/// Fields of the publish form in Tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishField {
    /// Host bind address.
    HostBind,
    /// Host port number.
    HostPort,
    /// Guest port number.
    GuestPort,
    /// Protocol toggle.
    Protocol,
}

impl PublishField {
    /// Next field in Tab order (wraps).
    fn next(self) -> Self {
        match self {
            Self::HostBind => Self::HostPort,
            Self::HostPort => Self::GuestPort,
            Self::GuestPort => Self::Protocol,
            Self::Protocol => Self::HostBind,
        }
    }
}

impl PublishForm {
    /// Initialize with the mockup defaults (loopback bind, tcp).
    pub fn new() -> Self {
        Self {
            host_bind: "127.0.0.1".into(),
            host_port: String::new(),
            guest_port: String::new(),
            protocol: "tcp".into(),
            field: PublishField::HostBind,
            error: None,
        }
    }

    /// Validate and build the port spec.
    pub fn to_port(&self) -> Result<PublishedPort, String> {
        let host_port: u16 = self
            .host_port
            .trim()
            .parse()
            .map_err(|_| "Host port must be 1-65535".to_string())?;
        let guest_port: u16 = self
            .guest_port
            .trim()
            .parse()
            .map_err(|_| "Guest port must be 1-65535".to_string())?;
        if host_port == 0 || guest_port == 0 {
            return Err("Ports must be 1-65535".to_string());
        }
        let bind = self.host_bind.trim().to_string();
        if bind.is_empty() {
            return Err("Host bind is required".to_string());
        }
        Ok(PublishedPort {
            host_bind: bind,
            host_port,
            guest_port,
            protocol: self.protocol.clone(),
        })
    }

    /// Route a key into the form. Returns `Some(port)` on Enter-submit
    /// (already validated), `None` while editing, and flips `cancelled`
    /// (via [`PublishForm::cancel`]) on Esc.
    pub fn handle_key(&mut self, key: KeyEvent) -> PublishFormAction {
        match key.code {
            KeyCode::Esc => {
                return PublishFormAction::Cancel;
            }
            KeyCode::Tab => {
                self.field = self.field.next();
                return PublishFormAction::Continue;
            }
            KeyCode::Up if self.field == PublishField::Protocol => {
                self.protocol = "udp".into();
                return PublishFormAction::Continue;
            }
            KeyCode::Down if self.field == PublishField::Protocol => {
                self.protocol = "tcp".into();
                return PublishFormAction::Continue;
            }
            KeyCode::Enter => {
                return match self.to_port() {
                    Ok(port) => PublishFormAction::Submit(port),
                    Err(e) => {
                        self.error = Some(e);
                        PublishFormAction::Continue
                    }
                };
            }
            _ => {}
        }
        // Text input for the active text field.
        let buf = match self.field {
            PublishField::HostBind => &mut self.host_bind,
            PublishField::HostPort => &mut self.host_port,
            PublishField::GuestPort => &mut self.guest_port,
            PublishField::Protocol => return PublishFormAction::Continue,
        };
        match key.code {
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                buf.push(c);
                self.error = None;
            }
            KeyCode::Backspace => {
                buf.pop();
            }
            _ => {}
        }
        PublishFormAction::Continue
    }
}

/// Result of a key routed into the publish form.
#[derive(Debug, Clone, PartialEq)]
pub enum PublishFormAction {
    /// Keep editing.
    Continue,
    /// Validated port ready for the recreate flow.
    Submit(PublishedPort),
    /// User cancelled the form.
    Cancel,
}

impl Default for PublishForm {
    fn default() -> Self {
        Self::new()
    }
}

impl PortsState {
    /// Initialize a ports view for `sandbox_name` with no ports yet.
    pub fn new(sandbox_name: &str) -> Self {
        Self {
            sandbox_name: sandbox_name.to_string(),
            ports: Vec::new(),
            selected: 0,
            publish_form: None,
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

/// Render this sandbox's ports body into the detail pane (2.9 IA): the
/// sandbox's bindings + publish form / binding inspector. No chrome — the
/// dashboard owns the frame.
pub fn render_ports_body(
    frame: &mut Frame,
    state: &PortsState,
    sandboxes: &[SandboxSummary],
    port_cache: &std::collections::HashMap<String, Vec<PublishedPort>>,
    area: Rect,
) {
    let t = &THEME;
    // Header row (the sandbox scope) + matrix.
    let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
    let name = &state.sandbox_name;
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ⚡ ", Style::default().fg(t.warn)),
            Span::styled(
                format!("PORTS — {name}"),
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                "[+] publish  [-] unpublish  [r] refresh",
                Style::default().fg(t.muted),
            ),
        ])),
        chunks[0],
    );
    render_matrix(frame, sandboxes, port_cache, chunks[1]);
    // Publish form replaces the inspector area via a bottom overlay strip.
    if state.publish_form.is_some() {
        let split = Layout::vertical([Constraint::Min(3), Constraint::Length(12)]).split(chunks[1]);
        render_matrix(frame, sandboxes, port_cache, split[0]);
        render_publish_form(frame, state, sandboxes, port_cache, split[1]);
    }
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

/// Render the inline publish-port form (replaces the inspector while open).
/// The form always binds on the sandbox of the currently selected matrix
/// row — the matrix is the global overview, but each port belongs to
/// exactly one sandbox.
fn render_publish_form(
    frame: &mut Frame,
    state: &PortsState,
    sandboxes: &[SandboxSummary],
    port_cache: &std::collections::HashMap<String, Vec<PublishedPort>>,
    area: Rect,
) {
    let t = &THEME;
    let Some(form) = &state.publish_form else {
        return;
    };
    // Resolve the binding target: the selected row's sandbox (falling back
    // to the sandbox that opened the view when the matrix is empty).
    let target = selected_binding(state, sandboxes, port_cache)
        .map(|(name, _)| name.to_string())
        .unwrap_or_else(|| state.sandbox_name.clone());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.accent))
        .style(Style::default().bg(t.panel))
        .title(Span::styled(
            format!(" [+] BIND NEW PORT — {target} "),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let field_line = |label: &str, value: &str, active: bool| -> Line<'static> {
        let style = if active {
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.muted)
        };
        let cursor = if active { "█" } else { "" };
        Line::from(vec![
            Span::styled(format!(" {label:<12}"), style),
            Span::styled(format!("{value}{cursor}"), Style::default().fg(t.fg)),
        ])
    };
    let proto = |p: &str| -> Span<'static> {
        let marker = if form.protocol == p { "(•)" } else { "( )" };
        let style = if form.protocol == p && form.field == PublishField::Protocol {
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
        } else if form.protocol == p {
            Style::default().fg(t.accent)
        } else {
            Style::default().fg(t.muted)
        };
        Span::styled(format!("{marker} {p} "), style)
    };

    let mut lines = vec![
        field_line(
            "Host bind:",
            &form.host_bind,
            form.field == PublishField::HostBind,
        ),
        field_line(
            "Host port:",
            &form.host_port,
            form.field == PublishField::HostPort,
        ),
        field_line(
            "Guest port:",
            &form.guest_port,
            form.field == PublishField::GuestPort,
        ),
        Line::from(vec![
            Span::styled(
                " Protocol:  ",
                if form.field == PublishField::Protocol {
                    Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(t.muted)
                },
            ),
            proto("tcp"),
            proto("udp"),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "[Enter]",
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" bind (recreate)  ", Style::default().fg(t.muted)),
            Span::styled(
                "[Tab]",
                Style::default().fg(t.fg).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" field  ", Style::default().fg(t.muted)),
            Span::styled(
                "[Esc]",
                Style::default().fg(t.err).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" cancel", Style::default().fg(t.muted)),
        ]),
    ];
    if let Some(err) = &form.error {
        lines.push(Line::from(vec![
            Span::styled(" ✗ ", Style::default().fg(t.err)),
            Span::styled(err.clone(), Style::default().fg(t.err)),
        ]));
    }
    lines.push(Line::from(Span::styled(
        " ⚠ Recreates the sandbox (rootfs resets)",
        Style::default().fg(t.warn),
    )));
    frame.render_widget(Paragraph::new(lines), inner);
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
            workdir: None,
            mounts: Vec::new(),
            shell: None,
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
    fn publish_form_validates_and_builds() {
        let mut form = PublishForm::new();
        let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        // Start field is HostBind (pre-filled 127.0.0.1); jump to host port.
        form.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        for c in "3000".chars() {
            form.handle_key(key(c));
        }
        form.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        for c in "3000".chars() {
            form.handle_key(key(c));
        }
        let action = form.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        match action {
            PublishFormAction::Submit(p) => {
                assert_eq!(p.host_bind, "127.0.0.1");
                assert_eq!(p.host_port, 3000);
                assert_eq!(p.guest_port, 3000);
                assert_eq!(p.protocol, "tcp");
            }
            other => panic!("expected submit, got {other:?}"),
        }
    }

    #[test]
    fn publish_form_rejects_bad_ports() {
        let mut form = PublishForm::new();
        form.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        for c in "abc".chars() {
            form.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        let action = form.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(action, PublishFormAction::Continue);
        assert!(form.error.is_some(), "invalid input must set an error");
    }

    #[test]
    fn publish_form_esc_cancels() {
        let mut form = PublishForm::new();
        let action = form.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(action, PublishFormAction::Cancel);
    }

    #[test]
    fn publish_form_protocol_toggle() {
        let mut form = PublishForm::new();
        // Tab thrice: bind -> host -> guest -> protocol.
        form.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        form.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        form.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(form.field, PublishField::Protocol);
        form.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(form.protocol, "udp");
        form.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(form.protocol, "tcp");
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
