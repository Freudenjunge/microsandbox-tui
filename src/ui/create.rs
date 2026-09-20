//! Create-sandbox form: interactive field entry with validation.
//!
//! [`CreateForm`] owns all form state (text inputs, list items, selected
//! network profile, active field) and the pure key→action state machine.
//! Rendering is a thin function that reads form state and draws labeled
//! rows — all logic lives in the [`CreateForm`] methods so it is unit-tested
//! without a terminal.
//!
//! NOTE(dead_code): consumers arrive in Task 10.

#![allow(dead_code)]

use anyhow::{Result, anyhow, bail};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::backend::CreateSpec;
use crate::models::PublishedPort;

// ---------------------------------------------------------------------------
// NetProfile
// ---------------------------------------------------------------------------

/// Network profile preset for new sandboxes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetProfile {
    /// Internet allowed, private networks blocked (default).
    Public,
    /// LAN/internal only.
    Private,
    /// Host machine access.
    Host,
    /// No network.
    None,
    /// User-defined rules.
    Custom,
}

impl NetProfile {
    /// All variants in display order.
    pub const ALL: [Self; 5] = [
        Self::Public,
        Self::Private,
        Self::Host,
        Self::None,
        Self::Custom,
    ];

    /// String used for the `--net` CLI flag.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Private => "private",
            Self::Host => "host",
            Self::None => "none",
            Self::Custom => "custom",
        }
    }

    /// Cycle to the next profile (wraps around).
    pub fn next(self) -> Self {
        let idx = Self::ALL.iter().position(|&p| p == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }

    /// Cycle to the previous profile (wraps around).
    pub fn prev(self) -> Self {
        let idx = Self::ALL.iter().position(|&p| p == self).unwrap_or(0);
        Self::ALL[(idx + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

// ---------------------------------------------------------------------------
// FormField
// ---------------------------------------------------------------------------

/// Which form field is currently active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormField {
    Image,
    Name,
    Cpus,
    Memory,
    Workdir,
    NetProfile,
    Ports,
    Volumes,
    EnvVars,
    NetRules,
}

impl FormField {
    /// All fields in Tab order.
    pub const ALL: [Self; 10] = [
        Self::Image,
        Self::Name,
        Self::Cpus,
        Self::Memory,
        Self::Workdir,
        Self::NetProfile,
        Self::Ports,
        Self::Volumes,
        Self::EnvVars,
        Self::NetRules,
    ];

    /// Whether this field is a list-type (items + input buffer).
    pub fn is_list(&self) -> bool {
        matches!(
            self,
            Self::Ports | Self::Volumes | Self::EnvVars | Self::NetRules
        )
    }

    /// Human-readable label.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Image => "Image",
            Self::Name => "Name",
            Self::Cpus => "CPUs",
            Self::Memory => "Memory",
            Self::Workdir => "Workdir",
            Self::NetProfile => "Profile",
            Self::Ports => "Ports",
            Self::Volumes => "Volumes",
            Self::EnvVars => "Env",
            Self::NetRules => "Rules",
        }
    }
}

// ---------------------------------------------------------------------------
// FormAction
// ---------------------------------------------------------------------------

/// What the form wants the caller to do after a key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormAction {
    /// Stay on the form (re-render).
    Continue,
    /// User pressed Enter on the last field — validate and create.
    Submit,
    /// User pressed Esc — cancel and return to dashboard.
    Cancel,
    /// Tab was pressed — moved to next field.
    NextField,
    /// Shift-Tab was pressed — moved to previous field.
    PrevField,
}

// ---------------------------------------------------------------------------
// CreateForm
// ---------------------------------------------------------------------------

/// Interactive create-sandbox form state.
///
/// All text inputs are owned `String` buffers; list fields share a single
/// `list_input` buffer and `list_selected` index that are reset whenever
/// the active field changes.
#[derive(Debug)]
pub struct CreateForm {
    /// Image reference (e.g. `alpine`, `python:3.12`).
    pub image: String,
    /// Optional sandbox name (empty = auto-generated).
    pub name: String,
    /// vCPU count as a text string.
    pub cpus: String,
    /// Memory limit string (e.g. `512M`, `1G`).
    pub memory: String,
    /// Working directory inside the sandbox.
    pub workdir: String,
    /// Selected network profile.
    pub net_profile: NetProfile,
    /// Published port specs (e.g. `8080:80`, `0.0.0.0:9090:90/udp`).
    pub ports: Vec<String>,
    /// Volume mount specs (e.g. `./src:/app`, `mydata:/data`).
    pub volumes: Vec<String>,
    /// Environment variables in `KEY=VALUE` form.
    pub env_vars: Vec<String>,
    /// Network rule strings (e.g. `allow@api.example.com`).
    pub net_rules: Vec<String>,
    /// Currently active field.
    pub active_field: FormField,
    /// Validation error to display (red line).
    pub error: Option<String>,
    /// Cached image references for autocomplete.
    pub images: Vec<String>,
    /// Input buffer for the active list field.
    list_input: String,
    /// Selected item index within the active list field.
    list_selected: usize,
}

impl CreateForm {
    /// Initialize a form with default values (public profile, 1 CPU, 512M).
    pub fn new(images: Vec<String>) -> Self {
        Self {
            image: String::new(),
            name: String::new(),
            cpus: "1".to_string(),
            memory: "512M".to_string(),
            workdir: String::new(),
            net_profile: NetProfile::Public,
            ports: Vec::new(),
            volumes: Vec::new(),
            env_vars: Vec::new(),
            net_rules: Vec::new(),
            active_field: FormField::Image,
            error: None,
            images,
            list_input: String::new(),
            list_selected: 0,
        }
    }

    /// Index of the active field in [`FormField::ALL`].
    pub fn active_field_index(&self) -> usize {
        FormField::ALL
            .iter()
            .position(|&f| f == self.active_field)
            .unwrap_or(0)
    }

    /// Total number of fields.
    pub fn field_count() -> usize {
        FormField::ALL.len()
    }

    /// Process a key and return what the caller should do.
    pub fn handle_key(&mut self, key: KeyEvent) -> FormAction {
        // Global keys.
        match key.code {
            KeyCode::Esc => return FormAction::Cancel,
            KeyCode::Tab => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.prev_field();
                    return FormAction::PrevField;
                }
                self.next_field();
                return FormAction::NextField;
            }
            KeyCode::BackTab => {
                self.prev_field();
                return FormAction::PrevField;
            }
            _ => {}
        }

        // Field-specific dispatch.
        if self.active_field.is_list() {
            self.handle_list_key(key)
        } else if self.active_field == FormField::NetProfile {
            self.handle_net_profile_key(key)
        } else {
            self.handle_text_key(key)
        }
    }

    // -- navigation --

    /// Advance to the next field (wraps around).
    fn next_field(&mut self) {
        let idx = self.active_field_index();
        let next = (idx + 1) % Self::field_count();
        self.set_active_field(FormField::ALL[next]);
    }

    /// Go back to the previous field (wraps around).
    fn prev_field(&mut self) {
        let idx = self.active_field_index();
        let prev = (idx + Self::field_count() - 1) % Self::field_count();
        self.set_active_field(FormField::ALL[prev]);
    }

    /// Switch the active field and reset list-editing state.
    fn set_active_field(&mut self, field: FormField) {
        self.active_field = field;
        self.list_input.clear();
        self.list_selected = 0;
    }

    // -- text field handling --

    /// Handle a key on a text input field.
    fn handle_text_key(&mut self, key: KeyEvent) -> FormAction {
        match key.code {
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.active_text_buf().push(c);
                FormAction::Continue
            }
            KeyCode::Backspace => {
                self.active_text_buf().pop();
                FormAction::Continue
            }
            KeyCode::Enter => {
                if self.active_field_index() + 1 >= Self::field_count() {
                    FormAction::Submit
                } else {
                    self.next_field();
                    FormAction::NextField
                }
            }
            _ => FormAction::Continue,
        }
    }

    /// Mutable reference to the string buffer of the active text field.
    fn active_text_buf(&mut self) -> &mut String {
        match self.active_field {
            FormField::Image => &mut self.image,
            FormField::Name => &mut self.name,
            FormField::Cpus => &mut self.cpus,
            FormField::Memory => &mut self.memory,
            FormField::Workdir => &mut self.workdir,
            _ => unreachable!("active_text_buf on non-text field"),
        }
    }

    // -- net profile handling --

    /// Handle a key on the network profile selector.
    fn handle_net_profile_key(&mut self, key: KeyEvent) -> FormAction {
        match key.code {
            KeyCode::Up | KeyCode::Left => {
                self.net_profile = self.net_profile.prev();
                FormAction::Continue
            }
            KeyCode::Down | KeyCode::Right => {
                self.net_profile = self.net_profile.next();
                FormAction::Continue
            }
            KeyCode::Enter => {
                if self.active_field_index() + 1 >= Self::field_count() {
                    FormAction::Submit
                } else {
                    self.next_field();
                    FormAction::NextField
                }
            }
            _ => FormAction::Continue,
        }
    }

    // -- list field handling --

    /// Handle a key on a list-type field (ports, volumes, env, rules).
    fn handle_list_key(&mut self, key: KeyEvent) -> FormAction {
        match key.code {
            KeyCode::Delete => {
                self.remove_selected_item();
                FormAction::Continue
            }
            // '+' adds an empty item when not actively typing.
            KeyCode::Char('+') if self.list_input.is_empty() => {
                self.active_list_mut().push(String::new());
                self.list_selected = self.active_list_ref().len() - 1;
                FormAction::Continue
            }
            // '-' removes the selected item when not actively typing.
            KeyCode::Char('-') if self.list_input.is_empty() => {
                self.remove_selected_item();
                FormAction::Continue
            }
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.list_input.push(c);
                FormAction::Continue
            }
            KeyCode::Backspace => {
                self.list_input.pop();
                FormAction::Continue
            }
            KeyCode::Enter => {
                if !self.list_input.is_empty() {
                    let item = std::mem::take(&mut self.list_input);
                    self.active_list_mut().push(item);
                    self.list_selected = self.active_list_ref().len() - 1;
                    FormAction::Continue
                } else if self.active_field_index() + 1 >= Self::field_count() {
                    FormAction::Submit
                } else {
                    self.next_field();
                    FormAction::NextField
                }
            }
            KeyCode::Up => {
                self.list_selected = self.list_selected.saturating_sub(1);
                FormAction::Continue
            }
            KeyCode::Down => {
                let len = self.active_list_ref().len();
                if len > 0 && self.list_selected + 1 < len {
                    self.list_selected += 1;
                }
                FormAction::Continue
            }
            _ => FormAction::Continue,
        }
    }

    /// Mutable reference to the item list of the active list field.
    fn active_list_mut(&mut self) -> &mut Vec<String> {
        match self.active_field {
            FormField::Ports => &mut self.ports,
            FormField::Volumes => &mut self.volumes,
            FormField::EnvVars => &mut self.env_vars,
            FormField::NetRules => &mut self.net_rules,
            _ => unreachable!("active_list_mut on non-list field"),
        }
    }

    /// Shared reference to the item list of the active list field.
    fn active_list_ref(&self) -> &Vec<String> {
        match self.active_field {
            FormField::Ports => &self.ports,
            FormField::Volumes => &self.volumes,
            FormField::EnvVars => &self.env_vars,
            FormField::NetRules => &self.net_rules,
            _ => unreachable!("active_list_ref on non-list field"),
        }
    }

    /// Remove the selected item from the active list, adjusting selection.
    fn remove_selected_item(&mut self) {
        let selected = self.list_selected;
        let items = self.active_list_mut();
        if !items.is_empty() && selected < items.len() {
            items.remove(selected);
            self.list_selected = if items.is_empty() {
                0
            } else if selected >= items.len() {
                items.len() - 1
            } else {
                selected
            };
        }
    }

    // -- conversion --

    /// Convert form state to a [`CreateSpec`] for the backend, validating
    /// all fields. Returns the first validation error if any.
    pub fn to_create_spec(&self) -> Result<CreateSpec> {
        // Image
        let image = self.image.trim();
        if image.is_empty() {
            bail!("Image is required");
        }

        // CPUs
        let cpus: u32 = self
            .cpus
            .trim()
            .parse()
            .map_err(|_| anyhow!("CPUs must be a positive integer"))?;
        if cpus == 0 {
            bail!("CPUs must be at least 1");
        }

        // Memory
        let memory = self.memory.trim();
        if !is_valid_memory(memory) {
            bail!("Memory must be like '512M' or '1G'");
        }

        // Ports
        let mut ports = Vec::new();
        for p in &self.ports {
            let p = p.trim();
            if p.is_empty() {
                continue;
            }
            ports.push(parse_port_spec(p)?);
        }

        // Name (optional — empty = auto-generated).
        let name = {
            let n = self.name.trim();
            if n.is_empty() {
                None
            } else {
                Some(n.to_string())
            }
        };

        // Workdir (optional).
        let workdir = {
            let w = self.workdir.trim();
            if w.is_empty() {
                None
            } else {
                Some(w.to_string())
            }
        };

        // Filter empty entries from string lists.
        let volumes = trim_filter(&self.volumes);
        let env = trim_filter(&self.env_vars);
        let net_rules = trim_filter(&self.net_rules);

        // Net profile → string for --net flag.
        let net_profile = Some(self.net_profile.as_str().to_string());

        Ok(CreateSpec {
            image: image.to_string(),
            name,
            cpus: Some(cpus),
            memory: Some(memory.to_string()),
            workdir,
            ports,
            volumes,
            env,
            labels: Vec::new(),
            net_profile,
            net_rules,
        })
    }
}

// ---------------------------------------------------------------------------
// validation helpers
// ---------------------------------------------------------------------------

/// Check that a memory string matches `<number>[K|M|G|T]` (case-insensitive)
/// or a plain number.
fn is_valid_memory(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() {
        return false;
    }
    let bytes = s.as_bytes();
    let last = bytes[bytes.len() - 1];
    if last.is_ascii_alphabetic() {
        let num = &s[..s.len() - 1];
        num.parse::<u64>().is_ok() && matches!(last.to_ascii_uppercase(), b'K' | b'M' | b'G' | b'T')
    } else {
        s.parse::<u64>().is_ok()
    }
}

/// Parse a port spec string in `HOST:GUEST` or `BIND:HOST:GUEST` form with
/// an optional `/udp` suffix.
fn parse_port_spec(s: &str) -> Result<PublishedPort> {
    let s = s.trim();
    let (spec, protocol) = match s.split_once('/') {
        Some((spec, proto)) => (spec, proto.to_lowercase()),
        None => (s, "tcp".to_string()),
    };
    if protocol != "tcp" && protocol != "udp" {
        bail!("invalid protocol '{protocol}', use tcp or udp");
    }
    let parts: Vec<&str> = spec.split(':').collect();
    let (host_bind, host_port, guest_port) = match parts.as_slice() {
        [port1, port2] => ("127.0.0.1", *port1, *port2),
        [bind, port1, port2] => (*bind, *port1, *port2),
        _ => bail!("port spec must be HOST:GUEST or BIND:HOST:GUEST"),
    };
    let host_port: u16 = host_port
        .parse()
        .map_err(|_| anyhow!("invalid host port '{host_port}'"))?;
    let guest_port: u16 = guest_port
        .parse()
        .map_err(|_| anyhow!("invalid guest port '{guest_port}'"))?;
    Ok(PublishedPort {
        host_bind: host_bind.to_string(),
        host_port,
        guest_port,
        protocol,
    })
}

/// Trim and filter empty strings from a list.
fn trim_filter(list: &[String]) -> Vec<String> {
    list.iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

// ---------------------------------------------------------------------------
// rendering
// ---------------------------------------------------------------------------

/// Render the create-sandbox form into `area`.
pub fn render_create_form(frame: &mut Frame, form: &CreateForm, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " Create Sandbox ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    // -- text fields --
    lines.push(text_field_line(
        "Image",
        &form.image,
        form.active_field == FormField::Image,
    ));

    // Autocomplete hints for the image field.
    if form.active_field == FormField::Image && !form.image.is_empty() {
        let matches: Vec<&str> = form
            .images
            .iter()
            .filter(|img| img.starts_with(form.image.as_str()))
            .take(3)
            .map(String::as_str)
            .collect();
        if !matches.is_empty() {
            lines.push(Line::from(vec![
                Span::raw("  └ "),
                Span::styled(matches.join("  "), Style::default().fg(Color::DarkGray)),
            ]));
        }
    }

    lines.push(text_field_line(
        "Name",
        &form.name,
        form.active_field == FormField::Name,
    ));
    lines.push(Line::from(Span::styled(
        "  (auto-generated if empty)",
        Style::default().fg(Color::DarkGray),
    )));

    // CPUs + Memory on one line.
    lines.push(Line::from(vec![
        field_label_span("CPUs", form.active_field == FormField::Cpus),
        Span::raw(" "),
        text_value_span(&form.cpus, form.active_field == FormField::Cpus),
        Span::raw("   "),
        field_label_span("Memory", form.active_field == FormField::Memory),
        Span::raw(" "),
        text_value_span(&form.memory, form.active_field == FormField::Memory),
    ]));

    lines.push(text_field_line(
        "Workdir",
        &form.workdir,
        form.active_field == FormField::Workdir,
    ));

    // -- network section --
    lines.push(Line::raw(""));
    lines.push(section_header("Network"));

    // Profile radio buttons.
    let profile_active = form.active_field == FormField::NetProfile;
    let mut profile_spans = vec![Span::raw("  Profile: ")];
    for p in NetProfile::ALL {
        let is_selected = form.net_profile == p;
        let marker = if is_selected { "(•)" } else { "( )" };
        let style = if is_selected && profile_active {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else if is_selected {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        profile_spans.push(Span::styled(format!("{marker} {}  ", p.as_str()), style));
    }
    lines.push(Line::from(profile_spans));

    // List fields.
    lines.extend(list_field_lines(
        "Ports",
        &form.ports,
        &form.list_input,
        form.active_field == FormField::Ports,
        form.list_selected,
    ));
    lines.extend(list_field_lines(
        "Rules",
        &form.net_rules,
        &form.list_input,
        form.active_field == FormField::NetRules,
        form.list_selected,
    ));

    // -- volumes section --
    lines.push(Line::raw(""));
    lines.push(section_header("Volumes"));
    lines.extend(list_field_lines(
        "Volumes",
        &form.volumes,
        &form.list_input,
        form.active_field == FormField::Volumes,
        form.list_selected,
    ));

    // -- environment section --
    lines.push(Line::raw(""));
    lines.push(section_header("Environment"));
    lines.extend(list_field_lines(
        "Env",
        &form.env_vars,
        &form.list_input,
        form.active_field == FormField::EnvVars,
        form.list_selected,
    ));

    // -- footer --
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "[Tab] next field  [Enter] create  [Esc] cancel  [+/-] add/remove items",
        Style::default().fg(Color::DarkGray),
    )));

    // -- error line --
    if let Some(err) = &form.error {
        lines.push(Line::from(vec![
            Span::styled("✗ ", Style::default().fg(Color::Red)),
            Span::styled(err, Style::default().fg(Color::Red)),
        ]));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

/// Build a labeled text-field line: `  Label:  [value█]`.
fn text_field_line(label: &str, value: &str, active: bool) -> Line<'static> {
    Line::from(vec![
        field_label_span(label, active),
        Span::raw(" "),
        text_value_span(value, active),
    ])
}

/// Styled label span for a field.
fn field_label_span(label: &str, active: bool) -> Span<'static> {
    let style = if active {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    };
    Span::styled(format!("  {label:<8}:"), style)
}

/// Styled value span for a text field, with cursor block when active.
fn text_value_span(value: &str, active: bool) -> Span<'static> {
    if active {
        Span::styled(format!("[{value}█]"), Style::default().fg(Color::White))
    } else {
        Span::styled(format!("[{value}]"), Style::default().fg(Color::DarkGray))
    }
}

/// Section header line (e.g. `── Network ──`).
fn section_header(title: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!("── {title} ──"),
        Style::default().fg(Color::DarkGray),
    ))
}

/// Build the lines for a list-type field.
fn list_field_lines(
    label: &str,
    items: &[String],
    input: &str,
    active: bool,
    selected: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Label line with hints.
    let label_style = if active {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    };
    let hint = if active { "  [+ Add]  [- Remove]" } else { "" };
    lines.push(Line::from(vec![
        Span::styled(format!("  {label:<8}:"), label_style),
        Span::styled(hint, Style::default().fg(Color::DarkGray)),
    ]));

    // Items.
    for (i, item) in items.iter().enumerate() {
        let is_sel = active && i == selected;
        let style = if is_sel {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        let marker = if is_sel { "▶ " } else { "  " };
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(marker, style),
            Span::styled(format!("[{item}]"), style),
        ]));
    }

    // Input line (only when active).
    if active {
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(format!("  [{input}█]"), Style::default().fg(Color::Yellow)),
        ]));
    }

    // Empty state hint.
    if items.is_empty() && !active {
        lines.push(Line::from(Span::styled(
            "    (empty)",
            Style::default().fg(Color::DarkGray),
        )));
    }

    lines
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn key_with(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    // ---- new() defaults ----

    #[test]
    fn new_defaults() {
        let form = CreateForm::new(vec!["alpine".into(), "python:3.12".into()]);
        assert_eq!(form.image, "");
        assert_eq!(form.name, "");
        assert_eq!(form.cpus, "1");
        assert_eq!(form.memory, "512M");
        assert_eq!(form.workdir, "");
        assert_eq!(form.net_profile, NetProfile::Public);
        assert!(form.ports.is_empty());
        assert!(form.volumes.is_empty());
        assert!(form.env_vars.is_empty());
        assert!(form.net_rules.is_empty());
        assert_eq!(form.active_field, FormField::Image);
        assert!(form.error.is_none());
        assert_eq!(form.images, vec!["alpine", "python:3.12"]);
        assert_eq!(form.active_field_index(), 0);
        assert_eq!(CreateForm::field_count(), 10);
    }

    // ---- to_create_spec: valid ----

    #[test]
    fn to_create_spec_valid() {
        let mut form = CreateForm::new(vec![]);
        form.image = "alpine".into();
        form.name = "my-sandbox".into();
        form.cpus = "2".into();
        form.memory = "1G".into();
        form.workdir = "/app".into();
        form.ports = vec!["8080:80".into(), "0.0.0.0:9090:90/udp".into()];
        form.volumes = vec!["./src:/app".into()];
        form.env_vars = vec!["DEBUG=true".into()];
        form.net_rules = vec!["allow@api.example.com".into()];
        form.net_profile = NetProfile::Private;

        let spec = form.to_create_spec().unwrap();
        assert_eq!(spec.image, "alpine");
        assert_eq!(spec.name, Some("my-sandbox".into()));
        assert_eq!(spec.cpus, Some(2));
        assert_eq!(spec.memory, Some("1G".into()));
        assert_eq!(spec.workdir, Some("/app".into()));
        assert_eq!(spec.ports.len(), 2);
        assert_eq!(spec.ports[0].host_port, 8080);
        assert_eq!(spec.ports[0].guest_port, 80);
        assert_eq!(spec.ports[0].host_bind, "127.0.0.1");
        assert_eq!(spec.ports[0].protocol, "tcp");
        assert_eq!(spec.ports[1].host_bind, "0.0.0.0");
        assert_eq!(spec.ports[1].protocol, "udp");
        assert_eq!(spec.volumes, vec!["./src:/app"]);
        assert_eq!(spec.env, vec!["DEBUG=true"]);
        assert_eq!(spec.net_rules, vec!["allow@api.example.com"]);
        assert_eq!(spec.net_profile, Some("private".into()));
        assert!(spec.labels.is_empty());
    }

    #[test]
    fn to_create_spec_empty_name_uses_none() {
        let mut form = CreateForm::new(vec![]);
        form.image = "alpine".into();
        let spec = form.to_create_spec().unwrap();
        assert_eq!(spec.name, None);
    }

    #[test]
    fn to_create_spec_net_profile_mapping() {
        let mut form = CreateForm::new(vec![]);
        form.image = "alpine".into();

        form.net_profile = NetProfile::Public;
        assert_eq!(
            form.to_create_spec().unwrap().net_profile,
            Some("public".into())
        );

        form.net_profile = NetProfile::Private;
        assert_eq!(
            form.to_create_spec().unwrap().net_profile,
            Some("private".into())
        );

        form.net_profile = NetProfile::Host;
        assert_eq!(
            form.to_create_spec().unwrap().net_profile,
            Some("host".into())
        );

        form.net_profile = NetProfile::None;
        assert_eq!(
            form.to_create_spec().unwrap().net_profile,
            Some("none".into())
        );

        form.net_profile = NetProfile::Custom;
        assert_eq!(
            form.to_create_spec().unwrap().net_profile,
            Some("custom".into())
        );
    }

    // ---- to_create_spec: validation errors ----

    #[test]
    fn to_create_spec_empty_image_errors() {
        let mut form = CreateForm::new(vec![]);
        form.image = "".into();
        assert!(form.to_create_spec().is_err());
    }

    #[test]
    fn to_create_spec_invalid_cpus_errors() {
        let mut form = CreateForm::new(vec![]);
        form.image = "alpine".into();
        form.cpus = "abc".into();
        assert!(form.to_create_spec().is_err());
    }

    #[test]
    fn to_create_spec_zero_cpus_errors() {
        let mut form = CreateForm::new(vec![]);
        form.image = "alpine".into();
        form.cpus = "0".into();
        assert!(form.to_create_spec().is_err());
    }

    #[test]
    fn to_create_spec_invalid_memory_errors() {
        let mut form = CreateForm::new(vec![]);
        form.image = "alpine".into();
        form.memory = "abc".into();
        assert!(form.to_create_spec().is_err());
    }

    #[test]
    fn to_create_spec_valid_memory_formats() {
        let mut form = CreateForm::new(vec![]);
        form.image = "alpine".into();

        for mem in &["512M", "1G", "256K", "2T", "512m", "1g", "1024"] {
            form.memory = (*mem).into();
            assert!(
                form.to_create_spec().is_ok(),
                "memory '{mem}' should be valid"
            );
        }
    }

    // ---- port validation ----

    #[test]
    fn parse_port_spec_host_guest() {
        let p = parse_port_spec("8080:80").unwrap();
        assert_eq!(p.host_bind, "127.0.0.1");
        assert_eq!(p.host_port, 8080);
        assert_eq!(p.guest_port, 80);
        assert_eq!(p.protocol, "tcp");
    }

    #[test]
    fn parse_port_spec_bind_host_guest_udp() {
        let p = parse_port_spec("0.0.0.0:9090:90/udp").unwrap();
        assert_eq!(p.host_bind, "0.0.0.0");
        assert_eq!(p.host_port, 9090);
        assert_eq!(p.guest_port, 90);
        assert_eq!(p.protocol, "udp");
    }

    #[test]
    fn parse_port_spec_invalid() {
        assert!(parse_port_spec("8080").is_err());
        assert!(parse_port_spec("abc:def").is_err());
        assert!(parse_port_spec("8080:80/icmp").is_err());
        assert!(parse_port_spec("1:2:3:4").is_err());
    }

    #[test]
    fn to_create_spec_invalid_port_errors() {
        let mut form = CreateForm::new(vec![]);
        form.image = "alpine".into();
        form.ports = vec!["notaport".into()];
        assert!(form.to_create_spec().is_err());
    }

    #[test]
    fn to_create_spec_skips_empty_ports() {
        let mut form = CreateForm::new(vec![]);
        form.image = "alpine".into();
        form.ports = vec!["".into(), "8080:80".into(), "".into()];
        let spec = form.to_create_spec().unwrap();
        assert_eq!(spec.ports.len(), 1);
    }

    // ---- field navigation ----

    #[test]
    fn tab_advances_field() {
        let mut form = CreateForm::new(vec![]);
        assert_eq!(form.active_field, FormField::Image);

        assert_eq!(form.handle_key(key(KeyCode::Tab)), FormAction::NextField);
        assert_eq!(form.active_field, FormField::Name);

        form.handle_key(key(KeyCode::Tab));
        assert_eq!(form.active_field, FormField::Cpus);
    }

    #[test]
    fn backtab_goes_prev_field() {
        let mut form = CreateForm::new(vec![]);
        form.active_field = FormField::Memory;

        assert_eq!(
            form.handle_key(key(KeyCode::BackTab)),
            FormAction::PrevField
        );
        assert_eq!(form.active_field, FormField::Cpus);
    }

    #[test]
    fn shift_tab_goes_prev_field() {
        let mut form = CreateForm::new(vec![]);
        form.active_field = FormField::Memory;

        assert_eq!(
            form.handle_key(key_with(KeyCode::Tab, KeyModifiers::SHIFT)),
            FormAction::PrevField
        );
        assert_eq!(form.active_field, FormField::Cpus);
    }

    #[test]
    fn tab_wraps_around() {
        let mut form = CreateForm::new(vec![]);
        form.active_field = FormField::NetRules; // last field

        form.handle_key(key(KeyCode::Tab));
        assert_eq!(form.active_field, FormField::Image); // wraps to first
    }

    #[test]
    fn backtab_wraps_around() {
        let mut form = CreateForm::new(vec![]);
        form.active_field = FormField::Image; // first field

        form.handle_key(key(KeyCode::BackTab));
        assert_eq!(form.active_field, FormField::NetRules); // wraps to last
    }

    // ---- text field input ----

    #[test]
    fn typing_appends_to_text_field() {
        let mut form = CreateForm::new(vec![]);
        form.active_field = FormField::Image;

        form.handle_key(key(KeyCode::Char('a')));
        form.handle_key(key(KeyCode::Char('l')));
        form.handle_key(key(KeyCode::Char('p')));
        assert_eq!(form.image, "alp");
    }

    #[test]
    fn backspace_removes_last_char() {
        let mut form = CreateForm::new(vec![]);
        form.active_field = FormField::Name;
        form.name = "test".into();

        form.handle_key(key(KeyCode::Backspace));
        assert_eq!(form.name, "tes");
    }

    #[test]
    fn enter_on_text_field_advances() {
        let mut form = CreateForm::new(vec![]);
        form.active_field = FormField::Image;

        assert_eq!(form.handle_key(key(KeyCode::Enter)), FormAction::NextField);
        assert_eq!(form.active_field, FormField::Name);
    }

    #[test]
    fn ctrl_chars_not_typed() {
        let mut form = CreateForm::new(vec![]);
        form.active_field = FormField::Image;

        form.handle_key(key_with(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(form.image, "");
    }

    // ---- net profile ----

    #[test]
    fn net_profile_down_cycles() {
        let mut form = CreateForm::new(vec![]);
        form.active_field = FormField::NetProfile;
        assert_eq!(form.net_profile, NetProfile::Public);

        form.handle_key(key(KeyCode::Down));
        assert_eq!(form.net_profile, NetProfile::Private);

        form.handle_key(key(KeyCode::Down));
        assert_eq!(form.net_profile, NetProfile::Host);
    }

    #[test]
    fn net_profile_up_cycles_reverse() {
        let mut form = CreateForm::new(vec![]);
        form.active_field = FormField::NetProfile;
        form.net_profile = NetProfile::Private;

        form.handle_key(key(KeyCode::Up));
        assert_eq!(form.net_profile, NetProfile::Public);
    }

    #[test]
    fn net_profile_wraps() {
        let mut form = CreateForm::new(vec![]);
        form.active_field = FormField::NetProfile;
        form.net_profile = NetProfile::Custom; // last

        form.handle_key(key(KeyCode::Down));
        assert_eq!(form.net_profile, NetProfile::Public); // wraps
    }

    // ---- list field: adding items ----

    #[test]
    fn enter_commits_list_input() {
        let mut form = CreateForm::new(vec![]);
        form.active_field = FormField::Ports;

        // Type a port spec.
        for c in "80:80".chars() {
            form.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(form.list_input, "80:80");

        // Enter commits it.
        form.handle_key(key(KeyCode::Enter));
        assert_eq!(form.ports, vec!["80:80"]);
        assert_eq!(form.list_input, "");
    }

    #[test]
    fn plus_adds_empty_item() {
        let mut form = CreateForm::new(vec![]);
        form.active_field = FormField::Volumes;

        form.handle_key(key(KeyCode::Char('+')));
        assert_eq!(form.volumes, vec![""]);
    }

    #[test]
    fn plus_with_input_appends_char() {
        let mut form = CreateForm::new(vec![]);
        form.active_field = FormField::Volumes;
        form.handle_key(key(KeyCode::Char('.')));
        form.handle_key(key(KeyCode::Char('/')));

        // '+' should go into the input, not add an item.
        form.handle_key(key(KeyCode::Char('+')));
        assert_eq!(form.list_input, "./+");
        assert!(form.volumes.is_empty());
    }

    // ---- list field: removing items ----

    #[test]
    fn delete_removes_selected_item() {
        let mut form = CreateForm::new(vec![]);
        form.active_field = FormField::Ports;
        form.ports = vec!["8080:80".into(), "9090:90".into()];
        form.list_selected = 0;

        form.handle_key(key(KeyCode::Delete));
        assert_eq!(form.ports, vec!["9090:90"]);
        assert_eq!(form.list_selected, 0);
    }

    #[test]
    fn minus_removes_selected_item() {
        let mut form = CreateForm::new(vec![]);
        form.active_field = FormField::EnvVars;
        form.env_vars = vec!["A=1".into(), "B=2".into()];
        form.list_selected = 1;

        form.handle_key(key(KeyCode::Char('-')));
        assert_eq!(form.env_vars, vec!["A=1"]);
        assert_eq!(form.list_selected, 0);
    }

    #[test]
    fn minus_with_input_appends_char() {
        let mut form = CreateForm::new(vec![]);
        form.active_field = FormField::EnvVars;
        form.handle_key(key(KeyCode::Char('A')));

        // '-' goes into input.
        form.handle_key(key(KeyCode::Char('-')));
        assert_eq!(form.list_input, "A-");
        assert!(form.env_vars.is_empty());
    }

    #[test]
    fn delete_on_empty_list_is_noop() {
        let mut form = CreateForm::new(vec![]);
        form.active_field = FormField::Ports;
        assert!(form.ports.is_empty());

        form.handle_key(key(KeyCode::Delete));
        assert!(form.ports.is_empty());
    }

    #[test]
    fn delete_removes_last_item_resets_selection() {
        let mut form = CreateForm::new(vec![]);
        form.active_field = FormField::Ports;
        form.ports = vec!["8080:80".into()];
        form.list_selected = 0;

        form.handle_key(key(KeyCode::Delete));
        assert!(form.ports.is_empty());
        assert_eq!(form.list_selected, 0);
    }

    // ---- list field: navigation ----

    #[test]
    fn down_arrow_navigates_list() {
        let mut form = CreateForm::new(vec![]);
        form.active_field = FormField::Ports;
        form.ports = vec!["a".into(), "b".into(), "c".into()];
        form.list_selected = 0;

        form.handle_key(key(KeyCode::Down));
        assert_eq!(form.list_selected, 1);

        form.handle_key(key(KeyCode::Down));
        assert_eq!(form.list_selected, 2);

        // Clamps at last.
        form.handle_key(key(KeyCode::Down));
        assert_eq!(form.list_selected, 2);
    }

    #[test]
    fn up_arrow_navigates_list() {
        let mut form = CreateForm::new(vec![]);
        form.active_field = FormField::Ports;
        form.ports = vec!["a".into(), "b".into()];
        form.list_selected = 1;

        form.handle_key(key(KeyCode::Up));
        assert_eq!(form.list_selected, 0);

        // Clamps at 0.
        form.handle_key(key(KeyCode::Up));
        assert_eq!(form.list_selected, 0);
    }

    // ---- submit / cancel ----

    #[test]
    fn enter_on_last_field_submits() {
        let mut form = CreateForm::new(vec![]);
        form.active_field = FormField::NetRules; // last field
        form.list_input.clear();

        assert_eq!(form.handle_key(key(KeyCode::Enter)), FormAction::Submit);
    }

    #[test]
    fn enter_on_last_field_with_input_commits_not_submits() {
        let mut form = CreateForm::new(vec![]);
        form.active_field = FormField::NetRules;
        form.handle_key(key(KeyCode::Char('x')));

        assert_eq!(form.handle_key(key(KeyCode::Enter)), FormAction::Continue);
        assert_eq!(form.net_rules, vec!["x"]);
    }

    #[test]
    fn esc_cancels() {
        let mut form = CreateForm::new(vec![]);
        assert_eq!(form.handle_key(key(KeyCode::Esc)), FormAction::Cancel);
    }

    // ---- switching fields resets list state ----

    #[test]
    fn switching_fields_clears_list_input() {
        let mut form = CreateForm::new(vec![]);
        form.active_field = FormField::Ports;
        form.handle_key(key(KeyCode::Char('8')));
        assert_eq!(form.list_input, "8");

        // Tab to Volumes — list input should be cleared.
        form.handle_key(key(KeyCode::Tab));
        assert_eq!(form.list_input, "");
        assert_eq!(form.active_field, FormField::Volumes);
    }
}
