//! Create-sandbox form: quick path by default, advanced fields on demand.
//!
//! UX (per docs/DESIGN.md "quick + advanced"):
//! - Quick mode shows only **Image** (with an always-visible suggestion
//!   picker) and an optional **Name**. Everything else takes SDK defaults:
//!   no cpus/memory limit, `public` network profile, no ports/volumes/env.
//! - `Ctrl+A` expands the advanced fields (cpus, memory, workdir, ports,
//!   volumes, env, labels, network profile); they are prefilled empty, and
//!   empty means "use the runtime default".
//! - The image picker is not a modal: typing filters it, `↑↓` moves the
//!   highlight, `Enter` adopts the highlighted suggestion.

use anyhow::{Result, anyhow, bail};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::backend::CreateSpec;
use crate::models::PublishedPort;
use crate::ui::theme::THEME;

/// Rows visible in the image picker (it scrolls beyond this).
const PICKER_VISIBLE: usize = 5;
/// Safety cap for the full suggestion list (the picker scrolls through it).
const PICKER_WINDOW_MAX: usize = 50;

/// Curated image suggestions shown when the local image list is empty or
/// as stable defaults above it.
pub const SUGGESTED_IMAGES: &[&str] = &[
    "alpine",
    "debian",
    "ubuntu",
    "fedora",
    "archlinux",
    "python",
    "node",
    "rust",
    "nginx",
    "redis",
    "postgres",
];

// ---------------------------------------------------------------------------
// NetProfile
// ---------------------------------------------------------------------------

/// Network profile selected in the advanced section.
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

    /// String used for the backend network profile.
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

/// Which form field is currently active (advanced section only; quick mode
/// always starts on [`FormField::Image`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormField {
    Image,
    Name,
    MountCwd,
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
    /// All advanced fields in Tab order.
    pub const ALL: [Self; 11] = [
        Self::Image,
        Self::Name,
        Self::MountCwd,
        Self::Cpus,
        Self::Memory,
        Self::Workdir,
        Self::NetProfile,
        Self::Ports,
        Self::Volumes,
        Self::EnvVars,
        Self::NetRules,
    ];

    /// Quick-mode fields in Tab order.
    pub const QUICK: [Self; 3] = [Self::Image, Self::Name, Self::MountCwd];

    /// Whether this field is a list-type (items + input buffer).
    pub fn is_list(&self) -> bool {
        matches!(
            self,
            Self::Ports | Self::Volumes | Self::EnvVars | Self::NetRules
        )
    }

    /// Whether this field edits a plain text buffer.
    fn is_text(&self) -> bool {
        matches!(
            self,
            Self::Image | Self::Name | Self::Cpus | Self::Memory | Self::Workdir
        )
    }

    /// Human-readable label.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Image => "Image",
            Self::Name => "Name",
            Self::MountCwd => "Mount current dir",
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
/// Quick mode ([`CreateForm::advanced`] == false) exposes only the image
/// picker and an optional name; all other settings stay empty and are
/// mapped to runtime defaults in [`CreateForm::to_create_spec`].
#[derive(Debug)]
pub struct CreateForm {
    /// Image reference (e.g. `alpine`, `python:3.12`).
    pub image: String,
    /// Optional sandbox name (empty = auto-generated).
    pub name: String,
    /// vCPU count as a text string; empty = runtime default.
    pub cpus: String,
    /// Memory limit string (e.g. `512M`); empty = runtime default.
    pub memory: String,
    /// Working directory inside the sandbox.
    pub workdir: String,
    /// Whether to bind-mount the TUI's current working directory into the
    /// sandbox at the same absolute path (Docker-sbx workspace behavior).
    /// Defaults to `true` — sbx mounts the current dir on every run.
    pub mount_cwd: bool,
    /// Host path captured when the form opened; the mount source for
    /// [`Self::mount_cwd`] and the default value of the workdir field.
    pub cwd: String,
    /// Selected network profile (advanced only).
    pub net_profile: NetProfile,
    /// Published port specs (e.g. `8080:80`, `0.0.0.0:9090:90/udp`).
    pub ports: Vec<String>,
    /// Volume mount specs (e.g. `./src:/app`, `mydata:/data`).
    pub volumes: Vec<String>,
    /// Environment variables in `KEY=VALUE` form.
    pub env_vars: Vec<String>,
    /// Network rule strings (e.g. `allow@api.example.com`).
    pub net_rules: Vec<String>,
    /// Whether the advanced fields are expanded.
    pub advanced: bool,
    /// Currently active field.
    pub active_field: FormField,
    /// Validation error to display (red line).
    pub error: Option<String>,
    /// Cached images (from `list_images`), shown in the picker with size.
    pub images: Vec<crate::models::Image>,
    /// Highlighted index within the current image suggestions.
    pub picker_selected: usize,
    /// Input buffer for the active list field.
    list_input: String,
    /// Selected item index within the active list field.
    list_selected: usize,
}

impl CreateForm {
    /// Initialize a quick form with runtime defaults and no images yet.
    ///
    /// The workspace mount defaults to **on** (Docker-sbx behavior): the TUI's
    /// current directory is mounted at the same absolute path and becomes the
    /// workdir. Unchecking clears the auto-filled workdir (empty = image
    /// default — see `toggle_mount_cwd`).
    pub fn new(images: Vec<crate::models::Image>) -> Self {
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self {
            image: String::new(),
            name: String::new(),
            cpus: String::new(),
            memory: String::new(),
            workdir: cwd.clone(),
            mount_cwd: true,
            cwd,
            net_profile: NetProfile::Public,
            ports: Vec::new(),
            volumes: Vec::new(),
            env_vars: Vec::new(),
            net_rules: Vec::new(),
            advanced: false,
            active_field: FormField::Image,
            error: None,
            images,
            picker_selected: 0,
            list_input: String::new(),
            list_selected: 0,
        }
    }

    /// Toggle the current-dir workspace mount and keep the workdir field
    /// coherent: checked → workdir becomes the mount path (unless the user
    /// typed their own), unchecked → the auto-filled mount path is cleared
    /// (empty = image default; the SDK stats explicit workdirs in the guest,
    /// and plain OCI images lack the sbx template's `/home/agent/workspace`).
    pub fn toggle_mount_cwd(&mut self) {
        self.mount_cwd = !self.mount_cwd;
        if self.mount_cwd {
            // Restore the mount default unless the user chose a custom dir.
            if self.workdir.trim().is_empty() {
                self.workdir = self.cwd.clone();
            }
        } else {
            // Drop the auto-filled value; an explicitly typed one survives.
            if self.workdir == self.cwd {
                self.workdir.clear();
            }
        }
    }

    /// Index of the active field within the visible field list.
    pub fn active_field_index(&self) -> usize {
        self.visible_fields()
            .iter()
            .position(|&f| f == self.active_field)
            .unwrap_or(0)
    }

    /// Number of visible fields (quick: 3, advanced: 11).
    pub fn field_count(&self) -> usize {
        self.visible_fields().len()
    }

    /// The fields visible in the current mode, in Tab order.
    fn visible_fields(&self) -> &'static [FormField] {
        if self.advanced {
            &FormField::ALL
        } else {
            &FormField::QUICK
        }
    }

    /// Image suggestions for the picker: locally pulled images first (they
    /// exist on this machine), then curated defaults, filtered by the
    /// current image buffer (case-insensitive substring). The list is NOT
    /// truncated to the visible window — the picker scrolls instead
    /// ([`PICKER_VISIBLE`] rows, [`PICKER_WINDOW_MAX`] safety cap).
    pub fn suggestions(&self) -> Vec<String> {
        let mut all: Vec<String> = Vec::with_capacity(SUGGESTED_IMAGES.len() + self.images.len());
        for img in &self.images {
            if !all.contains(&img.reference) {
                all.push(img.reference.clone());
            }
        }
        for s in SUGGESTED_IMAGES {
            let s = (*s).to_string();
            if !all.contains(&s) {
                all.push(s);
            }
        }
        let filter = self.image.trim().to_lowercase();
        if filter.is_empty() {
            all.truncate(PICKER_WINDOW_MAX);
            return all;
        }
        all.into_iter()
            .filter(|img| img.to_lowercase().contains(&filter))
            .take(PICKER_WINDOW_MAX)
            .collect()
    }

    /// The size of a cached image by reference, if known (shown in the
    /// picker next to the reference).
    pub fn image_size(&self, reference: &str) -> Option<u64> {
        self.images
            .iter()
            .find(|i| i.reference == reference)
            .map(|i| i.size_bytes)
    }

    /// Process a key and return what the caller should do.
    pub fn handle_key(&mut self, key: KeyEvent) -> FormAction {
        // Global keys.
        if key.code == KeyCode::Char('a') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.toggle_advanced();
            return FormAction::Continue;
        }
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
        if self.active_field == FormField::MountCwd {
            self.handle_mount_cwd_key(key)
        } else if self.active_field.is_list() {
            self.handle_list_key(key)
        } else if self.active_field == FormField::NetProfile {
            self.handle_net_profile_key(key)
        } else {
            self.handle_text_key(key)
        }
    }

    // -- navigation --

    /// Show or hide the advanced fields. Expanding keeps the focus where it
    /// is (Image/Name exist in both modes); collapsing resets the focus to
    /// the image picker.
    pub fn toggle_advanced(&mut self) {
        self.advanced = !self.advanced;
        if !self.advanced {
            self.set_active_field(FormField::Image);
        } else {
            self.list_input.clear();
            self.list_selected = 0;
        }
    }

    /// Advance to the next visible field (wraps around).
    fn next_field(&mut self) {
        let fields = self.visible_fields();
        let idx = self.active_field_index();
        let next = (idx + 1) % fields.len();
        self.set_active_field(fields[next]);
    }

    /// Go back to the previous visible field (wraps around).
    fn prev_field(&mut self) {
        let fields = self.visible_fields();
        let idx = self.active_field_index();
        let prev = (idx + fields.len() - 1) % fields.len();
        self.set_active_field(fields[prev]);
    }

    /// Switch the active field and reset list-editing + picker state.
    fn set_active_field(&mut self, field: FormField) {
        self.active_field = field;
        self.list_input.clear();
        self.list_selected = 0;
        self.picker_selected = 0;
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
                self.picker_selected = 0;
                FormAction::Continue
            }
            KeyCode::Backspace => {
                self.active_text_buf().pop();
                self.picker_selected = 0;
                FormAction::Continue
            }
            KeyCode::Up | KeyCode::Down if self.active_field == FormField::Image => {
                self.move_picker(if key.code == KeyCode::Up { -1 } else { 1 });
                FormAction::Continue
            }
            KeyCode::Enter if self.active_field == FormField::Image => self.enter_on_image(),
            KeyCode::Enter => {
                if self.active_field_index() + 1 >= self.field_count() {
                    FormAction::Submit
                } else {
                    self.next_field();
                    FormAction::NextField
                }
            }
            _ => FormAction::Continue,
        }
    }

    /// Enter on the image field: adopt the highlighted suggestion, or — if
    /// a custom reference was typed without a matching suggestion — move on
    /// to the name field.
    fn enter_on_image(&mut self) -> FormAction {
        let suggestions = self.suggestions();
        if let Some(pick) = suggestions.get(self.picker_selected) {
            self.image = pick.clone();
            self.next_field();
            FormAction::NextField
        } else if !self.image.trim().is_empty() {
            self.next_field();
            FormAction::NextField
        } else {
            self.error = Some("Pick an image with ↑↓ + Enter, or type one".into());
            FormAction::Continue
        }
    }

    /// Move the picker highlight by `delta`, clamped to the list.
    fn move_picker(&mut self, delta: isize) {
        let len = self.suggestions().len();
        if len == 0 {
            return;
        }
        let selected = self.picker_selected as isize + delta;
        self.picker_selected = selected.clamp(0, len as isize - 1) as usize;
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

    // -- workspace mount toggle handling --

    /// Space toggles the current-dir mount; Enter advances like a text field.
    fn handle_mount_cwd_key(&mut self, key: KeyEvent) -> FormAction {
        match key.code {
            KeyCode::Char(' ') => {
                self.toggle_mount_cwd();
                FormAction::Continue
            }
            KeyCode::Enter => {
                if self.active_field_index() + 1 >= self.field_count() {
                    FormAction::Submit
                } else {
                    self.next_field();
                    FormAction::NextField
                }
            }
            _ => FormAction::Continue,
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
                if self.active_field_index() + 1 >= self.field_count() {
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
                } else if self.active_field_index() + 1 >= self.field_count() {
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

    /// The workdir value the form state implies by itself (the mount path
    /// when checked, nothing when not) — used to detect *deliberately* typed
    /// workdirs for the quick-mode summary.
    fn auto_workdir(&self) -> String {
        if self.mount_cwd {
            self.cwd.clone()
        } else {
            String::new()
        }
    }

    /// One-line summary of the set advanced fields (shown in quick mode so
    /// deliberately-set values stay visible).
    pub fn advanced_summary(&self) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        if !self.cpus.trim().is_empty() {
            parts.push(format!("{} cpus", self.cpus.trim()));
        }
        if !self.memory.trim().is_empty() {
            parts.push(self.memory.trim().to_string());
        }
        if !self.workdir.trim().is_empty() && self.workdir.trim() != self.auto_workdir() {
            parts.push(format!("wd {}", self.workdir.trim()));
        }
        if !self.mount_cwd {
            parts.push("no mount".to_string());
        }
        if self.net_profile != NetProfile::Public {
            parts.push(self.net_profile.as_str().to_string());
        }
        for (items, label) in [
            (&self.ports, "port"),
            (&self.volumes, "volume"),
            (&self.env_vars, "env"),
            (&self.net_rules, "rule"),
        ] {
            if !items.is_empty() {
                parts.push(format!("{} {}s", items.len(), label));
            }
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" · "))
        }
    }

    // -- conversion --

    /// Convert form state to a [`CreateSpec`] for the backend, validating
    /// all fields. Empty advanced fields mean "runtime default" (`None`).
    pub fn to_create_spec(&self) -> Result<CreateSpec> {
        // Image
        let image = self.image.trim();
        if image.is_empty() {
            bail!("Image is required");
        }

        // CPUs — empty means runtime default.
        let cpus = match self.cpus.trim() {
            "" => None,
            s => {
                let cpus: u32 = s
                    .parse()
                    .map_err(|_| anyhow!("CPUs must be a positive integer"))?;
                if cpus == 0 {
                    bail!("CPUs must be at least 1");
                }
                Some(cpus)
            }
        };

        // Memory — empty means runtime default.
        let memory = match self.memory.trim() {
            "" => None,
            s if is_valid_memory(s) => Some(s.to_string()),
            _ => bail!("Memory must be like '512M' or '1G'"),
        };

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

        // Workdir: explicit input wins; with the workspace mount on (and no
        // explicit entry) the mounted path is the default — sbx starts the
        // sandbox in its primary workspace. Unchecked means NO explicit
        // workdir: the SDK stats the workdir inside the guest rootfs at
        // create time, and plain OCI images don't contain
        // `/home/agent/workspace` (it's baked into sbx template images only)
        // — mountless sandboxes start in the image's own working directory,
        // exactly like `sbx run` without a workspace.
        let mut workspace_mount: Option<String> = None;
        if self.mount_cwd {
            let cwd = self.cwd.trim();
            if !cwd.starts_with('/') {
                bail!("Workspace mount: {cwd} is not an absolute path");
            }
            if !std::path::Path::new(cwd).exists() {
                bail!("Workspace mount: {cwd} does not exist (deleted after open?)");
            }
            workspace_mount = Some(cwd.to_string());
        }
        let workdir = {
            let w = self.workdir.trim();
            if !w.is_empty() {
                Some(w.to_string())
            } else {
                workspace_mount.clone()
            }
        };

        // Filter empty entries from string lists.
        let mut volumes = trim_filter(&self.volumes);
        if let Some(cwd) = workspace_mount {
            // Bind-mount the current dir at the same absolute path (sbx:
            // paths match between host and guest so stack traces line up).
            let spec = format!("{cwd}:{cwd}");
            if !volumes.contains(&spec) {
                volumes.insert(0, spec);
            }
        }
        let env = trim_filter(&self.env_vars);
        let net_rules = trim_filter(&self.net_rules);

        // Quick mode leaves the profile at the runtime default; advanced
        // mode applies the explicitly selected profile.
        let net_profile = if self.advanced {
            Some(self.net_profile.as_str().to_string())
        } else {
            None
        };

        Ok(CreateSpec {
            image: image.to_string(),
            name,
            cpus,
            memory,
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

/// Render the create-sandbox form into `area` (full frame; chrome included).
pub fn render_create_form(frame: &mut Frame, form: &CreateForm, area: Rect) {
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

    let t = &THEME;
    let mode = if form.advanced { "Advanced" } else { "Quick" };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            format!(" ⚡ QUICK CREATE MICROVM — {mode} "),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(t.accent))
        .style(Style::default().bg(t.panel));

    let inner = block.inner(areas.body);
    frame.render_widget(block, areas.body);

    let mut lines: Vec<Line> = Vec::new();

    // Image field with an always-visible suggestion picker.
    lines.push(text_field_line(
        "Image",
        &form.image,
        form.active_field == FormField::Image,
    ));
    lines.extend(picker_lines(form));

    lines.push(text_field_line(
        "Name",
        &form.name,
        form.active_field == FormField::Name,
    ));
    if form.active_field == FormField::Name || !form.advanced {
        lines.push(Line::from(Span::styled(
            "  (auto-generated if empty)",
            Style::default().fg(t.muted),
        )));
    }

    // Workspace mount toggle (Docker-sbx style): Space toggles, Enter moves
    // on. Shows the exact host path being mounted.
    let mount_active = form.active_field == FormField::MountCwd;
    let (marker, marker_color) = if form.mount_cwd {
        ("[x]", t.ok)
    } else {
        ("[ ]", t.muted)
    };
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(marker, Style::default().fg(marker_color)),
        Span::raw(" "),
        Span::styled(
            "Mount current dir",
            if mount_active {
                Style::default().fg(t.fg).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(t.text)
            },
        ),
        Span::styled(
            format!("  → {} (space toggles)", form.cwd),
            Style::default().fg(t.muted),
        ),
    ]));

    // Quick mode: summarize advanced values the user set earlier.
    if let Some(summary) = form.advanced_summary() {
        lines.push(Line::from(vec![
            Span::styled("⚙ ", Style::default().fg(t.accent)),
            Span::styled(summary, Style::default().fg(t.text)),
        ]));
    }

    if form.advanced {
        lines.extend(advanced_field_lines(form));
    } else {
        lines.push(Line::from(Span::styled(
            "  ^a for ports, volumes, cpus, …",
            Style::default().fg(t.muted),
        )));
    }

    // -- footer --
    lines.push(Line::raw(""));
    let footer = if form.advanced {
        "[Tab] next field  [Enter] create  [^a] quick  [Esc] cancel"
    } else {
        "[↑↓] pick image  [Enter] next  [^a] advanced  [Esc] cancel"
    };
    lines.push(Line::from(Span::styled(
        footer,
        Style::default().fg(t.muted),
    )));

    // -- error line --
    if let Some(err) = &form.error {
        lines.push(Line::from(vec![
            Span::styled("✗ ", Style::default().fg(t.err)),
            Span::styled(err, Style::default().fg(t.err)),
        ]));
    }

    frame.render_widget(Paragraph::new(lines), inner);

    // Chrome footer over the legacy hint line.
    let hints = vec![
        crate::ui::chrome::FooterHint {
            key: "[Tab]",
            label: "next field",
            role: crate::ui::chrome::FooterRole::Plain,
        },
        crate::ui::chrome::FooterHint {
            key: "[Enter]",
            label: if form.advanced { "create" } else { "next" },
            role: crate::ui::chrome::FooterRole::Accent,
        },
        crate::ui::chrome::FooterHint {
            key: "[^a]",
            label: if form.advanced { "quick" } else { "advanced" },
            role: crate::ui::chrome::FooterRole::Warn,
        },
        crate::ui::chrome::FooterHint {
            key: "[Esc]",
            label: "cancel",
            role: crate::ui::chrome::FooterRole::Err,
        },
    ];
    crate::ui::chrome::render_chrome_footer(frame, &areas, &hints);
}

/// The sliding window of picker rows: as many as [`PICKER_VISIBLE`] fit,
/// scrolled so `selected` stays visible (same rule as the card rail).
/// Returns `(start, count)`.
fn picker_window(total: usize, selected: usize) -> (usize, usize) {
    if total == 0 {
        return (0, 0);
    }
    let visible = total.min(PICKER_VISIBLE);
    let start = selected
        .saturating_sub(visible.saturating_sub(1))
        .min(total.saturating_sub(visible));
    (start, visible)
}

/// The picker's suggestion rows for the sliding window around
/// `picker_selected`, with the cached image size (when known) next to the
/// reference and the highlighted row marked.
fn picker_lines(form: &CreateForm) -> Vec<Line<'static>> {
    let t = &THEME;
    let suggestions = form.suggestions();
    if suggestions.is_empty() {
        return Vec::new();
    }
    let active = form.active_field == FormField::Image;
    let (start, count) = picker_window(suggestions.len(), form.picker_selected);
    suggestions[start..start + count]
        .iter()
        .enumerate()
        .map(|(i, img)| {
            let idx = start + i;
            let selected = active && idx == form.picker_selected;
            let marker = if selected { "▶ " } else { "  " };
            let style = if selected {
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(t.text)
            };
            // Cached image size next to the reference (curated suggestions
            // without a cached image show none). Display name shortens
            // digest-pinned variants; adoption keeps the full reference.
            let size = form
                .image_size(img)
                .map(|b| format!("  {}", crate::ui::dashboard::format_bytes(b)))
                .unwrap_or_default();
            let display = form
                .images
                .iter()
                .find(|i| i.reference == *img)
                .map(|i| i.display_name())
                .unwrap_or_else(|| img.clone());
            Line::from(vec![
                Span::raw("  └ "),
                Span::styled(format!("{marker}{display}"), style),
                Span::styled(size, Style::default().fg(t.muted)),
            ])
        })
        .collect()
}

/// Advanced-mode field lines (network, ports, volumes, environment).
fn advanced_field_lines(form: &CreateForm) -> Vec<Line<'static>> {
    // CPUs + Memory on one line.
    let mut lines = vec![Line::from(vec![
        field_label_span("CPUs", form.active_field == FormField::Cpus),
        Span::raw(" "),
        text_value_span(&form.cpus, form.active_field == FormField::Cpus),
        Span::raw("   "),
        field_label_span("Memory", form.active_field == FormField::Memory),
        Span::raw(" "),
        text_value_span(&form.memory, form.active_field == FormField::Memory),
    ])];

    lines.push(text_field_line(
        "Workdir",
        &form.workdir,
        form.active_field == FormField::Workdir,
    ));

    // -- network section --
    lines.push(Line::raw(""));
    lines.push(section_header("Network"));

    let profile_active = form.active_field == FormField::NetProfile;
    let mut profile_spans = vec![Span::raw("  Profile: ")];
    for p in NetProfile::ALL {
        let is_selected = form.net_profile == p;
        let marker = if is_selected { "(•)" } else { "( )" };
        let style = if is_selected && profile_active {
            Style::default()
                .fg(THEME.accent)
                .add_modifier(Modifier::BOLD)
        } else if is_selected {
            Style::default().fg(THEME.accent)
        } else {
            Style::default().fg(THEME.muted)
        };
        profile_spans.push(Span::styled(format!("{marker} {}  ", p.as_str()), style));
    }
    lines.push(Line::from(profile_spans));

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

    lines
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
    let t = &THEME;
    let style = if active {
        Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(t.muted)
    };
    Span::styled(format!("  {label:<8}:"), style)
}

/// Styled value span for a text field, with cursor block when active.
fn text_value_span(value: &str, active: bool) -> Span<'static> {
    let t = &THEME;
    if active {
        Span::styled(format!("[{value}█]"), Style::default().fg(t.fg))
    } else {
        Span::styled(format!("[{value}]"), Style::default().fg(t.muted))
    }
}

/// Section header line (e.g. `── Network ──`).
fn section_header(title: &str) -> Line<'static> {
    let t = &THEME;
    Line::from(Span::styled(
        format!("── {title} ──"),
        Style::default().fg(t.muted),
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
    let t = &THEME;
    let mut lines = Vec::new(); // Label line with hints.
    let label_style = if active {
        Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(t.muted)
    };
    let hint = if active { "  [+ Add]  [- Remove]" } else { "" };
    lines.push(Line::from(vec![
        Span::styled(format!("  {label:<8}:"), label_style),
        Span::styled(hint, Style::default().fg(t.muted)),
    ]));

    // Items.
    for (i, item) in items.iter().enumerate() {
        let is_sel = active && i == selected;
        let style = if is_sel {
            Style::default().fg(t.fg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.text)
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
            Span::styled(format!("  [{input}█]"), Style::default().fg(t.warn)),
        ]));
    }

    // Empty state hint.
    if items.is_empty() && !active {
        lines.push(Line::from(Span::styled(
            "    (empty)",
            Style::default().fg(t.muted),
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

    fn key_mod(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    fn img(reference: &str) -> crate::models::Image {
        crate::models::Image {
            architecture: "amd64".into(),
            created_at: chrono::Utc::now(),
            digest: format!("sha256:{reference}"),
            layer_count: 1,
            os: "linux".into(),
            reference: reference.into(),
            size_bytes: 1024,
        }
    }

    fn flatten(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    // -- image picker: scrolling window (smoke finding: list was capped at
    //    8 with no scrolling, snapshot digest variants crowded it out) --

    #[test]
    fn suggestions_include_all_local_images() {
        // 12 local images: the list must NOT be hard-truncated at 8 —
        // the picker scrolls instead, so nothing is unreachable.
        let images: Vec<crate::models::Image> = (0..12).map(|i| img(&format!("img{i}"))).collect();
        let form = CreateForm::new(images);
        let s = form.suggestions();
        for i in 0..12 {
            assert!(s.contains(&format!("img{i}")), "img{i} missing from {s:?}");
        }
    }

    #[test]
    fn picker_window_slides_with_selection() {
        // Same rule as the card rail: the window slides once the selection
        // passes the bottom row.
        assert_eq!(picker_window(10, 0), (0, 5));
        assert_eq!(picker_window(10, 4), (0, 5));
        assert_eq!(picker_window(10, 5), (1, 5));
        assert_eq!(picker_window(10, 9), (5, 5));
        // Fewer suggestions than the window: show them all.
        assert_eq!(picker_window(3, 2), (0, 3));
        assert_eq!(picker_window(0, 0), (0, 0));
    }

    #[test]
    fn picker_lines_render_scrolling_window() {
        let images: Vec<crate::models::Image> = (0..10).map(|i| img(&format!("img{i}"))).collect();
        let mut form = CreateForm::new(images);
        form.picker_selected = 7;
        let lines = picker_lines(&form);
        assert_eq!(lines.len(), 5, "visible window, not the full list");
        let text = flatten(&lines);
        assert!(text.contains("img7"), "selection inside the window: {text}");
        assert!(
            !text.contains("img0 "),
            "entries above the window are hidden: {text}"
        );

        // Selection at the top shows the first window.
        form.picker_selected = 0;
        let text = flatten(&picker_lines(&form));
        assert!(text.contains("img0"));
        assert!(!text.contains("img5 "));
    }

    #[test]
    fn picker_lines_show_image_size() {
        let mut alpine = img("alpine");
        alpine.size_bytes = 3_849_738;
        let form = CreateForm::new(vec![alpine]);
        let text = flatten(&picker_lines(&form));
        assert!(
            text.contains("3.7M"),
            "image size shown next to the reference: {text}"
        );
    }

    // -- quick mode --

    #[test]
    fn new_starts_quick_on_image() {
        let form = CreateForm::new(Vec::new());
        assert!(!form.advanced);
        assert_eq!(form.active_field, FormField::Image);
        assert_eq!(form.field_count(), 3);
        assert!(form.error.is_none());
    }

    #[test]
    fn quick_spec_uses_runtime_defaults() {
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        // The sbx-style workspace mount is on by default; everything else
        // stays at runtime defaults.
        let spec = form.to_create_spec().unwrap();
        assert_eq!(spec.image, "alpine");
        assert_eq!(spec.cpus, None);
        assert_eq!(spec.memory, None);
        assert_eq!(spec.workdir.as_deref(), Some(form.cwd.as_str()));
        assert_eq!(spec.net_profile, None);
        assert!(spec.ports.is_empty());
        assert_eq!(spec.volumes, vec![format!("{}:{}", form.cwd, form.cwd)]);
        assert!(spec.name.is_none());

        // Unchecked, the form is back to pure runtime defaults — and NO
        // explicit workdir (the SDK stats workdirs in the guest rootfs;
        // plain images don't have the sbx template's /home/agent/workspace).
        form.toggle_mount_cwd();
        let spec = form.to_create_spec().unwrap();
        assert_eq!(spec.workdir, None);
        assert!(spec.volumes.is_empty());
    }

    #[test]
    fn image_is_required() {
        let form = CreateForm::new(Vec::new());
        let err = form.to_create_spec().unwrap_err();
        assert!(err.to_string().contains("Image is required"));
    }

    #[test]
    fn suggestions_put_local_images_first() {
        let form = CreateForm::new(vec![img("python:3.12"), img("alpine")]);
        let s = form.suggestions();
        // Local images come first (they exist on this machine).
        assert_eq!(s.first().unwrap(), "python:3.12");
        // Curated suggestions fill the rest; "alpine" is deduped.
        assert!(s.contains(&"alpine".to_string()));
        assert_eq!(s.iter().filter(|i| i.as_str() == "alpine").count(), 1);
        assert!(s.len() <= PICKER_WINDOW_MAX);
    }

    #[test]
    fn suggestions_filter_by_typed_text() {
        let mut form = CreateForm::new(vec![img("python:3.12")]);
        form.image = "py".into();
        // Local images surface first, curated fill afterwards.
        let s = form.suggestions();
        assert_eq!(s, vec!["python:3.12".to_string(), "python".to_string()]);
    }

    #[test]
    fn enter_adopts_highlighted_suggestion() {
        let mut form = CreateForm::new(Vec::new());
        form.picker_selected = 1;
        let action = form.handle_key(key(KeyCode::Enter));
        assert_eq!(action, FormAction::NextField);
        assert_eq!(form.image, SUGGESTED_IMAGES[1]);
        assert_eq!(form.active_field, FormField::Name);
    }

    #[test]
    fn enter_with_typed_custom_image_moves_on() {
        let mut form = CreateForm::new(Vec::new());
        form.image = "my.registry.io/custom:1.0".into();
        let action = form.handle_key(key(KeyCode::Enter));
        assert_eq!(action, FormAction::NextField);
        assert_eq!(form.active_field, FormField::Name);
    }

    #[test]
    fn enter_adopts_first_suggestion_when_buffer_empty() {
        let mut form = CreateForm::new(Vec::new());
        let action = form.handle_key(key(KeyCode::Enter));
        assert_eq!(action, FormAction::NextField);
        assert_eq!(form.image, "alpine");
        assert_eq!(form.active_field, FormField::Name);
    }

    #[test]
    fn enter_on_unmatched_text_sets_hint() {
        let mut form = CreateForm::new(Vec::new());
        form.image = "zzz-no-match".into();
        // No suggestion matches; Enter adopts the typed text instead.
        let action = form.handle_key(key(KeyCode::Enter));
        assert_eq!(action, FormAction::NextField);
        assert_eq!(form.active_field, FormField::Name);
        assert_eq!(form.image, "zzz-no-match");
    }

    #[test]
    fn typing_filters_and_resets_picker_selection() {
        let mut form = CreateForm::new(Vec::new());
        form.picker_selected = 3;
        form.handle_key(key(KeyCode::Char('u')));
        assert_eq!(form.picker_selected, 0);
        assert!(form.image == "u");
    }

    #[test]
    fn picker_moves_within_bounds() {
        let mut form = CreateForm::new(Vec::new());
        let last = form.suggestions().len() - 1;
        for _ in 0..(last + 5) {
            form.handle_key(key(KeyCode::Down));
        }
        assert_eq!(form.picker_selected, last);
        form.handle_key(key(KeyCode::Up));
        assert_eq!(form.picker_selected, last - 1);
    }

    #[test]
    fn quick_tab_cycles_three_fields() {
        let mut form = CreateForm::new(Vec::new());
        form.handle_key(key(KeyCode::Tab));
        assert_eq!(form.active_field, FormField::Name);
        form.handle_key(key(KeyCode::Tab));
        assert_eq!(form.active_field, FormField::MountCwd);
        form.handle_key(key(KeyCode::Tab));
        assert_eq!(form.active_field, FormField::Image);
    }

    #[test]
    fn quick_enter_on_name_moves_to_mount_toggle() {
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        form.active_field = FormField::Name;
        assert_eq!(form.handle_key(key(KeyCode::Enter)), FormAction::NextField);
        assert_eq!(form.active_field, FormField::MountCwd);
        // Enter on the toggle (last quick field) submits.
        assert_eq!(form.handle_key(key(KeyCode::Enter)), FormAction::Submit);
        // Space flips the checkbox without moving.
        assert_eq!(
            form.handle_key(key(KeyCode::Char(' '))),
            FormAction::Continue
        );
        assert!(!form.mount_cwd);
    }

    // -- advanced toggle --

    #[test]
    fn ctrl_a_toggles_advanced() {
        let mut form = CreateForm::new(Vec::new());
        form.handle_key(key_mod(KeyCode::Char('a'), KeyModifiers::CONTROL));
        assert!(form.advanced);
        assert_eq!(form.field_count(), FormField::ALL.len());
        form.handle_key(key_mod(KeyCode::Char('a'), KeyModifiers::CONTROL));
        assert!(!form.advanced);
        assert_eq!(form.active_field, FormField::Image);
    }

    #[test]
    fn typing_a_is_not_advanced_toggle() {
        let mut form = CreateForm::new(Vec::new());
        form.handle_key(key(KeyCode::Char('a')));
        assert!(!form.advanced);
        assert_eq!(form.image, "a");
    }

    #[test]
    fn advanced_values_apply_when_set() {
        let mut form = CreateForm::new(Vec::new());
        form.handle_key(key_mod(KeyCode::Char('a'), KeyModifiers::CONTROL));
        form.image = "alpine".into();
        form.active_field = FormField::Cpus;
        form.cpus = "2".into();
        form.memory = "1G".into();
        form.active_field = FormField::Name;
        form.name = "web".into();
        let spec = form.to_create_spec().unwrap();
        assert_eq!(spec.cpus, Some(2));
        assert_eq!(spec.memory.as_deref(), Some("1G"));
        assert_eq!(spec.name.as_deref(), Some("web"));
        assert_eq!(spec.net_profile.as_deref(), Some("public"));
    }

    #[test]
    fn advanced_invalid_cpus_rejected() {
        let mut form = CreateForm::new(Vec::new());
        form.handle_key(key_mod(KeyCode::Char('a'), KeyModifiers::CONTROL));
        form.image = "alpine".into();
        form.cpus = "abc".into();
        assert!(form.to_create_spec().is_err());
        form.cpus = "0".into();
        assert!(form.to_create_spec().is_err());
    }

    #[test]
    fn advanced_invalid_memory_rejected() {
        let mut form = CreateForm::new(Vec::new());
        form.handle_key(key_mod(KeyCode::Char('a'), KeyModifiers::CONTROL));
        form.image = "alpine".into();
        form.memory = "lots".into();
        assert!(form.to_create_spec().is_err());
    }

    #[test]
    fn advanced_summary_lists_set_fields() {
        let mut form = CreateForm::new(Vec::new());
        // The default state (mount cwd on, nothing else) is not "set" — the
        // summary exists to surface *deliberate* choices only.
        assert_eq!(form.advanced_summary(), None);
        form.cpus = "2".into();
        form.ports.push("8080:80".into());
        let summary = form.advanced_summary().unwrap();
        assert!(summary.contains("2 cpus"));
        assert!(summary.contains("1 ports"));
        form.toggle_mount_cwd();
        assert!(form.advanced_summary().unwrap().contains("no mount"));
    }

    // -- advanced field behavior (unchanged semantics) --

    #[test]
    fn advanced_net_profile_cycles() {
        let mut form = CreateForm::new(Vec::new());
        form.handle_key(key_mod(KeyCode::Char('a'), KeyModifiers::CONTROL));
        form.active_field = FormField::NetProfile;
        form.handle_key(key(KeyCode::Down));
        assert_eq!(form.net_profile, NetProfile::Private);
        form.handle_key(key(KeyCode::Left));
        assert_eq!(form.net_profile, NetProfile::Public);
    }

    #[test]
    fn advanced_list_add_and_remove() {
        let mut form = CreateForm::new(Vec::new());
        form.handle_key(key_mod(KeyCode::Char('a'), KeyModifiers::CONTROL));
        form.active_field = FormField::Ports;
        for ch in "8080:80".chars() {
            form.handle_key(key(KeyCode::Char(ch)));
        }
        form.handle_key(key(KeyCode::Enter)); // commits the list item
        assert_eq!(form.ports, vec!["8080:80".to_string()]);
        form.handle_key(key(KeyCode::Delete));
        assert!(form.ports.is_empty());
    }

    #[test]
    fn advanced_enter_on_last_field_submits() {
        let mut form = CreateForm::new(Vec::new());
        form.handle_key(key_mod(KeyCode::Char('a'), KeyModifiers::CONTROL));
        form.active_field = *FormField::ALL.last().unwrap();
        assert_eq!(form.handle_key(key(KeyCode::Enter)), FormAction::Submit);
    }

    // -- spec conversion helpers --

    #[test]
    fn port_specs_parse() {
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        form.handle_key(key_mod(KeyCode::Char('a'), KeyModifiers::CONTROL));
        form.active_field = FormField::Ports;
        form.ports = vec!["8080:80".into(), "0.0.0.0:9090:90/udp".into()];
        let spec = form.to_create_spec().unwrap();
        assert_eq!(spec.ports.len(), 2);
        assert_eq!(spec.ports[0].host_port, 8080);
        assert_eq!(spec.ports[0].guest_port, 80);
        assert_eq!(spec.ports[0].host_bind, "127.0.0.1");
        assert_eq!(spec.ports[1].protocol, "udp");
        assert_eq!(spec.ports[1].host_bind, "0.0.0.0");
    }

    #[test]
    fn empty_list_entries_are_skipped() {
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        form.handle_key(key_mod(KeyCode::Char('a'), KeyModifiers::CONTROL));
        form.toggle_mount_cwd(); // keep volumes purely what the user typed
        form.volumes = vec!["  ".into(), "./src:/app".into()];
        let spec = form.to_create_spec().unwrap();
        assert_eq!(spec.volumes, vec!["./src:/app".to_string()]);
    }

    #[test]
    fn workdir_and_name_optional() {
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        form.handle_key(key_mod(KeyCode::Char('a'), KeyModifiers::CONTROL));
        form.workdir = "/app".into();
        let spec = form.to_create_spec().unwrap();
        assert_eq!(spec.workdir.as_deref(), Some("/app"));
        assert_eq!(spec.name, None);
    }

    // -- workspace mount (Docker-sbx style) --

    #[test]
    fn mount_cwd_defaults_on_with_captured_cwd() {
        let form = CreateForm::new(Vec::new());
        // sbx parity: the checkbox starts checked, workspace = TUI's CWD.
        assert!(form.mount_cwd);
        assert!(!form.cwd.trim().is_empty());
        assert_eq!(form.workdir, form.cwd);
        assert_eq!(form.cwd, std::env::current_dir().unwrap().to_string_lossy());
    }

    #[test]
    fn mount_cwd_quick_mode_produces_bind_and_workdir() {
        // Quick mode (no advanced fields touched): checked box mounts the
        // current dir at the same absolute path and starts the sandbox there.
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        let spec = form.to_create_spec().unwrap();
        assert_eq!(
            spec.volumes,
            vec![format!("{}:{}", form.cwd, form.cwd)],
            "bind mount at the same absolute path (sbx behavior)"
        );
        assert_eq!(spec.workdir.as_deref(), Some(form.cwd.as_str()));
    }

    #[test]
    fn unchecking_mount_cwd_clears_workdir() {
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        form.toggle_mount_cwd();
        let spec = form.to_create_spec().unwrap();
        // No mount AND no explicit workdir: the SDK stats the workdir inside
        // the guest rootfs (create failed with `stat: No such file` for
        // /home/agent/workspace on plain OCI images), so mountless sandboxes
        // start in the image's own working directory — like `sbx run`.
        assert!(spec.volumes.is_empty());
        assert_eq!(spec.workdir, None);
    }

    #[test]
    fn explicit_workdir_overrides_mount_default() {
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        form.workdir = "/custom".into();
        let spec = form.to_create_spec().unwrap();
        // A deliberately typed workdir wins over the mount-derived one.
        assert_eq!(spec.workdir.as_deref(), Some("/custom"));
        assert_eq!(spec.volumes.len(), 1, "mount still happens");
    }

    #[test]
    fn unchecking_mount_cwd_clears_its_auto_workdir() {
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        form.toggle_mount_cwd();
        // The auto-filled workdir from the checked state is gone (empty =
        // image default, tested above); re-checking restores it.
        form.toggle_mount_cwd();
        let spec = form.to_create_spec().unwrap();
        assert_eq!(spec.workdir.as_deref(), Some(form.cwd.as_str()));
        assert_eq!(spec.volumes.len(), 1);
    }

    #[test]
    fn mount_cwd_rejects_non_absolute_or_missing_dir() {
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        form.cwd = "relative/path".into();
        let err = form.to_create_spec().unwrap_err();
        assert!(err.to_string().contains("absolute"));
        form.cwd = "/nonexistent-tui-cwd-probe-9x".into();
        let err = form.to_create_spec().unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    // -- memory validation --

    #[test]
    fn memory_strings_validate() {
        assert!(is_valid_memory("512M"));
        assert!(is_valid_memory("1G"));
        assert!(is_valid_memory("1073741824"));
        assert!(!is_valid_memory(""));
        assert!(!is_valid_memory("M"));
        assert!(!is_valid_memory("lots"));
    }

    #[test]
    fn port_specs_reject_garbage() {
        assert!(parse_port_spec("8080").is_err());
        assert!(parse_port_spec("a:b").is_err());
        assert!(parse_port_spec("8080:80/sctp").is_err());
        assert!(parse_port_spec("127.0.0.1:8080:80").is_ok());
    }

    #[test]
    fn trim_filter_removes_blanks() {
        let list = vec![" a ".into(), String::new(), "b".into()];
        assert_eq!(trim_filter(&list), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn picker_selection_clamped_to_visible_list() {
        let mut form = CreateForm::new(Vec::new());
        // move_picker clamps through the real suggestion list only.
        form.picker_selected = 100;
        form.move_picker(0);
        assert!(form.picker_selected < form.suggestions().len());
    }
}
