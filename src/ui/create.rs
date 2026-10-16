//! Create-sandbox form: one full-screen grouped form, entered from the
//! template picker (`e` = pre-filled, `n` = empty).
//!
//! UX (per the templates spec):
//! - All fields are visible in grouped Tab order (GENERAL/RESOURCES/MOUNTS/
//!   NETWORK/MISC); empty resource/list fields mean "use the runtime
//!   default".
//! - `Ctrl+S` saves the current form as a template (name + description
//!   dialog) — the authoring path writes the same TOML the picker reads.
//! - The image picker is not a modal: typing filters it, `↑↓` moves the
//!   highlight, `Enter` adopts the highlighted suggestion.
//! - The env-profile/credential fields use the same autocomplete: type to
//!   filter, `↑↓` pick, `Enter` adopts + moves on (never submits).
//! - All edited buffers support insert-at-caret editing (see
//!   `ui::text_input`) — mid-string edits and pastes work.

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
/// Curated image suggestions for the picker's tail (after local images):
/// the sbx Hub harness base images (download order — the same catalog
/// the template picker shows as built-ins; the Hub `*-kit` repos are not
/// bootable, the `<name>-image` bases are), then generic distro/tool
/// defaults. Single source of truth for the display order.
pub const SUGGESTED_IMAGES: &[&str] = &[
    // -- sbx Hub harness catalog (kit pull count, descending) --
    "sbx/pi-image",
    "sbx/hermes-agent-image",
    "sbx/openclaw-image",
    "sbx/kiro-image",
    "sbx/crush-image",
    "sbx/junie-image",
    "sbx/copilot-image",
    "sbx/aider-image",
    "sbx/droid-image",
    "sbx/open-interpreter-image",
    "sbx/openhands-image",
    "sbx/antigravity-image",
    "sbx/vibe-image",
    "sbx/docker-agent-image",
    "sbx/claude-image",
    "sbx/opencode-image",
    "sbx/codex-image",
    "sbx/cursor-image",
    "sbx/devin-image",
    // -- generic defaults --
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
/// (GENERAL: Image/Name · RESOURCES: Cpus/Memory/Disk · MOUNTS: MountCwd/Workdir/
/// Volumes · NETWORK: NetProfile/Ports/NetRules · MISC: Env-Profile/
/// Credentials/EnvVars/Labels).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormField {
    Image,
    Name,
    Shell,
    Cpus,
    Memory,
    RootDisk,
    MountCwd,
    Workdir,
    Volumes,
    NetProfile,
    Ports,
    NetRules,
    EnvProfile,
    Credentials,
    EnvVars,
    Labels,
}

impl FormField {
    /// All fields in grouped Tab order.
    pub const ALL: [Self; 16] = [
        Self::Image,
        Self::Name,
        Self::Shell,
        Self::Cpus,
        Self::Memory,
        Self::RootDisk,
        Self::MountCwd,
        Self::Workdir,
        Self::Volumes,
        Self::NetProfile,
        Self::Ports,
        Self::NetRules,
        Self::EnvProfile,
        Self::Credentials,
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
            Self::Image
                | Self::Name
                | Self::Shell
                | Self::Cpus
                | Self::Memory
                | Self::RootDisk
                | Self::Workdir
        )
    }

    /// Human-readable label.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Image => "Image",
            Self::Name => "Name",
            Self::Shell => "Shell",
            Self::MountCwd => "Mount current dir",
            Self::Cpus => "CPUs",
            Self::Memory => "Memory",
            Self::RootDisk => "Disk",
            Self::Workdir => "Workdir",
            Self::NetProfile => "Profile",
            Self::Ports => "Ports",
            Self::Volumes => "Volumes",
            Self::EnvVars => "Env",
            Self::NetRules => "Rules",
            Self::EnvProfile => "Env-Profile",
            Self::Credentials => "Credentials",
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
    /// CLI shell inside the sandbox (e.g. `/bin/bash`); empty = image
    /// default (`/bin/sh`).
    pub shell: String,
    /// vCPU count as a text string; empty = runtime default.
    pub cpus: String,
    /// Memory limit string (e.g. `512M`); empty = runtime default.
    pub memory: String,
    /// Root disk size string (e.g. `16G`); empty = runtime default (~4G overlay).
    pub root_disk: String,
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
    /// Environment profiles loaded once at form-open time
    /// (`environments.toml`): the names drive the suggest field's
    /// candidates, `env` feeds the merge at submit. Empty on load failure
    /// — the form still opens.
    env_profiles: Vec<crate::environments::Profile>,
    /// Credential names loaded once at form-open time (`credentials.toml`);
    /// only the typed value is recorded here, secret resolution happens in
    /// the create app wiring. Empty on load failure — the form still opens.
    credential_names: Vec<String>,
    /// Typed environment profile filter/value (empty = none). Typing
    /// filters the candidate list, ↑↓ picks, Enter adopts; merged at
    /// submit.
    env_profile: String,
    /// Typed credential value (empty = none); recorded for the app wiring.
    credential: String,
    /// Caret position (char index) inside the ACTIVE field's buffer —
    /// text, list-input and suggest fields share one slot because only
    /// one field is active at a time; reset to the buffer end on every
    /// field switch.
    text_cursor: usize,
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
    /// Open "save as template" dialog (Ctrl+S), if any.
    pub save_dialog: Option<SaveDialog>,
}

/// The Ctrl+S save-as-template dialog state.
#[derive(Debug, Default)]
pub struct SaveDialog {
    /// Template display name (also drives the file slug).
    pub name: String,
    /// One-line description.
    pub description: String,
    /// Which dialog field is active.
    pub field: SaveField,
}

/// Fields of the save dialog, in Tab order.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SaveField {
    #[default]
    Name,
    Description,
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
        let (env_profiles, credential_names) = Self::load_cycle_lists();
        Self {
            image: String::new(),
            name: String::new(),
            shell: String::new(),
            cpus: String::new(),
            memory: String::new(),
            root_disk: String::new(),
            workdir: cwd.clone(),
            mount_cwd: true,
            cwd,
            net_profile: NetProfile::Public,
            ports: Vec::new(),
            volumes: Vec::new(),
            env_vars: Vec::new(),
            labels: Vec::new(),
            net_rules: Vec::new(),
            env_profiles,
            credential_names,
            env_profile: String::new(),
            credential: String::new(),
            text_cursor: 0,
            active_field: FormField::Image,
            error: None,
            images,
            picker_selected: 0,
            list_input: String::new(),
            list_selected: 0,
            save_dialog: None,
        }
    }

    /// The cycle-select option lists, loaded once at form-open time (not
    /// per keystroke). A broken `environments.toml`/`credentials.toml`
    /// leaves the matching list empty instead of blocking the form — that
    /// cycle field just stays `<none>` (missing files load as empty too).
    fn load_cycle_lists() -> (Vec<crate::environments::Profile>, Vec<String>) {
        (
            crate::environments::load_profiles().unwrap_or_default(),
            crate::credentials::load_credentials()
                .map(|f| f.credentials.into_iter().map(|c| c.name).collect())
                .unwrap_or_default(),
        )
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

    /// The selected environment profile name (None = no profile). Read by
    /// the app's create wiring at Submit; the form itself never resolves
    /// credentials or secrets, only the profile env merge.
    pub fn selected_profile(&self) -> Option<&str> {
        let s = self.env_profile.trim();
        (!s.is_empty()).then_some(s)
    }

    /// The selected credential name (None = no credential). Read by the
    /// app's create wiring at Submit — secrets resolve there, not here.
    pub fn selected_credential(&self) -> Option<&str> {
        let s = self.credential.trim();
        (!s.is_empty()).then_some(s)
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

    /// Open the Ctrl+S save-as-template dialog, pre-filled with a name
    /// suggestion derived from the form's image.
    pub fn open_save_dialog(&mut self, name: String, description: String) -> FormAction {
        self.save_dialog = Some(SaveDialog {
            name,
            description,
            field: SaveField::Name,
        });
        FormAction::Continue
    }

    /// The slug the dialog's current name would be written as.
    pub fn save_dialog_slug(&self) -> String {
        self.save_dialog
            .as_ref()
            .map(|d| crate::template::slugify(&d.name))
            .unwrap_or_default()
    }

    /// Keys while the save dialog is open (modal): typing into the active
    /// field, Tab switches fields, Enter emits SaveTemplate, Esc closes.
    fn handle_save_dialog_key(&mut self, key: KeyEvent) -> FormAction {
        let Some(dialog) = &mut self.save_dialog else {
            return FormAction::Continue;
        };
        match key.code {
            KeyCode::Esc => {
                self.save_dialog = None;
                FormAction::Continue
            }
            KeyCode::Tab => {
                dialog.field = match dialog.field {
                    SaveField::Name => SaveField::Description,
                    SaveField::Description => SaveField::Name,
                };
                FormAction::Continue
            }
            KeyCode::Enter => {
                let name = dialog.name.trim().to_string();
                if name.is_empty() {
                    return FormAction::Continue;
                }
                let description = dialog.description.trim().to_string();
                let template = self.to_template(name, description);
                // I1: validate BEFORE writing — an invalid template would
                // vanish at the next startup (load-time validation skips
                // the whole file).
                if let Err(e) = crate::template::validate_spec(&template.spec) {
                    self.error = Some(format!("Template: {e}"));
                    return FormAction::Continue;
                }
                self.save_dialog = None;
                FormAction::SaveTemplate(Box::new(template))
            }
            KeyCode::Backspace => {
                match dialog.field {
                    SaveField::Name => {
                        dialog.name.pop();
                    }
                    SaveField::Description => {
                        dialog.description.pop();
                    }
                }
                FormAction::Continue
            }
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                match dialog.field {
                    SaveField::Name => dialog.name.push(c),
                    SaveField::Description => dialog.description.push(c),
                }
                FormAction::Continue
            }
            _ => FormAction::Continue,
        }
    }

    /// Process a key and return what the caller should do.
    pub fn handle_key(&mut self, key: KeyEvent) -> FormAction {
        // The save dialog is modal while open.
        if self.save_dialog.is_some() {
            return self.handle_save_dialog_key(key);
        }
        // Ctrl+Enter: explicit create from anywhere (esp. list fields,
        // where plain Enter commits items / navigates).
        if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::CONTROL) {
            return FormAction::Submit;
        }
        // Ctrl+S: open the save-as-template dialog with a name suggestion
        // derived from the image reference.
        if key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL) {
            let suggested = {
                let base = self.image.split('/').next_back().unwrap_or("");
                let base = base.split(':').next().unwrap_or(base);
                let mut chars = base.chars();
                match chars.next() {
                    Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            };
            return self.open_save_dialog(suggested, String::new());
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
        } else if self.active_field == FormField::EnvProfile
            || self.active_field == FormField::Credentials
        {
            self.handle_suggest_key(key)
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
        // Caret at the end of whatever buffer became active (list fields
        // freshly cleared → 0).
        self.text_cursor = match self.active_field {
            FormField::EnvProfile => crate::ui::text_input::caret_end(&self.env_profile),
            FormField::Credentials => crate::ui::text_input::caret_end(&self.credential),
            _ => {
                if self.active_field.is_list() {
                    crate::ui::text_input::caret_end(&self.list_input)
                } else {
                    crate::ui::text_input::caret_end(self.active_text_buf())
                }
            }
        };
    }

    /// Whether the active field is the last in Tab order (its Enter
    /// submits — the keyboard-only create path).
    fn is_last_field(&self) -> bool {
        self.active_field_index() + 1 >= self.field_count()
    }

    // -- text field handling --

    /// Handle a key on a text input field. Cursor editing (insert at
    /// caret, ←/→/Home/End/Delete) comes first; the remaining keys follow
    /// the form grammar (Enter submits, ↑↓ only on the image picker).
    fn handle_text_key(&mut self, key: KeyEvent) -> FormAction {
        let (buf, cursor) = self.active_text_parts();
        if crate::ui::text_input::text_edit(buf, cursor, &key) {
            self.picker_selected = 0;
            return FormAction::Continue;
        }
        match key.code {
            KeyCode::Up | KeyCode::Down if self.active_field == FormField::Image => {
                self.move_picker(if key.code == KeyCode::Up { -1 } else { 1 });
                FormAction::Continue
            }
            KeyCode::Enter if self.active_field == FormField::Image => self.enter_on_image(),
            // Enter = next field everywhere in the form's text fields — a
            // bare Enter on a mid-form field must NOT fire a create (the
            // create key is Ctrl+Enter, or Enter on the last field).
            KeyCode::Enter => {
                if self.is_last_field() {
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

    /// Move the picker highlight by `delta`, clamped to `len`.
    fn move_picker_in(&mut self, len: usize, delta: isize) {
        if len == 0 {
            return;
        }
        let selected = self.picker_selected as isize + delta;
        self.picker_selected = selected.clamp(0, len as isize - 1) as usize;
    }

    /// Move the image picker highlight (candidates = current suggestions).
    fn move_picker(&mut self, delta: isize) {
        let len = self.suggestions().len();
        self.move_picker_in(len, delta);
    }

    /// Mutable reference to the string buffer of the active text field.
    fn active_text_buf(&mut self) -> &mut String {
        match self.active_field {
            FormField::Image => &mut self.image,
            FormField::Name => &mut self.name,
            FormField::Shell => &mut self.shell,
            FormField::Cpus => &mut self.cpus,
            FormField::Memory => &mut self.memory,
            FormField::RootDisk => &mut self.root_disk,
            FormField::Workdir => &mut self.workdir,
            _ => unreachable!("active_text_buf on non-text field"),
        }
    }

    /// The active text buffer plus its caret — one `&mut self` borrow,
    /// disjoint direct field refs (the borrow checker allows field-level
    /// disjointness, not two full-`self` borrows).
    fn active_text_parts(&mut self) -> (&mut String, &mut usize) {
        let cursor = &mut self.text_cursor;
        let buf = match self.active_field {
            FormField::Image => &mut self.image,
            FormField::Name => &mut self.name,
            FormField::Shell => &mut self.shell,
            FormField::Cpus => &mut self.cpus,
            FormField::Memory => &mut self.memory,
            FormField::RootDisk => &mut self.root_disk,
            FormField::Workdir => &mut self.workdir,
            _ => unreachable!("active_text_parts on non-text field"),
        };
        (buf, cursor)
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
                if self.is_last_field() {
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
                if self.is_last_field() {
                    FormAction::Submit
                } else {
                    self.next_field();
                    FormAction::NextField
                }
            }
            _ => FormAction::Continue,
        }
    }

    // -- suggest fields (env profile, credentials): typed autocomplete --

    /// The candidate names of the active suggest field.
    fn candidate_names(&self) -> Vec<String> {
        match self.active_field {
            FormField::EnvProfile => self.env_profiles.iter().map(|p| p.name.clone()).collect(),
            FormField::Credentials => self.credential_names.clone(),
            _ => Vec::new(),
        }
    }

    /// The typed value of the active suggest field.
    fn typed(&self) -> &str {
        match self.active_field {
            FormField::EnvProfile => &self.env_profile,
            FormField::Credentials => &self.credential,
            _ => "",
        }
    }

    /// Candidates of the active suggest field, filtered by the typed text
    /// (image-picker rules: case-insensitive substring, window-capped).
    fn suggest_candidates(&self) -> Vec<String> {
        let names = self.candidate_names();
        let filter = self.typed().trim().to_lowercase();
        if filter.is_empty() {
            let mut all = names;
            all.truncate(PICKER_WINDOW_MAX);
            return all;
        }
        names
            .into_iter()
            .filter(|n| n.to_lowercase().contains(&filter))
            .take(PICKER_WINDOW_MAX)
            .collect()
    }

    /// Keys on an env-profile / credential suggest field: type to filter,
    /// ↑↓ pick, Enter adopts the highlighted candidate (or the exact typed
    /// name) and moves on — the Image field's autocomplete pattern. Enter
    /// never submits here so browsing the lists can't fire a create.
    fn handle_suggest_key(&mut self, key: KeyEvent) -> FormAction {
        let (buf, cursor) = match self.active_field {
            FormField::EnvProfile => (&mut self.env_profile, &mut self.text_cursor),
            FormField::Credentials => (&mut self.credential, &mut self.text_cursor),
            _ => unreachable!("suggest key on a non-suggest field"),
        };
        if crate::ui::text_input::text_edit(buf, cursor, &key) {
            self.picker_selected = 0;
            return FormAction::Continue;
        }
        match key.code {
            KeyCode::Up | KeyCode::Down => {
                let len = self.suggest_candidates().len();
                self.move_picker_in(len, if key.code == KeyCode::Up { -1 } else { 1 });
                FormAction::Continue
            }
            KeyCode::Enter => self.adopt_suggestion(),
            _ => FormAction::Continue,
        }
    }

    /// Enter on a suggest field. Adoption rule (documented in the hint):
    /// - typed text: exact candidate passes as-is; a partial match adopts
    ///   the highlighted candidate; no match rejects inline.
    /// - empty text: `<none>` and move on — UNLESS the highlight was moved
    ///   without typing (browse mode): then the highlighted candidate is
    ///   adopted. So clearing = Enter on the untouched field; picking
    ///   without typing = one arrow press first.
    fn adopt_suggestion(&mut self) -> FormAction {
        let typed = self.typed().trim().to_string();
        let browse_pick = typed.is_empty() && self.picker_selected > 0;
        if typed.is_empty() && self.picker_selected == 0 {
            self.next_field();
            return FormAction::NextField;
        }
        let exact = self.candidate_names().contains(&typed);
        let candidates = self.suggest_candidates();
        if !exact && candidates.is_empty() {
            self.error = Some(match self.active_field {
                FormField::EnvProfile => {
                    format!(
                        "unknown environment profile '{typed}' — pick a suggestion or clear the field"
                    )
                }
                _ => format!("unknown credential '{typed}' — pick a suggestion or clear the field"),
            });
            return FormAction::Continue;
        }
        // Adopt the highlighted candidate when the typed text is not
        // already an exact match — in browse mode that is the picked row,
        // in filter mode the narrowed match.
        if !exact
            && let Some(pick) = candidates.get(if browse_pick {
                self.picker_selected.min(candidates.len().saturating_sub(1))
            } else {
                self.picker_selected
            })
        {
            match self.active_field {
                FormField::EnvProfile => self.env_profile = pick.clone(),
                FormField::Credentials => self.credential = pick.clone(),
                _ => unreachable!("adopt on a non-suggest field"),
            }
        }
        self.next_field();
        FormAction::NextField
    }

    // -- list field handling --

    /// Handle a key on a list-type field (ports, volumes, env, rules).
    /// The input row edits AT THE CARET via the shared text editing; with
    /// an empty input, Delete keeps the row-removal semantics (below).
    fn handle_list_key(&mut self, key: KeyEvent) -> FormAction {
        // Row ops first: `+`/`-` on an empty input add/remove rows (the
        // shared text editing would otherwise type them into the input).
        if self.list_input.is_empty() {
            match key.code {
                KeyCode::Char('+') => {
                    self.active_list_mut().push(String::new());
                    self.list_selected = self.active_list_ref().len() - 1;
                    return FormAction::Continue;
                }
                KeyCode::Char('-') => {
                    self.remove_selected_item();
                    return FormAction::Continue;
                }
                _ => {}
            }
        }
        // Delete on an EMPTY input keeps its "remove the selected row"
        // semantics — text_edit would consume it as a no-op.
        let row_delete = self.list_input.is_empty() && key.code == KeyCode::Delete;
        let consumed = !row_delete && {
            let cursor = &mut self.text_cursor;
            let input = &mut self.list_input;
            crate::ui::text_input::text_edit(input, cursor, &key)
        };
        if consumed {
            return FormAction::Continue;
        }
        match key.code {
            KeyCode::Delete => {
                self.remove_selected_item();
                FormAction::Continue
            }
            KeyCode::Enter => {
                if !self.list_input.is_empty() {
                    let item = std::mem::take(&mut self.list_input);
                    self.active_list_mut().push(item);
                    self.list_selected = self.active_list_ref().len() - 1;
                    FormAction::Continue
                } else if self.active_field_index() + 1 >= self.field_count() {
                    // Last field: the keyboard-only create path.
                    FormAction::Submit
                } else {
                    // Mid-form: empty Enter navigates (Ctrl+Enter creates).
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

        // Root disk — empty means runtime default (~4G managed overlay).
        let root_disk = match self.root_disk.trim() {
            "" => None,
            s if is_valid_memory(s) => Some(s.to_string()),
            _ => bail!("Disk must be like '4G' or '16G'"),
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

        // A typed env profile merges under the form's env vars (profile
        // vars first, form second — the SDK applies last-wins; same order
        // as the template path). Without a value the env list is passed
        // through unchanged.
        let env = match self.selected_profile() {
            Some(name) => crate::environments::merge_env(
                &crate::environments::profile_env(&self.env_profiles, name)?,
                &self.env_vars,
            ),
            None => self.env_vars.clone(),
        };

        // Same validation as profiles: a typed credential the config file
        // doesn't know must not silently become "inject nothing" — reject
        // at submit with the picked name.
        if let Some(cred) = self.selected_credential()
            && !self.credential_names.iter().any(|n| n == cred)
        {
            bail!("unknown credential '{cred}' — pick one from the suggestions or clear the field");
        }

        crate::template::assemble_spec(crate::template::SpecInputs {
            image,
            name,
            shell: (!self.shell.trim().is_empty()).then(|| self.shell.trim().to_string()),
            cpus,
            memory,
            root_disk,
            workdir: Some(self.workdir.clone()),
            mount_cwd: self.mount_cwd,
            cwd: self.cwd.trim(),
            ports,
            volumes: self.volumes.clone(),
            env,
            labels: crate::template::trim_filter(&self.labels),
            net_rules: self.net_rules.clone(),
            net_profile,
        })
    }

    /// Build a form pre-filled from a template (the `e`-path in the
    /// template picker). Unset template fields fall back to the same
    /// defaults as [`Self::new`]; the template's `env_profile` is
    /// pre-typed in the environment suggest field.
    pub fn from_template(t: &crate::template::Template, cwd: String) -> Self {
        let s = &t.spec;
        let mount_cwd = s.mount_cwd.unwrap_or(true);
        let (env_profiles, credential_names) = Self::load_cycle_lists();
        Self {
            image: s.image.clone(),
            name: s.name.clone().unwrap_or_default(),
            shell: s.shell.clone().unwrap_or_default(),
            cpus: s.cpus.map(|c| c.to_string()).unwrap_or_default(),
            memory: s.memory.clone().unwrap_or_default(),
            root_disk: s.root_disk.clone().unwrap_or_default(),
            // Mount coherence (same rule as toggle_mount_cwd): with the
            // mount off there is no host CWD to prefill — the SDK stats
            // explicit workdirs inside the guest rootfs.
            workdir: s.workdir.clone().unwrap_or_else(|| {
                if mount_cwd {
                    cwd.clone()
                } else {
                    String::new()
                }
            }),
            mount_cwd,
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
            env_profiles,
            credential_names,
            env_profile: s.env_profile.clone().unwrap_or_default(),
            credential: String::new(),
            text_cursor: 0,
            active_field: FormField::Image,
            error: None,
            images: Vec::new(),
            picker_selected: 0,
            list_input: String::new(),
            list_selected: 0,
            save_dialog: None,
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
            shell: (!self.shell.trim().is_empty()).then(|| self.shell.trim().to_string()),
            cpus: self.cpus.trim().parse::<u32>().ok(),
            memory: (!self.memory.trim().is_empty()).then(|| self.memory.trim().to_string()),
            env_profile: (!self.env_profile.trim().is_empty())
                .then(|| self.env_profile.trim().to_string()),
            root_disk: (!self.root_disk.trim().is_empty())
                .then(|| self.root_disk.trim().to_string()),
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

/// Cycle a `<none>`-first select through `names` (wrap-around): from
/// `<none>` forward to [0] / backward to [len-1], from [i] to [i±1], and
/// past either end back to `<none>`. With no names the selection stays
/// `<none>` (the cycle field is inert).
fn cycle_none_selection<'a>(
    names: impl Iterator<Item = &'a str>,
    selected: Option<&str>,
    forward: bool,
) -> Option<String> {
    let names: Vec<&str> = names.collect();
    if names.is_empty() {
        return None;
    }
    let pos = selected.and_then(|s| names.iter().position(|&n| n == s));
    match (pos, forward) {
        (None, true) => Some(names[0].to_string()),
        (None, false) => Some(names[names.len() - 1].to_string()),
        (Some(i), true) if i + 1 < names.len() => Some(names[i + 1].to_string()),
        (Some(i), false) if i > 0 => Some(names[i - 1].to_string()),
        // Fell off either end of the name list: back to `<none>`.
        (Some(_), _) => None,
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

    // All fields, grouped into the five spec sections.
    lines.extend(grouped_field_lines(form));

    // -- footer --
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "[Tab] next field  [Enter] next field (last field / [Ctrl+Enter]: create)  [Esc] cancel",
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

    // The Ctrl+S dialog overlays the form while open.
    if form.save_dialog.is_some() {
        render_save_dialog(frame, inner, form);
    }

    // Chrome footer over the legacy hint line.
    let hints = vec![
        crate::ui::chrome::FooterHint {
            key: "[Tab]",
            label: "next field",
            role: crate::ui::chrome::FooterRole::Plain,
        },
        crate::ui::chrome::FooterHint {
            key: "[^s]",
            label: "template",
            role: crate::ui::chrome::FooterRole::Plain,
        },
        crate::ui::chrome::FooterHint {
            key: "[Enter]",
            label: "next field",
            role: crate::ui::chrome::FooterRole::Plain,
        },
        crate::ui::chrome::FooterHint {
            key: "[Ctrl+↵]",
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
fn grouped_field_lines(form: &CreateForm) -> Vec<Line<'static>> {
    let t = &THEME;
    let mut lines = Vec::new();

    // -- GENERAL --
    lines.push(section_header("GENERAL"));
    lines.push(text_field_caret_line(
        "Image",
        &form.image,
        form.text_cursor,
        form.active_field == FormField::Image,
    ));
    lines.extend(picker_lines(form));
    lines.push(text_field_caret_line(
        "Name",
        &form.name,
        form.text_cursor,
        form.active_field == FormField::Name,
    ));
    if form.active_field == FormField::Name {
        lines.push(Line::from(Span::styled(
            "  (auto-generated if empty)",
            Style::default().fg(t.muted),
        )));
    }
    lines.push(text_field_caret_line(
        "Shell",
        &form.shell,
        form.text_cursor,
        form.active_field == FormField::Shell,
    ));
    if form.active_field == FormField::Shell {
        lines.push(Line::from(Span::styled(
            "  (empty = image default /bin/sh; e.g. /bin/bash for a native CLI)",
            Style::default().fg(t.muted),
        )));
    }

    // -- RESOURCES --
    lines.push(Line::raw(""));
    lines.push(section_header("RESOURCES"));
    // CPUs + Memory on one line.
    let cpus_active = form.active_field == FormField::Cpus;
    let mem_active = form.active_field == FormField::Memory;
    let mut resource_row = vec![field_label_span("CPUs", cpus_active), Span::raw(" ")];
    resource_row.extend(text_value_caret_span(
        &form.cpus,
        form.text_cursor,
        cpus_active,
    ));
    resource_row.push(Span::raw("   "));
    resource_row.push(field_label_span("Memory", mem_active));
    resource_row.push(Span::raw(" "));
    resource_row.extend(text_value_caret_span(
        &form.memory,
        form.text_cursor,
        mem_active,
    ));
    lines.push(Line::from(resource_row));
    // Disk on its own line (root disk size; empty = ~4G managed default).
    let disk_active = form.active_field == FormField::RootDisk;
    let mut disk_row = vec![field_label_span("Disk", disk_active), Span::raw(" ")];
    disk_row.extend(text_value_caret_span(
        &form.root_disk,
        form.text_cursor,
        disk_active,
    ));
    lines.push(Line::from(disk_row));

    // -- MOUNTS --
    lines.push(Line::raw(""));
    lines.push(section_header("MOUNTS"));
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
    lines.push(text_field_caret_line(
        "Workdir",
        &form.workdir,
        form.text_cursor,
        form.active_field == FormField::Workdir,
    ));
    lines.extend(list_field_lines(
        "Volumes",
        &form.volumes,
        &form.list_input,
        form.text_cursor,
        form.active_field == FormField::Volumes,
        form.list_selected,
    ));

    // -- NETWORK --
    lines.push(Line::raw(""));
    lines.push(section_header("NETWORK"));
    let profile_active = form.active_field == FormField::NetProfile;
    let mut profile_spans = vec![Span::raw("  Profile: ")];
    for p in NetProfile::ALL {
        let is_selected = form.net_profile == p;
        let marker = if is_selected { "(•)" } else { "( )" };
        let style = if is_selected && profile_active {
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
        } else if is_selected {
            Style::default().fg(t.accent)
        } else {
            Style::default().fg(t.muted)
        };
        profile_spans.push(Span::styled(format!("{marker} {}  ", p.as_str()), style));
    }
    lines.push(Line::from(profile_spans));
    lines.extend(list_field_lines(
        "Ports",
        &form.ports,
        &form.list_input,
        form.text_cursor,
        form.active_field == FormField::Ports,
        form.list_selected,
    ));
    lines.extend(list_field_lines(
        "Rules",
        &form.net_rules,
        &form.list_input,
        form.text_cursor,
        form.active_field == FormField::NetRules,
        form.list_selected,
    ));

    // -- MISC --
    lines.push(Line::raw(""));
    lines.push(section_header("MISC"));
    // Suggest fields (file-backed): typing filters the candidate list, ↑↓
    // picks, Enter adopts and moves on; the suggestion rows render below
    // the field while it is active. Candidates live in the form state
    // (loaded once at open).
    let profile_active = form.active_field == FormField::EnvProfile;
    lines.push(text_field_caret_line(
        "Env-Profile",
        if !profile_active && form.env_profile.is_empty() {
            "<none>"
        } else {
            &form.env_profile
        },
        form.text_cursor,
        profile_active,
    ));
    if profile_active {
        lines.push(Line::from(Span::styled(
            "  (type to filter; ↑↓ pick; Enter adopts — empty = no profile)",
            Style::default().fg(t.muted),
        )));
        lines.extend(suggestion_rows(
            &form.suggest_candidates(),
            form.picker_selected,
        ));
    }
    let cred_active = form.active_field == FormField::Credentials;
    lines.push(text_field_caret_line(
        "Credentials",
        if !cred_active && form.credential.is_empty() {
            "<none>"
        } else {
            &form.credential
        },
        form.text_cursor,
        cred_active,
    ));
    if cred_active {
        lines.push(Line::from(Span::styled(
            "  (type to filter; ↑↓ pick; Enter adopts — empty = none)",
            Style::default().fg(t.muted),
        )));
        lines.extend(suggestion_rows(
            &form.suggest_candidates(),
            form.picker_selected,
        ));
    }
    lines.extend(list_field_lines(
        "Env",
        &form.env_vars,
        &form.list_input,
        form.text_cursor,
        form.active_field == FormField::EnvVars,
        form.list_selected,
    ));
    lines.extend(list_field_lines(
        "Labels",
        &form.labels,
        &form.list_input,
        form.text_cursor,
        form.active_field == FormField::Labels,
        form.list_selected,
    ));

    lines
}

/// The Ctrl+S save-as-template dialog: a small centered overlay with a
/// name and description field. Enter emits
/// [`FormAction::SaveTemplate`], Esc closes it.
fn render_save_dialog(frame: &mut Frame, parent: Rect, form: &CreateForm) {
    let t = &THEME;
    let Some(dialog) = &form.save_dialog else {
        return;
    };
    let width = parent.width.clamp(30, 64);
    let height = 7.min(parent.height);
    let x = parent.x + (parent.width.saturating_sub(width)) / 2;
    let y = parent.y + (parent.height.saturating_sub(height)) / 2;
    let area = Rect {
        x,
        y,
        width,
        height,
    };

    frame.render_widget(ratatui::widgets::Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " Save as template ",
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(t.accent))
        .style(Style::default().bg(t.panel));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = vec![
        text_field_line("Name", &dialog.name, dialog.field == SaveField::Name),
        text_field_line(
            "Descr.",
            &dialog.description,
            dialog.field == SaveField::Description,
        ),
        Line::raw(""),
        Line::from(Span::styled(
            "[Tab] field  [Enter] save  [Esc] cancel",
            Style::default().fg(t.muted),
        )),
    ];
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

/// Build a labeled text-field line with an IN-STRING caret (used wherever
/// the buffer is edited with cursor mechanics): `  Label:  [ab│cd]`.
fn text_field_caret_line(label: &str, value: &str, cursor: usize, active: bool) -> Line<'static> {
    let mut spans = vec![field_label_span(label, active), Span::raw(" ")];
    spans.extend(text_value_caret_span(value, cursor, active));
    Line::from(spans)
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

/// Styled value span with the edit point visible mid-string: ONLY the
/// caret char renders reversed; the surrounding text stays normal.
fn text_value_caret_span(value: &str, cursor: usize, active: bool) -> Vec<Span<'static>> {
    let t = &THEME;
    if !active {
        return vec![Span::styled(
            format!("[{value}]"),
            Style::default().fg(t.muted),
        )];
    }
    let (before, caret, after) = crate::ui::text_input::split_caret(value, cursor);
    let mut spans = vec![
        Span::styled("[", Style::default().fg(t.fg)),
        Span::styled(before, Style::default().fg(t.fg)),
    ];
    match caret {
        Some(c) => spans.push(Span::styled(
            c.to_string(),
            Style::default().fg(t.fg).add_modifier(Modifier::REVERSED),
        )),
        None => spans.push(Span::styled("█", Style::default().fg(t.fg))),
    }
    if !after.is_empty() {
        spans.push(Span::styled(after, Style::default().fg(t.fg)));
    }
    spans.push(Span::styled("]", Style::default().fg(t.fg)));
    spans
}

/// Suggestion rows under a suggest field (image-picker `└` row style).
fn suggestion_rows(items: &[String], selected: usize) -> Vec<Line<'static>> {
    let t = &THEME;
    items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let is_sel = i == selected;
            let style = if is_sel {
                Style::default().fg(t.fg).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(t.muted)
            };
            Line::from(vec![Span::raw("  └ "), Span::styled(item.clone(), style)])
        })
        .collect()
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
    input_cursor: usize,
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

    // Input line (only when active): in-string caret (list inputs edit at
    // the caret like every text field).
    if active {
        let (before, caret, after) = crate::ui::text_input::split_caret(input, input_cursor);
        let mut value = vec![
            Span::styled("  [", Style::default().fg(t.warn)),
            Span::styled(before, Style::default().fg(t.warn)),
        ];
        match caret {
            Some(c) => value.push(Span::styled(
                c.to_string(),
                Style::default().fg(t.warn).add_modifier(Modifier::REVERSED),
            )),
            None => value.push(Span::styled("█", Style::default().fg(t.warn))),
        }
        if !after.is_empty() {
            value.push(Span::styled(after, Style::default().fg(t.warn)));
        }
        value.push(Span::styled("]", Style::default().fg(t.warn)));
        lines.push(Line::from(
            vec![Span::raw("    ")]
                .into_iter()
                .chain(value)
                .collect::<Vec<_>>(),
        ));
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

    fn key_mod_ctrl_enter() -> KeyEvent {
        KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL)
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
        assert_eq!(form.field_count(), 16);
        assert_eq!(FormField::ALL[0], FormField::Image);
        assert_eq!(FormField::ALL[1], FormField::Name);
        assert_eq!(FormField::ALL[2], FormField::Shell);
        assert_eq!(FormField::ALL[3], FormField::Cpus);
        assert_eq!(FormField::ALL[6], FormField::MountCwd);
        assert_eq!(FormField::ALL[12], FormField::EnvProfile);
        assert_eq!(FormField::ALL[13], FormField::Credentials);
        assert_eq!(FormField::ALL[15], FormField::Labels);
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
        assert_eq!(form.root_disk, "16G");
        assert_eq!(form.shell, "/bin/bash");
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
        assert_eq!(t1.spec.shell.as_deref(), Some("/bin/bash"));
        assert_eq!(t1.spec.root_disk.as_deref(), Some("16G"));
        assert_eq!(t1.spec.labels_vec(), vec!["team=infra"]);
    }

    #[test]
    fn save_dialog_validates_before_emit() {
        // I1: an invalid template must NOT be written — it would vanish at
        // the next startup (load-time validation skips the whole file).
        let mut form = CreateForm::new(Vec::new());
        form.image = String::new(); // invalid: image required
        form.open_save_dialog("Test".into(), "d".into());
        let action = form.handle_key(key(KeyCode::Enter));
        assert_eq!(action, FormAction::Continue, "no SaveTemplate emitted");
        assert!(form.error.as_deref().unwrap_or_default().contains("image"));
        assert!(form.save_dialog.is_some(), "dialog stays open");
    }

    #[test]
    fn save_dialog_rejects_bad_memory() {
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        form.memory = "lots".into();
        form.open_save_dialog("Test".into(), "d".into());
        let action = form.handle_key(key(KeyCode::Enter));
        assert_eq!(action, FormAction::Continue);
        assert!(form.error.as_deref().unwrap_or_default().contains("memory"));
    }

    #[test]
    fn from_template_keeps_mount_off_coherent() {
        // I2: mount_cwd=false without workdir → NO workdir prefill (the
        // SDK would stat the workdir inside the guest rootfs and fail).
        let t = crate::template::parse(
            "nomount",
            "[meta]\nname = \"NoMount\"\ndescription = \"d\"\n\n[spec]\nimage = \"alpine\"\nmount_cwd = false\n",
        )
        .unwrap();
        let form = CreateForm::from_template(&t, "/home/me/proj".into());
        assert!(!form.mount_cwd);
        assert_eq!(form.workdir, "", "no host CWD prefill without the mount");
    }

    #[test]
    fn enter_on_name_advances_not_submits() {
        // Grammar (user feedback: bare Enter mid-form fired a create and
        // "ejected" them from the form): Enter walks the fields everywhere;
        // the create is Ctrl+Enter — or Enter on the last field.
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        form.active_field = FormField::Name;
        assert_eq!(form.handle_key(key(KeyCode::Enter)), FormAction::NextField);
        assert_eq!(form.active_field, FormField::Shell);
    }

    #[test]
    fn enter_last_field_submits_from_every_field_kind() {
        // The keyboard-only path stays: empty Enter on the last field of
        // the grouped form (Labels) submits from list fields too.
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        form.active_field = FormField::EnvVars;
        assert_eq!(form.handle_key(key(KeyCode::Enter)), FormAction::NextField);
        assert_eq!(form.active_field, FormField::Labels);
        assert_eq!(form.handle_key(key(KeyCode::Enter)), FormAction::Submit);
    }

    #[test]
    fn enter_on_mount_toggle_advances() {
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        form.active_field = FormField::MountCwd;
        assert_eq!(form.handle_key(key(KeyCode::Enter)), FormAction::NextField);
        assert_eq!(form.active_field, FormField::Workdir);
    }

    #[test]
    fn enter_on_net_profile_advances() {
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        form.active_field = FormField::NetProfile;
        assert_eq!(form.handle_key(key(KeyCode::Enter)), FormAction::NextField);
        assert_eq!(form.active_field, FormField::Ports);
    }

    #[test]
    fn enter_on_list_field_with_empty_input_advances() {
        // A-Followup (user feedback): in the Ports field an empty Enter must
        // NOT submit the sandbox — it advances (like Tab); otherwise
        // "commit one port, type another" would create prematurely.
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        form.active_field = FormField::Ports;
        assert_eq!(form.handle_key(key(KeyCode::Enter)), FormAction::NextField);
        assert_eq!(form.active_field, FormField::NetRules);
    }

    #[test]
    fn enter_on_last_list_field_with_empty_input_submits() {
        // Labels ist das letzte Feld — hier bleibt der Tastatur-Create-Pfad.
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        form.active_field = FormField::Labels;
        assert_eq!(form.handle_key(key(KeyCode::Enter)), FormAction::Submit);
    }

    #[test]
    fn ctrl_enter_submits_from_anywhere() {
        // Explizites Create aus Listenfeldern heraus.
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        form.active_field = FormField::Ports;
        assert_eq!(form.handle_key(key_mod_ctrl_enter()), FormAction::Submit);
    }

    #[test]
    fn enter_commits_typed_list_item_and_stays() {
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        form.active_field = FormField::Ports;
        for ch in "8080:80".chars() {
            form.handle_key(key(KeyCode::Char(ch)));
        }
        assert_eq!(form.handle_key(key(KeyCode::Enter)), FormAction::Continue);
        assert_eq!(form.ports, vec!["8080:80".to_string()]);
        // Zweites Enter (Input jetzt leer) → NICHT erstellen, weitergehen.
        assert_eq!(form.handle_key(key(KeyCode::Enter)), FormAction::NextField);
    }

    #[test]
    fn enter_on_image_adopts_and_moves_to_name() {
        // Submit exception: the picker adopts the selection.
        let mut form = CreateForm::new(Vec::new());
        assert_eq!(form.handle_key(key(KeyCode::Enter)), FormAction::NextField);
        assert_eq!(form.active_field, FormField::Name);
        assert_eq!(form.image, crate::ui::create::SUGGESTED_IMAGES[0]);
    }

    // -- quick mode --

    #[test]
    fn new_starts_grouped_on_image() {
        let form = CreateForm::new(Vec::new());
        assert_eq!(form.active_field, FormField::Image);
        assert_eq!(form.field_count(), 16);
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
    fn suggestions_lead_with_sbx_catalog_then_distros() {
        // Phase 2.13: the curated tail is the sbx Hub harness catalog
        // (download order, the same list the template picker shows),
        // then the generic distro/tool defaults.
        let form = CreateForm::new(Vec::new());
        let s = form.suggestions();
        let sbx: Vec<&String> = s.iter().take_while(|r| r.starts_with("sbx/")).collect();
        assert!(
            sbx.len() >= 19,
            "the 19-entry harness catalog leads the curated list: {s:?}"
        );
        assert_eq!(sbx[0], "sbx/pi-image", "catalog is in download order");
        assert!(s.contains(&"sbx/claude-image".to_string()));
        // Distros still follow (the generic defaults remain reachable).
        assert!(s.iter().any(|r| r == "alpine"));
    }

    #[test]
    fn suggestions_filter_matches_sbx_refs() {
        let mut form = CreateForm::new(Vec::new());
        form.image = "cla".into();
        let s = form.suggestions();
        assert!(
            s.contains(&"sbx/claude-image".to_string()),
            "typing 'cla' suggests the Claude Code base image: {s:?}"
        );
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
        // The curated tail now leads with the sbx Hub catalog.
        assert_eq!(form.image, "sbx/pi-image");
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
        assert_eq!(form.active_field, FormField::Shell);
        form.handle_key(key(KeyCode::Tab));
        assert_eq!(form.active_field, FormField::Cpus);
        form.handle_key(key(KeyCode::Tab));
        assert_eq!(form.active_field, FormField::Memory);
    }

    #[test]
    fn space_on_mount_toggle_flips_without_moving() {
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        form.active_field = FormField::MountCwd;
        assert_eq!(
            form.handle_key(key(KeyCode::Char(' '))),
            FormAction::Continue
        );
        assert!(!form.mount_cwd);
        assert_eq!(form.active_field, FormField::MountCwd);
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

    #[test]
    fn invalid_root_disk_rejected() {
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        form.root_disk = "huge".into();
        assert!(form.to_create_spec().is_err());
        // Empty is fine (runtime default).
        form.root_disk = String::new();
        let spec = form.to_create_spec().unwrap();
        assert_eq!(spec.root_disk, None);
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

    // -- env profile / credential cycle selects (issues #7 / #6) --

    fn prof(name: &str, env: &[&str]) -> crate::environments::Profile {
        crate::environments::Profile {
            name: name.into(),
            description: String::new(),
            env: env.iter().map(|e| e.to_string()).collect(),
        }
    }

    #[test]
    fn env_profile_suggest_typing_filters_and_enter_adopts() {
        let mut form = CreateForm::new(Vec::new());
        form.env_profiles = vec![prof("dev", &["A=1"]), prof("prod", &["B=2"])];
        form.active_field = FormField::EnvProfile;
        // Typing filters the candidates…
        for c in "pr".chars() {
            form.handle_key(key(KeyCode::Char(c)));
        }
        // …and Enter adopts the (sole) match, then moves on — it never
        // submits a create from inside a list browse.
        assert_eq!(form.handle_key(key(KeyCode::Enter)), FormAction::NextField);
        assert_eq!(form.selected_profile(), Some("prod"));
    }

    #[test]
    fn env_profile_suggest_arrow_moves_highlight_before_adopt() {
        let mut form = CreateForm::new(Vec::new());
        form.env_profiles = vec![prof("dev", &[]), prof("prod", &[]), prof("deploy", &[])];
        form.active_field = FormField::EnvProfile;
        // Empty typing shows all candidates (file order); ↑↓ moves the
        // highlight; Enter adopts the highlighted one.
        form.handle_key(key(KeyCode::Down)); // highlight #2
        form.handle_key(key(KeyCode::Down)); // highlight #3
        assert_eq!(form.handle_key(key(KeyCode::Enter)), FormAction::NextField);
        assert_eq!(form.selected_profile(), Some("deploy"));
    }

    #[test]
    fn suggest_empty_text_enter_is_none_and_moves_on() {
        let mut form = CreateForm::new(Vec::new());
        form.env_profiles = vec![prof("dev", &[])];
        form.credential_names = vec!["gh".into()];
        form.active_field = FormField::EnvProfile;
        assert_eq!(form.handle_key(key(KeyCode::Enter)), FormAction::NextField);
        assert_eq!(form.selected_profile(), None, "empty = <none>");
        form.active_field = FormField::Credentials;
        assert_eq!(form.handle_key(key(KeyCode::Enter)), FormAction::NextField);
        assert_eq!(form.selected_credential(), None);
    }

    #[test]
    fn suggest_unknown_typed_value_errors_inline() {
        let mut form = CreateForm::new(Vec::new());
        form.env_profiles = vec![prof("dev", &[])];
        form.credential_names = vec!["gh".into()];
        form.active_field = FormField::EnvProfile;
        for c in "zzz".chars() {
            form.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(form.handle_key(key(KeyCode::Enter)), FormAction::Continue);
        assert!(
            form.error
                .as_deref()
                .unwrap_or("")
                .contains("unknown environment profile")
        );
        form.active_field = FormField::Credentials;
        form.error = None;
        for c in "zzz".chars() {
            form.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(form.handle_key(key(KeyCode::Enter)), FormAction::Continue);
        assert!(
            form.error
                .as_deref()
                .unwrap_or("")
                .contains("unknown credential")
        );
    }

    #[test]
    fn suggest_fields_support_caret_editing() {
        // Fixing the user-reported pain: a pasted string can be edited in
        // the middle instead of only popping from the end.
        let mut form = CreateForm::new(Vec::new());
        form.env_profiles = vec![prof("deploy", &[])];
        form.active_field = FormField::EnvProfile;
        for c in "deply".chars() {
            form.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(form.env_profile, "deply");
        // ← twice (caret between 'p' and 'l'), insert 'o': "deply" → "depoly".
        form.handle_key(key(KeyCode::Left));
        form.handle_key(key(KeyCode::Left));
        form.handle_key(key(KeyCode::Char('o')));
        assert_eq!(form.env_profile, "depoly");
        // …a wrong typed value drains from the caret; at position 0
        // Backspace is a no-op, so the caret jumps to End first:
        for _ in 0..4 {
            form.handle_key(key(KeyCode::Backspace));
        }
        assert_eq!(form.env_profile, "ly", "drained: {:?}", form.env_profile);
        form.handle_key(key(KeyCode::End));
        for _ in 0..2 {
            form.handle_key(key(KeyCode::Backspace));
        }
        assert!(
            form.env_profile.is_empty(),
            "drained: {:?}",
            form.env_profile
        );
        for c in "deploy".chars() {
            form.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(form.handle_key(key(KeyCode::Enter)), FormAction::NextField);
        assert_eq!(form.selected_profile(), Some("deploy"));
    }

    #[test]
    fn suggest_fields_with_no_options_stay_usable() {
        // A broken/unreadable name file → empty lists: no options, but
        // empty Enter just moves on and unknown typing errors inline —
        // nothing wedges.
        let mut form = CreateForm::new(Vec::new());
        form.env_profiles = Vec::new();
        form.credential_names = Vec::new();
        form.active_field = FormField::EnvProfile;
        assert_eq!(form.handle_key(key(KeyCode::Enter)), FormAction::NextField);
        assert_eq!(form.selected_profile(), None);
        form.active_field = FormField::Credentials;
        for c in "zzz".chars() {
            form.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(form.handle_key(key(KeyCode::Enter)), FormAction::Continue);
        assert!(form.error.is_some());
    }

    #[test]
    fn to_create_spec_merges_profile_vars_before_form_vars() {
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        form.env_profiles = vec![prof("dev", &["A=1", "B=1"])];
        form.env_vars = vec!["A=2".into(), "C=3".into()];
        form.env_profile = "dev".into();
        let spec = form.to_create_spec().unwrap();
        // Profile vars first, form vars appended — SDK last-wins, so the
        // form's A=2 overrides the profile's A=1.
        assert_eq!(
            spec.env,
            vec!["A=1", "B=1", "A=2", "C=3"]
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn without_selected_profile_env_is_unchanged() {
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        form.env_vars = vec!["A=1".into()];
        form.env_profiles = Vec::new();
        let spec = form.to_create_spec().unwrap();
        assert_eq!(spec.env, vec!["A=1".to_string()]);
    }

    #[test]
    fn unknown_selected_profile_errors_like_the_validator() {
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        form.env_profile = "no-such-profile".into();
        let err = form.to_create_spec().unwrap_err().to_string();
        assert!(
            err.contains("unknown environment profile 'no-such-profile'"),
            "{err}"
        );
    }

    #[test]
    fn from_template_preselects_the_spec_env_profile() {
        let t = crate::template::parse(
            "x",
            "[meta]\nname = \"T\"\ndescription = \"d\"\n\n[spec]\nimage = \"alpine\"\nenv_profile = \"dev\"\n",
        )
        .unwrap();
        let form = CreateForm::from_template(&t, "/home/me/proj".into());
        assert_eq!(form.selected_profile(), Some("dev"));
        // Templates carry no credential association: starts at <none>.
        assert!(form.selected_credential().is_none());
    }

    #[test]
    fn save_template_roundtrips_env_profile() {
        let mut form = CreateForm::new(Vec::new());
        form.image = "alpine".into();
        form.env_profile = "dev".into();
        let t = form.to_template("T".into(), "d".into());
        assert_eq!(t.spec.env_profile.as_deref(), Some("dev"));
        // …and it pre-selects again on the from-template path.
        let form = CreateForm::from_template(&t, "/home/me/proj".into());
        assert_eq!(form.selected_profile(), Some("dev"));
    }

    #[test]
    fn suggest_fields_render_typed_values_and_placeholders() {
        let mut form = CreateForm::new(Vec::new());
        let text = flatten(&grouped_field_lines(&form));
        assert!(text.contains("Env-Profile"), "{text}");
        assert!(text.contains("Credentials"), "{text}");
        assert!(text.contains("[<none>]"), "{text}");
        form.env_profile = "dev".into();
        let text = flatten(&grouped_field_lines(&form));
        assert!(text.contains("[dev]"), "{text}");
    }

    #[test]
    fn getters_report_the_selections() {
        let mut form = CreateForm::new(Vec::new());
        assert_eq!(form.selected_profile(), None);
        assert_eq!(form.selected_credential(), None);
        form.env_profile = "dev".into();
        form.credential = "gh".into();
        assert_eq!(form.selected_profile(), Some("dev"));
        assert_eq!(form.selected_credential(), Some("gh"));
        // Whitespace-only counts as cleared (none).
        form.env_profile = "  ".into();
        assert_eq!(form.selected_profile(), None);
    }
}
