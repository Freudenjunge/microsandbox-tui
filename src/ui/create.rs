//! Create-sandbox form: one full-screen grouped form, entered from the
//! template picker (`e` = pre-filled, `n` = empty).
//!
//! UX (per the templates spec):
//! - All fields are visible in grouped Tab order (BASIS/RESSOURCEN/MOUNTS/
//!   NETZWERK/SONSTIGES); empty resource/list fields mean "use the runtime
//!   default".
//! - `Ctrl+S` saves the current form as a template (name + description
//!   dialog) — the authoring path writes the same TOML the picker reads.
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

    /// Parse the profile from its string form (`public`, `private`, …);
    /// unknown values yield `None` (callers default to `Public`).
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|p| p.as_str() == s)
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

/// Which form field is currently active, in grouped Tab order
/// (BASIS: Image/Name · RESSOURCEN: Cpus/Memory · MOUNTS: MountCwd/Workdir/
/// Volumes · NETZWERK: NetProfile/Ports/NetRules · SONSTIGES: EnvVars/
/// Labels).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormField {
    Image,
    Name,
    Cpus,
    Memory,
    MountCwd,
    Workdir,
    Volumes,
    NetProfile,
    Ports,
    NetRules,
    EnvVars,
    Labels,
}

impl FormField {
    /// All fields in grouped Tab order.
    pub const ALL: [Self; 12] = [
        Self::Image,
        Self::Name,
        Self::Cpus,
        Self::Memory,
        Self::MountCwd,
        Self::Workdir,
        Self::Volumes,
        Self::NetProfile,
        Self::Ports,
        Self::NetRules,
        Self::EnvVars,
        Self::Labels,
    ];

    /// Whether this field is a list-type (items + input buffer).
    pub fn is_list(&self) -> bool {
        matches!(
            self,
            Self::Ports | Self::Volumes | Self::EnvVars | Self::NetRules | Self::Labels
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
            Self::Labels => "Labels",
        }
    }
}

// ---------------------------------------------------------------------------
// FormAction
// ---------------------------------------------------------------------------

/// What the form wants the caller to do after a key press.
#[derive(Debug, Clone, PartialEq)]
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
    /// Ctrl+S confirmed in the save dialog — write this template to disk.
    SaveTemplate(Box<crate::template::Template>),
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
    /// Labels in `KEY=VALUE` form.
    pub labels: Vec<String>,
    /// Network rule strings (e.g. `allow@api.example.com`).
    pub net_rules: Vec<String>,
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
            labels: Vec::new(),
            net_rules: Vec::new(),
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

    /// Index of the active field within the grouped field list.
    pub fn active_field_index(&self) -> usize {
        FormField::ALL
            .iter()
            .position(|&f| f == self.active_field)
            .unwrap_or(0)
    }

    /// Number of fields in grouped Tab order.
    pub fn field_count(&self) -> usize {
        FormField::ALL.len()
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

    /// Advance to the next field (wraps around).
    fn next_field(&mut self) {
        let idx = self.active_field_index();
        let next = (idx + 1) % FormField::ALL.len();
        self.set_active_field(FormField::ALL[next]);
    }

    /// Go back to the previous visible field (wraps around).
    fn prev_field(&mut self) {
        let idx = self.active_field_index();
        let prev = (idx + FormField::ALL.len() - 1) % FormField::ALL.len();
        self.set_active_field(FormField::ALL[prev]);
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
            FormField::Labels => &mut self.labels,
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
            FormField::Labels => &self.labels,
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
            ports.push(PublishedPort::parse_cli(p)?);
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

        // Workspace mount source checks live with the form: the TUI's CWD
        // must be absolute and still exist at submit time (the assembly
        // itself is shared with templates in `template::assemble_spec`).
        if self.mount_cwd {
            let cwd = self.cwd.trim();
            if !cwd.starts_with('/') {
                bail!("Workspace mount: {cwd} is not an absolute path");
            }
            if !std::path::Path::new(cwd).exists() {
                bail!("Workspace mount: {cwd} does not exist (deleted after open?)");
            }
        }

        // Grouped form: the profile is always visible and always applied.
        let net_profile = Some(self.net_profile.as_str().to_string());

        crate::template::assemble_spec(crate::template::SpecInputs {
            image,
            name,
            cpus,
            memory,
            workdir: Some(self.workdir.clone()),
            mount_cwd: self.mount_cwd,
            cwd: self.cwd.trim(),
            ports,
            volumes: self.volumes.clone(),
            env: self.env_vars.clone(),
            labels: crate::template::trim_filter(&self.labels),
            net_rules: self.net_rules.clone(),
            net_profile,
        })
    }

    /// Build a form pre-filled from a template (the `e`-path in the
    /// template picker). Unset template fields fall back to the same
    /// defaults as [`Self::new`].
    pub fn from_template(t: &crate::template::Template, cwd: String) -> Self {
        let s = &t.spec;
        Self {
            image: s.image.clone(),
            name: s.name.clone().unwrap_or_default(),
            cpus: s.cpus.map(|c| c.to_string()).unwrap_or_default(),
            memory: s.memory.clone().unwrap_or_default(),
            workdir: s.workdir.clone().unwrap_or_else(|| cwd.clone()),
            mount_cwd: s.mount_cwd.unwrap_or(true),
            cwd,
            net_profile: s
                .net_profile
                .as_deref()
                .and_then(NetProfile::parse)
                .unwrap_or(NetProfile::Public),
            ports: s.ports.clone(),
            volumes: s.volumes.clone(),
            env_vars: s.env.clone(),
            labels: s.labels_vec(),
            net_rules: s.net_rules.clone(),
            active_field: FormField::Image,
            error: None,
            images: Vec::new(),
            picker_selected: 0,
            list_input: String::new(),
            list_selected: 0,
        }
    }

    /// Convert the current form state into a template (the `Ctrl+S` path).
    /// Field parsing/validation happens again when the template is used.
    pub fn to_template(&self, name: String, description: String) -> crate::template::Template {
        let spec = crate::template::TemplateSpec {
            image: self.image.trim().to_string(),
            name: {
                let n = self.name.trim();
                (!n.is_empty()).then(|| n.to_string())
            },
            cpus: self.cpus.trim().parse::<u32>().ok(),
            memory: (!self.memory.trim().is_empty()).then(|| self.memory.trim().to_string()),
            workdir: (!self.workdir.trim().is_empty()).then(|| self.workdir.trim().to_string()),
            mount_cwd: Some(self.mount_cwd),
            ports: crate::template::trim_filter(&self.ports),
            volumes: crate::template::trim_filter(&self.volumes),
            env: crate::template::trim_filter(&self.env_vars),
            labels: self
                .labels
                .iter()
                .filter_map(|l| {
                    let (k, v) = l.split_once('=')?;
                    Some((k.trim().to_string(), v.trim().to_string()))
                })
                .collect(),
            net_profile: Some(self.net_profile.as_str().to_string()),
            net_rules: crate::template::trim_filter(&self.net_rules),
            rootfs: None,
        };
        crate::template::Template {
            id: crate::template::slugify(&name),
            meta: crate::template::TemplateMeta { name, description },
            spec,
            built_in: false,
        }
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
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " ⚡ CREATE MICROVM — GROUPED ",
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
    if form.active_field == FormField::Name {
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

    // All fields, grouped order (section headers come with the grouped
    // rendering pass).
    lines.extend(advanced_field_lines(form));

    // -- footer --
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "[Tab] next field  [Enter] create  [Esc] cancel",
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
            label: "create",
            role: crate::ui::chrome::FooterRole::Accent,
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

    // -- grouped field model (templates rework) --

    #[test]
    fn field_order_is_grouped() {
        let form = CreateForm::new(Vec::new());
        assert_eq!(form.field_count(), 12);
        assert_eq!(FormField::ALL[0], FormField::Image);
        assert_eq!(FormField::ALL[1], FormField::Name);
        assert_eq!(FormField::ALL[2], FormField::Cpus);
        assert_eq!(FormField::ALL[4], FormField::MountCwd);
        assert_eq!(FormField::ALL[11], FormField::Labels);
    }

    #[test]
    fn net_profile_is_always_applied() {
        // Gruppiertes Formular hat kein Quick/Advanced mehr:
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        let spec = form.to_create_spec().unwrap();
        assert_eq!(spec.net_profile.as_deref(), Some("public"));
    }

    #[test]
    fn form_from_template_prefills_all_fields() {
        let t = crate::template::parse(
            "opencode",
            include_str!("../fixtures/templates/valid-full.toml"),
        )
        .unwrap();
        let form = CreateForm::from_template(&t, "/home/me/proj".into());
        assert_eq!(form.image, "node:22-alpine");
        assert_eq!(form.name, "opencode");
        assert_eq!(form.cpus, "4");
        assert_eq!(form.memory, "2G");
        assert!(form.mount_cwd);
        assert_eq!(form.workdir, "/workspace");
        assert_eq!(form.ports, vec!["127.0.0.1:3000:3000"]);
        assert_eq!(form.env_vars, vec!["TERM=xterm-256color"]);
        assert_eq!(form.labels, vec!["team=infra"]);
        assert_eq!(form.net_profile, NetProfile::Public);
    }

    #[test]
    fn form_to_template_roundtrips_through_file() {
        let t0 = crate::template::parse(
            "opencode",
            include_str!("../fixtures/templates/valid-full.toml"),
        )
        .unwrap();
        let form = CreateForm::from_template(&t0, "/home/me/proj".into());
        let t1 = form.to_template("Opencode".into(), "d".into());
        assert_eq!(t1.spec.image, "node:22-alpine");
        assert_eq!(t1.spec.cpus, Some(4));
        assert_eq!(t1.spec.labels_vec(), vec!["team=infra"]);
    }

    // -- quick mode --

    #[test]
    fn new_starts_grouped_on_image() {
        let form = CreateForm::new(Vec::new());
        assert_eq!(form.active_field, FormField::Image);
        assert_eq!(form.field_count(), 12);
        assert!(form.error.is_none());
    }

    #[test]
    fn defaults_spec_uses_runtime_defaults() {
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        // The sbx-style workspace mount is on by default; cpus/memory stay
        // at runtime defaults; the profile is always applied (grouped form).
        let spec = form.to_create_spec().unwrap();
        assert_eq!(spec.image, "alpine");
        assert_eq!(spec.cpus, None);
        assert_eq!(spec.memory, None);
        assert_eq!(spec.workdir.as_deref(), Some(form.cwd.as_str()));
        assert_eq!(spec.net_profile.as_deref(), Some("public"));
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
    fn tab_cycles_the_grouped_fields() {
        let mut form = CreateForm::new(Vec::new());
        form.handle_key(key(KeyCode::Tab));
        assert_eq!(form.active_field, FormField::Name);
        form.handle_key(key(KeyCode::Tab));
        assert_eq!(form.active_field, FormField::Cpus);
        form.handle_key(key(KeyCode::Tab));
        assert_eq!(form.active_field, FormField::Memory);
    }

    #[test]
    fn enter_on_name_moves_to_cpus() {
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        form.active_field = FormField::Name;
        assert_eq!(form.handle_key(key(KeyCode::Enter)), FormAction::NextField);
        assert_eq!(form.active_field, FormField::Cpus);
        // Space on the mount toggle flips the checkbox without moving.
        form.active_field = FormField::MountCwd;
        assert_eq!(
            form.handle_key(key(KeyCode::Char(' '))),
            FormAction::Continue
        );
        assert!(!form.mount_cwd);
    }

    // -- field values (grouped form; no toggle anymore) --

    #[test]
    fn typing_a_types_into_the_image_field() {
        let mut form = CreateForm::new(Vec::new());
        form.handle_key(key(KeyCode::Char('a')));
        assert_eq!(form.image, "a");
    }

    #[test]
    fn values_apply_when_set() {
        let mut form = CreateForm::new(Vec::new());
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
    fn invalid_cpus_rejected() {
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        form.cpus = "abc".into();
        assert!(form.to_create_spec().is_err());
        form.cpus = "0".into();
        assert!(form.to_create_spec().is_err());
    }

    #[test]
    fn invalid_memory_rejected() {
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        form.memory = "lots".into();
        assert!(form.to_create_spec().is_err());
    }

    // -- field behavior (semantics unchanged from the advanced form) --

    #[test]
    fn net_profile_cycles() {
        let mut form = CreateForm::new(Vec::new());
        form.active_field = FormField::NetProfile;
        form.handle_key(key(KeyCode::Down));
        assert_eq!(form.net_profile, NetProfile::Private);
        form.handle_key(key(KeyCode::Left));
        assert_eq!(form.net_profile, NetProfile::Public);
    }

    #[test]
    fn list_add_and_remove() {
        let mut form = CreateForm::new(Vec::new());
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
    fn enter_on_last_field_submits() {
        let mut form = CreateForm::new(Vec::new());
        form.active_field = *FormField::ALL.last().unwrap();
        assert_eq!(form.handle_key(key(KeyCode::Enter)), FormAction::Submit);
    }

    // -- spec conversion helpers --

    #[test]
    fn port_specs_parse() {
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
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
        form.toggle_mount_cwd(); // keep volumes purely what the user typed
        form.volumes = vec!["  ".into(), "./src:/app".into()];
        let spec = form.to_create_spec().unwrap();
        assert_eq!(spec.volumes, vec!["./src:/app".to_string()]);
    }

    #[test]
    fn workdir_and_name_optional() {
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
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
        assert!(PublishedPort::parse_cli("8080").is_err());
        assert!(PublishedPort::parse_cli("a:b").is_err());
        assert!(PublishedPort::parse_cli("8080:80/sctp").is_err());
        assert!(PublishedPort::parse_cli("127.0.0.1:8080:80").is_ok());
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
