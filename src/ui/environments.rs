//! Environments & credentials manager (GitHub issues #7/#6): a full-screen
//! master-detail view for the dashboard's `E` key. The PROFILES tab edits
//! `environments.toml`; the CREDENTIALS tab edits the `credentials.toml`
//! *metadata*. Both files are human-edit-sized (a few KiB), so profile and
//! credential metadata changes persist right here synchronously
//! (temp-file + rename, like the template save path).
//!
//! What stays OUTSIDE this view is the OS-keyring work — it is async and
//! owned by the event loop: [`EnvAction::SaveCredential`] /
//! [`EnvAction::DeleteCredential`] carry the secret delta (`Some(secret)` =
//! store/update the keyring value, `None` = the stored secret is unchanged)
//! and the loop reports back via [`EnvironmentsView::note_async_result`]
//! (+ optional [`EnvironmentsView::reload`]).
//!
//! The list fields (profile `env`, credential `profiles`/`sandboxes`)
//! mirror the create form's list editing as seen by the user (type +
//! Enter to add, Enter on a selected row to edit it in place, Delete
//! removes) but are implemented locally — no reuse of `ui/create.rs`
//! internals on either side.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::credentials::{Credential, CredentialsFile, save_credentials, validate_credential};
use crate::environments::{Profile, load_profiles, save_profiles, validate_profile};
use crate::template::{LayoutKind, template_layout};
use crate::ui::chrome::{self, FooterHint, FooterRole, Tab, TitleInfo};
use crate::ui::theme::THEME;

/// Which of the two inner tabs is active (chrome tab bar `1`/`2`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum EnvTab {
    /// The environment profiles (`environments.toml`).
    #[default]
    Profiles,
    /// The credentials (`credentials.toml` metadata + keyring).
    Credentials,
}

impl EnvTab {
    /// Index of this tab's master selection inside the `selected` array.
    fn slot(self) -> usize {
        match self {
            EnvTab::Profiles => 0,
            EnvTab::Credentials => 1,
        }
    }

    /// Wrapping predecessor of this tab.
    fn prev(self) -> Self {
        match self {
            EnvTab::Profiles => EnvTab::Credentials,
            EnvTab::Credentials => EnvTab::Profiles,
        }
    }

    /// Wrapping successor of this tab.
    fn next(self) -> Self {
        self.prev()
    }
}

/// What [`EnvironmentsView::handle_key`] produced for the event loop.
#[derive(PartialEq)]
pub enum EnvAction {
    /// Key consumed, nothing to report.
    Continue,
    /// Esc on the master list — close the view, back to the dashboard.
    Done,
    /// A validated profile was created/updated; the metadata file is
    /// already persisted.
    SaveProfile(Box<Profile>),
    /// A confirmed profile delete; the metadata file is already persisted.
    DeleteProfile(String),
    /// A validated credential metadata save, already persisted. The second
    /// element is the secret delta: `Some(secret)` = the event loop should
    /// store/update the keyring value, `None` = keep the stored secret.
    SaveCredential(Box<Credential>, Option<String>),
    /// A confirmed credential delete — metadata already persisted; the
    /// event loop removes the keyring secret for this name.
    DeleteCredential(String),
}

impl std::fmt::Debug for EnvAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Secret values never reach Debug (same rule as KeyringOp in app.rs).
        match self {
            Self::Continue => write!(f, "Continue"),
            Self::Done => write!(f, "Done"),
            Self::SaveProfile(p) => write!(f, "SaveProfile({})", p.name),
            Self::DeleteProfile(n) => write!(f, "DeleteProfile({n})"),
            Self::SaveCredential(c, s) => match s {
                Some(_) => write!(f, "SaveCredential({}, secret=…)", c.name),
                None => write!(f, "SaveCredential({}, secret=unchanged)", c.name),
            },
            Self::DeleteCredential(n) => write!(f, "DeleteCredential({n})"),
        }
    }
}

// ---------------------------------------------------------------------------
// Editor state
// ---------------------------------------------------------------------------

/// Focus within the profile editor, in Tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PField {
    Name,
    Description,
    Env,
}

impl PField {
    const ALL: [PField; 3] = [PField::Name, PField::Description, PField::Env];
}

/// Focus within the credential editor, in Tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CField {
    Name,
    Description,
    EnvKey,
    Secret,
    Profiles,
    Sandboxes,
}

impl CField {
    const ALL: [CField; 6] = [
        CField::Name,
        CField::Description,
        CField::EnvKey,
        CField::Secret,
        CField::Profiles,
        CField::Sandboxes,
    ];
}

/// Wrapping field step over a `ALL` const array (variants in the same order).
fn stepped<T, const N: usize>(all: [T; N], current: T, delta: isize) -> T
where
    T: Copy + PartialEq,
{
    let idx = all.iter().position(|f| *f == current).unwrap_or(0);
    let idx = (idx as isize + delta).rem_euclid(all.len() as isize);
    all[idx as usize]
}

/// Where an editor key left the editor: stay (normal input), commit the
/// editor (Enter save), or move focus one field forward.
#[derive(Debug, PartialEq)]
enum EditorOutcome {
    Stay,
    SaveEditor,
    NextField,
}

/// The open editor (one at a time; Esc closes and discards changes).
#[derive(PartialEq)]
enum Editor {
    Profile {
        /// `Some` = editing this row (rename allowed, relocations visible at
        /// next create); `None` = adding a new profile.
        index: Option<usize>,
        /// Caret (char index) of the ACTIVE field's buffer; reset to that
        /// buffer's end when the field focus moves.
        cursor: usize,
        name: String,
        description: String,
        env: Vec<String>,
        /// Focus within [`PField`].
        field: PField,
        env_input: String,
        env_sel: usize,
        /// Row currently loaded into the input row for in-place editing.
        env_slot: Option<usize>,
        error: Option<String>,
    },
    Credential {
        /// `Some` = editing this row (renaming is rejected — the keyring
        /// secret is keyed by name and would be orphaned); `None` = adding.
        index: Option<usize>,
        /// Caret of the active field (see [`Editor::Profile`]).
        cursor: usize,
        name: String,
        description: String,
        env_key: String,
        /// Raw secret characters; rendered masked only. Empty in an edit
        /// means "keep the stored secret" (`None` in the emitted action).
        secret: String,
        /// Profiles/scopes as one-entry-per-row strings (empty = unassigned).
        profiles: Vec<String>,
        sandboxes: Vec<String>,
        /// Focus within [`CField`].
        field: CField,
        /// Shared input/selection/slot for the active list field (mirrors
        /// the create form's single list buffer).
        list_input: String,
        list_sel: usize,
        list_slot: Option<usize>,
        error: Option<String>,
    },
}

impl std::fmt::Debug for Editor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The raw secret never reaches Debug (same rule as KeyringOp).
        match self {
            Self::Profile { .. } => write!(f, "Editor::Profile(…)"),
            Self::Credential { name, env_key, .. } => {
                write!(
                    f,
                    "Editor::Credential(name: {name}, env_key: {env_key}, secret: …)"
                )
            }
        }
    }
}

impl Editor {
    /// Move focus by `delta` fields (wrapping); the caret lands at the
    /// end of the newly focused buffer.
    fn advance(&mut self, delta: isize) {
        match self {
            Self::Profile { field, .. } => *field = stepped(PField::ALL, *field, delta),
            Self::Credential { field, .. } => *field = stepped(CField::ALL, *field, delta),
        }
        let caret = crate::ui::text_input::caret_end(self.active_field_buf());
        self.set_cursor(caret);
    }

    /// The buffer of the currently focused field (for caret resets).
    fn active_field_buf(&self) -> &str {
        match self {
            Self::Profile {
                field,
                name,
                description,
                env_input,
                ..
            } => match field {
                PField::Name => name,
                PField::Description => description,
                PField::Env => env_input,
            },
            Self::Credential {
                field,
                name,
                description,
                env_key,
                secret,
                list_input,
                ..
            } => match field {
                CField::Name => name,
                CField::Description => description,
                CField::EnvKey => env_key,
                CField::Secret => secret,
                CField::Profiles | CField::Sandboxes => list_input,
            },
        }
    }

    fn set_cursor(&mut self, at: usize) {
        match self {
            Self::Profile { cursor, .. } | Self::Credential { cursor, .. } => *cursor = at,
        }
    }

    /// Route a key into the fields of the open editor.
    fn handle_key(&mut self, key: KeyEvent) -> EditorOutcome {
        match self {
            Editor::Profile {
                field,
                cursor,
                name,
                description,
                env,
                env_input,
                env_sel,
                env_slot,
                ..
            } => match field {
                PField::Name => text_key(name, cursor, key),
                PField::Description => text_key(description, cursor, key),
                PField::Env => list_key(
                    ListBuf {
                        items: env,
                        input: env_input,
                        cursor,
                        sel: env_sel,
                        slot: env_slot,
                    },
                    key,
                    // Env is the profile form's last field.
                    true,
                ),
            },
            Editor::Credential {
                field,
                cursor,
                name,
                description,
                env_key,
                secret,
                profiles,
                sandboxes,
                list_input,
                list_sel,
                list_slot,
                ..
            } => match field {
                CField::Name => text_key(name, cursor, key),
                CField::Description => text_key(description, cursor, key),
                CField::EnvKey => text_key(env_key, cursor, key),
                CField::Secret => text_key(secret, cursor, key),
                CField::Profiles => list_key(
                    ListBuf {
                        items: profiles,
                        input: list_input,
                        cursor,
                        sel: list_sel,
                        slot: list_slot,
                    },
                    key,
                    false,
                ),
                CField::Sandboxes => list_key(
                    ListBuf {
                        items: sandboxes,
                        input: list_input,
                        cursor,
                        sel: list_sel,
                        slot: list_slot,
                    },
                    key,
                    // Sandboxes is the credential form's last field.
                    true,
                ),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// View state
// ---------------------------------------------------------------------------

/// State of the environments & credentials manager view. Constructed by the
/// event loop; everything private except the API below.
#[derive(Debug, Default)]
pub struct EnvironmentsView {
    /// Active inner tab (chrome pill shows it; `1`/`2`/arrows switch).
    tab: EnvTab,
    profiles: Vec<Profile>,
    credentials: Vec<Credential>,
    /// Master selection per tab, indexed by [`EnvTab::slot`].
    selected: [usize; 2],
    editor: Option<Editor>,
    /// Armed inline delete confirm: (tab, name); `y` commits, `n`/Esc
    /// cancels. All other keys are swallowed while armed.
    confirm: Option<(EnvTab, String)>,
    /// Keyring op in flight ("saving secret 'gh'…"), shown in the status
    /// band until [`note_async_result`](Self::note_async_result) clears it.
    async_pending: Option<String>,
    /// Last one-line report: (text, is_err).
    status: Option<(String, bool)>,
}

impl EnvironmentsView {
    /// Open the view, loading both config files.
    pub fn new() -> Self {
        let mut view = Self::default();
        view.reload();
        view
    }

    /// Re-read both TOML files into memory (used at construction and to
    /// pick up edits made outside the TUI). A read failure keeps the
    /// current lists and shows in the status band.
    pub fn reload(&mut self) {
        match load_profiles() {
            Ok(p) => self.profiles = p,
            Err(e) => self.status = Some((format!("environments: {e:#}"), true)),
        }
        match crate::credentials::load_credentials() {
            Ok(file) => self.credentials = file.credentials,
            Err(e) => self.status = Some((format!("credentials: {e:#}"), true)),
        }
        self.clamp_selected();
    }

    /// Report the outcome of an async keyring op and clear the pending
    /// marker. `ok = false` renders `msg` in the error color.
    pub fn note_async_result(&mut self, ok: bool, msg: &str) {
        self.async_pending = None;
        self.status = Some((msg.to_string(), !ok));
    }

    /// Handle a key while the view is open. The event loop dispatches the
    /// returned [`EnvAction`]: profile actions need nothing (already
    /// persisted here), credential actions carry the keyring work.
    pub fn handle_key(&mut self, key: KeyEvent) -> EnvAction {
        if self.editor.is_some() {
            return self.editor_key(key);
        }
        if self.confirm.is_some() {
            return self.confirm_key(key);
        }
        match key.code {
            KeyCode::Esc => EnvAction::Done,
            KeyCode::Up => {
                self.move_selection(-1);
                EnvAction::Continue
            }
            KeyCode::Down => {
                self.move_selection(1);
                EnvAction::Continue
            }
            KeyCode::Left | KeyCode::BackTab => {
                self.tab = self.tab.prev();
                EnvAction::Continue
            }
            KeyCode::Right | KeyCode::Tab => {
                self.tab = self.tab.next();
                EnvAction::Continue
            }
            KeyCode::Char('1') => {
                self.tab = EnvTab::Profiles;
                EnvAction::Continue
            }
            KeyCode::Char('2') => {
                self.tab = EnvTab::Credentials;
                EnvAction::Continue
            }
            KeyCode::Char('a') => {
                self.open_editor(true);
                EnvAction::Continue
            }
            KeyCode::Char('e') | KeyCode::Enter => {
                self.open_editor(false);
                EnvAction::Continue
            }
            KeyCode::Char('d') => {
                self.arm_delete();
                EnvAction::Continue
            }
            _ => EnvAction::Continue,
        }
    }

    /// Clamp both master selections to the current list lengths.
    fn clamp_selected(&mut self) {
        for (tab, len) in [
            (EnvTab::Profiles, self.profiles.len()),
            (EnvTab::Credentials, self.credentials.len()),
        ] {
            let slot = tab.slot();
            if len == 0 || self.selected[slot] >= len {
                self.selected[slot] = len.saturating_sub(1);
            }
        }
    }

    /// Move the active tab's master selection by `delta`, clamped.
    fn move_selection(&mut self, delta: isize) {
        let slot = self.tab.slot();
        let len = match self.tab {
            EnvTab::Profiles => self.profiles.len(),
            EnvTab::Credentials => self.credentials.len(),
        };
        if len == 0 {
            return;
        }
        let next = self.selected[slot] as isize + delta;
        self.selected[slot] = next.clamp(0, len as isize - 1) as usize;
    }

    /// Arm the inline delete confirm for the selected entry of the active
    /// tab (a no-op on an empty list).
    fn arm_delete(&mut self) {
        let slot = self.tab.slot();
        let name = match self.tab {
            EnvTab::Profiles => self
                .profiles
                .get(self.selected[slot])
                .map(|p| p.name.clone()),
            EnvTab::Credentials => self
                .credentials
                .get(self.selected[slot])
                .map(|c| c.name.clone()),
        };
        if let Some(name) = name {
            self.confirm = Some((self.tab, name));
        }
    }

    /// Keys while a delete confirm is armed: `y` commits, `n`/Esc cancels.
    fn confirm_key(&mut self, key: KeyEvent) -> EnvAction {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => self.commit_delete(),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.confirm = None;
                EnvAction::Continue
            }
            _ => EnvAction::Continue,
        }
    }

    /// Apply the confirmed delete. Profile deletes persist + report right
    /// here; credential deletes persist the metadata and defer the keyring
    /// work to the event loop (pending marker until it reports back).
    fn commit_delete(&mut self) -> EnvAction {
        let Some((tab, name)) = self.confirm.take() else {
            return EnvAction::Continue;
        };
        match tab {
            EnvTab::Profiles => {
                let mut updated = self.profiles.clone();
                updated.retain(|p| p.name != name);
                // shortcut: sync config write on the event thread — the file
                // is tiny (template-save precedent); move on a task if it grows
                if let Err(e) = save_profiles(&updated) {
                    self.status = Some((format!("profile delete: {e:#}"), true));
                    return EnvAction::Continue;
                }
                self.profiles = updated;
                self.clamp_selected();
                self.status = Some((format!("profile '{name}' deleted"), false));
                EnvAction::DeleteProfile(name)
            }
            EnvTab::Credentials => {
                let mut file = CredentialsFile {
                    credentials: self.credentials.clone(),
                };
                file.credentials.retain(|c| c.name != name);
                // See above: sync, small file.
                if let Err(e) = save_credentials(&file) {
                    self.status = Some((format!("credential delete: {e:#}"), true));
                    return EnvAction::Continue;
                }
                self.credentials = file.credentials;
                self.clamp_selected();
                self.async_pending = Some(format!("deleting keyring secret for '{name}'…"));
                EnvAction::DeleteCredential(name)
            }
        }
    }

    /// Open the add (`a`) or edit (`e`/Enter) editor for the active tab.
    fn open_editor(&mut self, add: bool) {
        match self.tab {
            EnvTab::Profiles => {
                let sel = self.selected[0];
                let (index, name, description, env) = match self.profiles.get(sel) {
                    Some(p) if !add => (
                        Some(sel),
                        p.name.clone(),
                        p.description.clone(),
                        p.env.clone(),
                    ),
                    Some(_) | None if add => (None, String::new(), String::new(), Vec::new()),
                    _ => return,
                };
                self.editor = Some(Editor::Profile {
                    index,
                    cursor: crate::ui::text_input::caret_end(&name),
                    name,
                    description,
                    env,
                    field: PField::Name,
                    env_input: String::new(),
                    env_sel: 0,
                    env_slot: None,
                    error: None,
                });
            }
            EnvTab::Credentials => {
                let sel = self.selected[1];
                let (index, name, description, env_key, profiles, sandboxes) =
                    match self.credentials.get(sel) {
                        Some(c) if !add => (
                            Some(sel),
                            c.name.clone(),
                            c.description.clone(),
                            c.env_key.clone(),
                            c.profiles.clone(),
                            c.sandboxes.clone(),
                        ),
                        Some(_) | None if add => (
                            None,
                            String::new(),
                            String::new(),
                            String::new(),
                            Vec::new(),
                            Vec::new(),
                        ),
                        _ => return,
                    };
                self.editor = Some(Editor::Credential {
                    index,
                    cursor: crate::ui::text_input::caret_end(&name),
                    name,
                    description,
                    env_key,
                    secret: String::new(),
                    profiles,
                    sandboxes,
                    field: CField::Name,
                    list_input: String::new(),
                    list_sel: 0,
                    list_slot: None,
                    error: None,
                });
            }
        }
    }

    /// Keys while an editor is open: input editing, field cycling (Tab),
    /// save (Enter on text / empty last field, Ctrl+Enter anywhere), Esc
    /// discards.
    fn editor_key(&mut self, key: KeyEvent) -> EnvAction {
        // Ctrl+Enter saves from any field — the create form's escape hatch.
        if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::CONTROL) {
            return self.commit_editor();
        }
        if key.code == KeyCode::Esc {
            self.editor = None;
            return EnvAction::Continue;
        }
        let Some(ed) = self.editor.as_mut() else {
            return EnvAction::Continue;
        };
        if key.code == KeyCode::Tab {
            ed.advance(1);
            return EnvAction::Continue;
        }
        if key.code == KeyCode::BackTab {
            ed.advance(-1);
            return EnvAction::Continue;
        }
        match ed.handle_key(key) {
            EditorOutcome::SaveEditor => self.commit_editor(),
            EditorOutcome::NextField => {
                ed.advance(1);
                EnvAction::Continue
            }
            EditorOutcome::Stay => EnvAction::Continue,
        }
    }

    /// Save whichever editor is open: validate, check for name collisions,
    /// persist the file, then emit the event. On any failure the editor
    /// stays open with the inline error and no event is emitted.
    fn commit_editor(&mut self) -> EnvAction {
        match &self.editor {
            Some(Editor::Profile { .. }) => self.commit_profile(),
            Some(Editor::Credential { .. }) => self.commit_credential(),
            None => EnvAction::Continue,
        }
    }

    /// Validate + persist the open profile editor and emit
    /// [`EnvAction::SaveProfile`].
    fn commit_profile(&mut self) -> EnvAction {
        let Some(Editor::Profile {
            index,
            name,
            description,
            env,
            error,
            ..
        }) = self.editor.as_mut()
        else {
            return EnvAction::Continue;
        };
        let at = *index;
        let profile = Profile {
            name: name.trim().to_string(),
            description: description.clone(),
            env: env.clone(),
        };
        if let Err(e) = validate_profile(&profile) {
            *error = Some(e);
            return EnvAction::Continue;
        }
        if self
            .profiles
            .iter()
            .enumerate()
            .any(|(i, p)| Some(i) != at && p.name == profile.name)
        {
            *error = Some(format!("profile '{}' already exists", profile.name));
            return EnvAction::Continue;
        }
        let mut updated = self.profiles.clone();
        match at {
            Some(i) if i < updated.len() => updated[i] = profile.clone(),
            _ => updated.push(profile.clone()),
        }
        // shortcut: sync config write on the event thread — the file
        // is tiny (template-save precedent); move on a task if it grows
        if let Err(e) = save_profiles(&updated) {
            *error = Some(format!("saving: {e:#}"));
            return EnvAction::Continue;
        }
        *name = profile.name.clone();
        self.profiles = updated;
        self.editor = None;
        self.status = Some((format!("profile '{}' saved", profile.name), false));
        EnvAction::SaveProfile(Box::new(profile))
    }

    /// Validate + persist the open credential editor and emit
    /// [`EnvAction::SaveCredential`] (with the secret delta for the event
    /// loop's keyring work).
    fn commit_credential(&mut self) -> EnvAction {
        let Some(Editor::Credential {
            index,
            name,
            description,
            env_key,
            secret,
            profiles,
            sandboxes,
            error,
            ..
        }) = self.editor.as_mut()
        else {
            return EnvAction::Continue;
        };
        let at = *index;
        let credential = Credential {
            name: name.trim().to_string(),
            description: description.clone(),
            env_key: env_key.trim().to_string(),
            profiles: profiles.clone(),
            sandboxes: sandboxes.clone(),
        };
        if let Err(e) = validate_credential(&credential) {
            *error = Some(e);
            return EnvAction::Continue;
        }
        if self
            .credentials
            .iter()
            .enumerate()
            .any(|(i, c)| Some(i) != at && c.name == credential.name)
        {
            *error = Some(format!("credential '{}' already exists", credential.name));
            return EnvAction::Continue;
        }
        // The keyring is keyed by the credential name: a rename would
        // silently orphan the stored secret (injection would find nothing).
        if let Some(i) = at
            && i < self.credentials.len()
            && self.credentials[i].name != credential.name
        {
            *error = Some(
                "renaming a credential would orphan its keyring secret — delete it and add it again"
                    .to_string(),
            );
            return EnvAction::Continue;
        }
        let secret_delta = if secret.is_empty() {
            None
        } else {
            Some(secret.clone())
        };
        if at.is_none() && secret_delta.is_none() {
            *error = Some(
                "a new credential needs a secret — it is stored in the OS keyring".to_string(),
            );
            return EnvAction::Continue;
        }
        let mut file = CredentialsFile {
            credentials: self.credentials.clone(),
        };
        match at {
            Some(i) if i < file.credentials.len() => file.credentials[i] = credential.clone(),
            _ => file.credentials.push(credential.clone()),
        }
        // shortcut: sync config write on the event thread — the file
        // is tiny (template-save precedent); move on a task if it grows
        if let Err(e) = save_credentials(&file) {
            *error = Some(format!("saving: {e:#}"));
            return EnvAction::Continue;
        }
        self.credentials = file.credentials;
        self.editor = None;
        if secret_delta.is_some() {
            self.async_pending = Some(format!("saving keyring secret for '{}'…", credential.name));
        } else {
            self.status = Some((
                format!("credential '{}' saved (secret unchanged)", credential.name),
                false,
            ));
        }
        EnvAction::SaveCredential(Box::new(credential), secret_delta)
    }
}

// ---------------------------------------------------------------------------
// Input-field key mechanics (create-form list/text pattern, local copy)
// ---------------------------------------------------------------------------

/// How a text-field key route ended: typing stays, Enter saves.
///
/// Cursor editing (insert at caret, ←/→/Home/End/Delete/Backspace) is the
/// shared `text_edit` route; with no caret movement the Enter path saves.
fn text_key(buf: &mut String, cursor: &mut usize, key: KeyEvent) -> EditorOutcome {
    use crossterm::event::KeyCode;
    if crate::ui::text_input::text_edit(buf, cursor, &key) {
        return EditorOutcome::Stay;
    }
    if key.code == KeyCode::Enter {
        return EditorOutcome::SaveEditor;
    }
    EditorOutcome::Stay
}

/// The four editable parts of one list field, bundled for the shared
/// handler (create-form single-buffer model).
struct ListBuf<'a> {
    items: &'a mut Vec<String>,
    input: &'a mut String,
    /// Caret within `input`.
    cursor: &'a mut usize,
    sel: &'a mut usize,
    /// Row loaded into the input for in-place editing (`Enter` on a row).
    slot: &'a mut Option<usize>,
}

/// A key on a list field: type to fill the input row, Enter commits the
/// input (appends, or replaces the slot row while editing), Enter on an
/// empty input pulls the selected row up for editing, ↑/↓ move the row
/// highlight, Delete removes the selected row (with an empty input).
fn list_key(f: ListBuf<'_>, key: KeyEvent, is_last_field: bool) -> EditorOutcome {
    // Delete on an empty input keeps the row-removal semantics — the
    // shared edit route would consume it as a no-op.
    let row_delete = f.input.is_empty() && key.code == KeyCode::Delete;
    if !row_delete && crate::ui::text_input::text_edit(f.input, f.cursor, &key) {
        return EditorOutcome::Stay;
    }
    match key.code {
        KeyCode::Delete => {
            if !f.items.is_empty() && *f.sel < f.items.len() {
                f.items.remove(*f.sel);
                if f.items.is_empty() {
                    *f.sel = 0;
                } else if *f.sel >= f.items.len() {
                    *f.sel = f.items.len() - 1;
                }
                // The slot row may have shifted under us: abort the edit
                // (the input keeps its text as a fresh "new item" draft).
                *f.slot = None;
            }
            EditorOutcome::Stay
        }
        KeyCode::Enter => {
            if !f.input.is_empty() {
                let item = std::mem::take(f.input);
                *f.cursor = 0;
                let target = match *f.slot {
                    Some(i) if i < f.items.len() => i,
                    _ => f.items.len(),
                };
                *f.sel = target;
                if target == f.items.len() {
                    f.items.push(item);
                } else {
                    f.items[target] = item;
                }
                *f.slot = None;
                EditorOutcome::Stay
            } else if f.slot.is_some() {
                // Empty Enter while editing: abort the edit.
                *f.slot = None;
                EditorOutcome::Stay
            } else if *f.sel < f.items.len() {
                // Pull the selected row up into the input for editing.
                *f.slot = Some(*f.sel);
                *f.input = f.items[*f.sel].clone();
                *f.cursor = crate::ui::text_input::caret_end(f.input);
                EditorOutcome::Stay
            } else if is_last_field {
                EditorOutcome::SaveEditor
            } else {
                EditorOutcome::NextField
            }
        }
        KeyCode::Up => {
            *f.sel = f.sel.saturating_sub(1);
            EditorOutcome::Stay
        }
        KeyCode::Down => {
            if *f.sel + 1 < f.items.len() {
                *f.sel += 1;
            }
            EditorOutcome::Stay
        }
        _ => EditorOutcome::Stay,
    }
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Render the whole view (chrome bands included) into `area`.
pub fn render(frame: &mut Frame, area: Rect, view: &EnvironmentsView) {
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

    // The armed delete confirm fills the warn banner band; the status band
    // shows the async keyring marker, else the last report.
    let banner = view.confirm.as_ref().map(|(tab, name)| match tab {
        EnvTab::Profiles => {
            format!(" Delete environment profile '{name}'? y/N — Esc cancels ")
        }
        EnvTab::Credentials => format!(
            " Delete credential '{name}'? (its keyring secret is removed too) y/N — Esc cancels "
        ),
    });
    let status = view
        .async_pending
        .as_deref()
        .map(|p| (p, false))
        .or_else(|| view.status.as_ref().map(|(m, e)| (m.as_str(), *e)));

    let areas = chrome::chrome_layout(area, banner.as_deref(), status.as_ref().map(|(m, _)| *m));
    let tabs = vec![
        Tab {
            key: '1',
            label: "PROFILES",
            active: view.tab == EnvTab::Profiles,
            count: Some(view.profiles.len()),
        },
        Tab {
            key: '2',
            label: "CREDENTIALS",
            active: view.tab == EnvTab::Credentials,
            count: Some(view.credentials.len()),
        },
    ];
    chrome::render_chrome(frame, &areas, &title, &tabs);
    chrome::render_chrome_bands(frame, &areas, banner.as_deref(), status);

    if let Some(ed) = &view.editor {
        render_editor(frame, areas.body, ed);
    } else {
        match template_layout(areas.body.width, areas.body.height) {
            LayoutKind::MasterDetail => {
                let [list, detail] = split_h(areas.body, 40);
                render_master_list(frame, list, view);
                render_details(frame, detail, view);
            }
            LayoutKind::Stacked => {
                let [list, detail] = split_v(areas.body, 40);
                render_master_list(frame, list, view);
                render_details(frame, detail, view);
            }
            LayoutKind::ListOnly => render_master_list(frame, areas.body, view),
        }
    }

    chrome::render_chrome_footer(frame, &areas, &footer_hints(view));
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

/// Footer hints for the current interaction mode (editor / armed confirm /
/// master list).
fn footer_hints(view: &EnvironmentsView) -> Vec<FooterHint<'static>> {
    fn hint(key: &'static str, label: &'static str, role: FooterRole) -> FooterHint<'static> {
        FooterHint { key, label, role }
    }
    if view.editor.is_some() {
        return vec![
            hint("[Tab]", "field", FooterRole::Plain),
            hint("[Ctrl+Enter]", "save", FooterRole::Accent),
            hint("[Del]", "remove row", FooterRole::Warn),
            hint("[Esc]", "cancel", FooterRole::Err),
        ];
    }
    if view.confirm.is_some() {
        return vec![
            hint("[y]", "delete", FooterRole::Err),
            hint("[n/Esc]", "no", FooterRole::Plain),
        ];
    }
    let add = match view.tab {
        EnvTab::Profiles => "add profile",
        EnvTab::Credentials => "add credential",
    };
    vec![
        hint("[a]", add, FooterRole::Accent),
        hint("[e]", "edit", FooterRole::Plain),
        hint("[d]", "delete", FooterRole::Warn),
        hint("[1/2]", "tab", FooterRole::Plain),
        hint("[Esc]", "←", FooterRole::Err),
    ]
}

/// The selected entry of the active tab in the pane's title.
fn pane_title(view: &EnvironmentsView) -> String {
    let slot = view.tab.slot();
    match view.tab {
        EnvTab::Profiles => view
            .profiles
            .get(view.selected[slot])
            .map(|p| format!(" Profile: {} ", p.name))
            .unwrap_or_else(|| " Profile ".to_string()),
        EnvTab::Credentials => view
            .credentials
            .get(view.selected[slot])
            .map(|c| format!(" Credential: {} ", c.name))
            .unwrap_or_else(|| " Credential ".to_string()),
    }
}

/// Master list: names only (details live in the detail pane per the
/// template picker convention), selection marker + scrolling window.
fn render_master_list(frame: &mut Frame, area: Rect, view: &EnvironmentsView) {
    let t = &THEME;
    let (title, empty_hint, names): (&'static str, &str, Vec<&str>) = match view.tab {
        EnvTab::Profiles => (
            " Profiles ",
            "(no profiles yet — press [a] to add)",
            view.profiles.iter().map(|p| p.name.as_str()).collect(),
        ),
        EnvTab::Credentials => (
            " Credentials ",
            "(no credentials yet — press [a] to add)",
            view.credentials.iter().map(|c| c.name.as_str()).collect(),
        ),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            title,
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(t.accent))
        .style(Style::default().bg(t.panel));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = Vec::with_capacity(names.len());
    if names.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("  {empty_hint}"),
            Style::default().fg(t.muted),
        )));
    }
    let selected = view.selected[view.tab.slot()];
    for (i, name) in names.iter().enumerate() {
        let is_sel = i == selected;
        let marker = if is_sel { "▶ " } else { "  " };
        let style = if is_sel {
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.fg)
        };
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(format!("{marker}{name}"), style),
        ]));
    }
    // The window follows the selection (same rule as the template picker).
    let entries_visible = inner.height.max(1) as usize;
    let offset = selected.saturating_sub(entries_visible.saturating_sub(1));
    let p = Paragraph::new(lines)
        .block(Block::default().style(Style::default().bg(t.panel)))
        .scroll((offset as u16, 0));
    frame.render_widget(p, inner);
}

/// Detail pane for the selected entry (the editor replaces it while open).
fn render_details(frame: &mut Frame, area: Rect, view: &EnvironmentsView) {
    let t = &THEME;
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            pane_title(view),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(t.accent))
        .style(Style::default().bg(t.panel));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let slot = view.tab.slot();
    let lines = match view.tab {
        EnvTab::Profiles => {
            let Some(p) = view.profiles.get(view.selected[slot]) else {
                return;
            };
            let mut lines = vec![Line::from(Span::styled(
                format!("  {}", muted_or(&p.description, "(no description)")),
                Style::default().fg(if p.description.is_empty() {
                    t.muted
                } else {
                    t.text
                }),
            ))];
            lines.push(Line::raw(""));
            lines.push(section_header("Environment variables"));
            if p.env.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  (none — injects no environment variables)",
                    Style::default().fg(t.muted),
                )));
            } else {
                for entry in &p.env {
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(entry.clone(), Style::default().fg(t.text)),
                    ]));
                }
            }
            lines
        }
        EnvTab::Credentials => {
            let Some(c) = view.credentials.get(view.selected[slot]) else {
                return;
            };
            let mut lines = vec![Line::from(Span::styled(
                format!("  {}", muted_or(&c.description, "(no description)")),
                Style::default().fg(if c.description.is_empty() {
                    t.muted
                } else {
                    t.text
                }),
            ))];
            lines.push(Line::raw(""));
            lines.push(Line::from(vec![
                field_label_span("Env var", false),
                Span::raw(" "),
                Span::styled(c.env_key.clone(), Style::default().fg(t.text)),
            ]));
            lines.push(Line::from(vec![
                field_label_span("Secret", false),
                Span::raw(" "),
                Span::styled(
                    "stored in the OS keyring — never displayed",
                    Style::default().fg(t.muted),
                ),
            ]));
            lines.push(Line::raw(""));
            lines.push(Line::from(vec![
                field_label_span("Profiles", false),
                Span::raw(" "),
                Span::styled(or_all(&c.profiles), Style::default().fg(t.text)),
            ]));
            lines.push(Line::from(vec![
                field_label_span("Sandboxes", false),
                Span::raw(" "),
                Span::styled(or_all(&c.sandboxes), Style::default().fg(t.text)),
            ]));
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled(
                "  injected only when assigned: select at create, or attach to the create's profile",
                Style::default().fg(t.muted),
            )));
            lines
        }
    };
    let p = Paragraph::new(lines).block(Block::default().style(Style::default().bg(t.panel)));
    frame.render_widget(p, inner);
}

/// `text` or the muted placeholder when empty.
fn muted_or<'a>(text: &'a str, placeholder: &'a str) -> &'a str {
    if text.is_empty() { placeholder } else { text }
}

/// A joined scope list or the unassigned placeholder (empty = injected nowhere).
fn or_all(items: &[String]) -> String {
    if items.is_empty() {
        "(none)".to_string()
    } else {
        items.join(", ")
    }
}

/// `── title ──` section header.
fn section_header(title: &str) -> Line<'static> {
    let t = &THEME;
    Line::from(Span::styled(
        format!("── {title} ──"),
        Style::default().fg(t.muted),
    ))
}

/// Bullets for masking a secret at char count; the raw chars never live
/// in the frame.
fn masked(secret: &str) -> String {
    "•".repeat(secret.chars().count())
}

/// Styled label span for a field (create-form convention).
fn field_label_span(label: &str, active: bool) -> Span<'static> {
    let t = &THEME;
    let style = if active {
        Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(t.muted)
    };
    Span::styled(format!("  {label:<9}:"), style)
}

/// Styled value span for a text field with the edit point visible
/// mid-string (caret char reverse-styled; end-block when at the end).
fn text_value_span(value: &str, cursor: usize, active: bool) -> Span<'static> {
    let t = &THEME;
    if !active {
        return Span::styled(format!("[{value}]"), Style::default().fg(t.muted));
    }
    let (before, caret, after) = crate::ui::text_input::split_caret(value, cursor);
    if let Some(c) = caret {
        Span::styled(
            format!("[{before}{c}{after}]"),
            Style::default().fg(t.fg).add_modifier(Modifier::REVERSED),
        )
    } else {
        Span::styled(format!("[{before}█]"), Style::default().fg(t.fg))
    }
}

/// One labeled text-field line: `  Label:  [value█]`.
fn text_field_line(label: &str, value: &str, cursor: usize, active: bool) -> Line<'static> {
    Line::from(vec![
        field_label_span(label, active),
        Span::raw(" "),
        text_value_span(value, cursor, active),
    ])
}

/// The Secret editor line: masked chars, with the empty-means-keep hint in
/// edit mode and the required hint in add mode (both while focused).
///
/// The caret renders over the MASKED value at the same index — caret and
/// buffer stay in sync because `masked` is 1:1 per char.
fn secret_field_line(secret: &str, cursor: usize, active: bool, editing: bool) -> Line<'static> {
    let t = &THEME;
    let mut spans = vec![
        field_label_span("Secret", active),
        Span::raw(" "),
        text_value_span(&masked(secret), cursor, active),
    ];
    if active {
        let hint = if editing {
            "  (empty = keep the stored secret)"
        } else {
            "  (required — stored in the OS keyring)"
        };
        spans.push(Span::styled(hint, Style::default().fg(t.muted)));
    }
    Line::from(spans)
}

/// Lines for a list-type field (create-form convention): label + hint
/// line, one row per item (`▶ [item]`, highlighted only when the field is
/// focused), the input row while focused, and an empty-state hint.
fn list_field_lines(
    label: &str,
    items: &[String],
    input: &str,
    input_cursor: usize,
    active: bool,
    selected: usize,
    slot: Option<usize>,
) -> Vec<Line<'static>> {
    let t = &THEME;
    let hint = if slot.is_some() {
        "  (editing — Enter commits, empty Enter aborts)"
    } else if active {
        "  [Enter] add/edit · [Del] remove"
    } else {
        ""
    };
    let label_style = if active {
        Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(t.muted)
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(format!("  {label:<9}:"), label_style),
        Span::styled(hint, Style::default().fg(t.muted)),
    ])];
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
    if active {
        let (before, caret, after) = crate::ui::text_input::split_caret(input, input_cursor);
        let value = match caret {
            Some(c) => Span::styled(
                format!("  [{before}{c}{after}]"),
                Style::default().fg(t.warn).add_modifier(Modifier::REVERSED),
            ),
            None => Span::styled(format!("  [{before}█]"), Style::default().fg(t.warn)),
        };
        lines.push(Line::from(vec![Span::raw("    "), value]));
    }
    if items.is_empty() && !active {
        lines.push(Line::from(Span::styled(
            "    (none)",
            Style::default().fg(t.muted),
        )));
    }
    lines
}

/// Editor pane over the whole body (the master list is frozen while
/// editing; the chrome tab pill keeps the context).
fn render_editor(frame: &mut Frame, area: Rect, ed: &Editor) {
    let t = &THEME;
    let (title, mut lines) = match ed {
        Editor::Profile {
            index,
            cursor,
            name,
            description,
            env,
            field,
            env_input,
            env_sel,
            env_slot,
            ..
        } => {
            let title = if index.is_some() {
                " Edit environment profile "
            } else {
                " New environment profile "
            };
            let mut lines = vec![
                text_field_line("Name", name, *cursor, *field == PField::Name),
                text_field_line(
                    "Descr.",
                    description,
                    *cursor,
                    *field == PField::Description,
                ),
            ];
            lines.extend(list_field_lines(
                "Env",
                env,
                env_input,
                *cursor,
                *field == PField::Env,
                *env_sel,
                *env_slot,
            ));
            (title, lines)
        }
        Editor::Credential {
            index,
            cursor,
            name,
            description,
            env_key,
            secret,
            profiles,
            sandboxes,
            field,
            list_input,
            list_sel,
            list_slot,
            ..
        } => {
            let title = if index.is_some() {
                " Edit credential "
            } else {
                " New credential "
            };
            let editing = index.is_some();
            let mut lines = vec![
                text_field_line("Name", name, *cursor, *field == CField::Name),
                text_field_line(
                    "Descr.",
                    description,
                    *cursor,
                    *field == CField::Description,
                ),
                text_field_line("Env var", env_key, *cursor, *field == CField::EnvKey),
                secret_field_line(secret, *cursor, *field == CField::Secret, editing),
            ];
            lines.extend(list_field_lines(
                "Profiles",
                profiles,
                list_input,
                *cursor,
                *field == CField::Profiles,
                *list_sel,
                *list_slot,
            ));
            lines.extend(list_field_lines(
                "Sandboxes",
                sandboxes,
                list_input,
                *cursor,
                *field == CField::Sandboxes,
                *list_sel,
                *list_slot,
            ));
            (title, lines)
        }
    };
    let error = match ed {
        Editor::Profile { error, .. } | Editor::Credential { error, .. } => error,
    };
    if let Some(e) = error.as_deref() {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            format!("  ✗ {e}"),
            Style::default().fg(t.err),
        )));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "  [Tab] field · [Enter]/[Ctrl+Enter] save · [Del] remove row · [Esc] cancel",
        Style::default().fg(t.muted),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            title,
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(t.accent))
        .style(Style::default().bg(t.panel));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let p = Paragraph::new(lines).block(Block::default().style(Style::default().bg(t.panel)));
    frame.render_widget(p, inner);
}

// ---------------------------------------------------------------------------
// Tests — pure state-machine checks (the data layer modules have their own
// file-level tests); the flows that persist redirect the config dir.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::path::PathBuf;
    use std::sync::{Mutex, MutexGuard, PoisonError};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }

    fn ctrl_enter() -> KeyEvent {
        KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL)
    }

    fn credential(name: &str, env_key: &str) -> Credential {
        Credential {
            name: name.to_string(),
            description: "test".to_string(),
            env_key: env_key.to_string(),
            profiles: Vec::new(),
            sandboxes: Vec::new(),
        }
    }

    /// Serializes the persisting tests and redirects the config dir (the
    /// data layer resolves `XDG_CONFIG_HOME` at call time; there is no
    /// path-injection API). unsafe: edition 2024 flags env mutation — no
    /// other test in this binary reads the variable (they all use injected
    /// paths), so the guarded window is race-free here.
    static DISK: Mutex<()> = Mutex::new(());

    struct CfgDir {
        _lock: MutexGuard<'static, ()>,
        dir: PathBuf,
    }

    impl CfgDir {
        fn view(tag: &str) -> Self {
            let lock = DISK.lock().unwrap_or_else(PoisonError::into_inner);
            let dir =
                std::env::temp_dir().join(format!("msb-tui-uienv-{}-{tag}", std::process::id()));
            std::fs::remove_dir_all(&dir).ok();
            unsafe { std::env::set_var("XDG_CONFIG_HOME", &dir) };
            CfgDir { _lock: lock, dir }
        }
    }

    impl Drop for CfgDir {
        fn drop(&mut self) {
            unsafe { std::env::remove_var("XDG_CONFIG_HOME") };
            std::fs::remove_dir_all(&self.dir).ok();
        }
    }

    // ---- tabs / navigation (disk-free) ----

    #[test]
    fn opens_on_profiles_and_cycles_tabs() {
        let mut v = EnvironmentsView::default();
        assert_eq!(v.tab, EnvTab::Profiles);
        v.handle_key(key(KeyCode::Right));
        assert_eq!(v.tab, EnvTab::Credentials);
        v.handle_key(key(KeyCode::Left));
        assert_eq!(v.tab, EnvTab::Profiles);
        v.handle_key(key(KeyCode::Char('2')));
        assert_eq!(v.tab, EnvTab::Credentials);
        v.handle_key(key(KeyCode::Char('1')));
        assert_eq!(v.tab, EnvTab::Profiles);
    }

    #[test]
    fn esc_on_master_closes_the_view() {
        let mut v = EnvironmentsView::default();
        assert_eq!(v.handle_key(key(KeyCode::Esc)), EnvAction::Done);
    }

    #[test]
    fn master_selection_moves_and_clamps() {
        let mut v = EnvironmentsView::default();
        v.profiles = vec![
            Profile {
                name: "a".into(),
                description: String::new(),
                env: Vec::new(),
            },
            Profile {
                name: "b".into(),
                description: String::new(),
                env: Vec::new(),
            },
        ];
        // Selection is per-tab state; Down moves, Up clamps at 0, and an
        // empty credentials list ignores movement entirely.
        v.handle_key(key(KeyCode::Down));
        assert_eq!(v.selected[0], 1);
        v.handle_key(key(KeyCode::Down));
        assert_eq!(v.selected[0], 1);
        v.handle_key(key(KeyCode::Up));
        assert_eq!(v.selected[0], 0);
        v.handle_key(key(KeyCode::Up));
        assert_eq!(v.selected[0], 0);
        v.handle_key(key(KeyCode::Char('2')));
        v.handle_key(key(KeyCode::Down));
        assert_eq!(v.selected[1], 0, "empty list ignores movement");
    }

    // ---- profile editor (disk-free paths) ----

    #[test]
    fn profile_add_rejects_empty_name_inline_no_event() {
        let mut v = EnvironmentsView::default();
        assert_eq!(v.handle_key(key(KeyCode::Char('a'))), EnvAction::Continue);
        // Enter on the empty Name field: validation error, editor stays open.
        assert_eq!(v.handle_key(key(KeyCode::Enter)), EnvAction::Continue);
        let Some(Editor::Profile { error, .. }) = &v.editor else {
            panic!("editor must stay open");
        };
        assert!(
            error
                .as_deref()
                .unwrap_or_default()
                .contains("must not be empty"),
            "inline error set: {error:?}"
        );
    }

    #[test]
    fn duplicate_profile_name_rejected_inline() {
        let mut v = EnvironmentsView::default();
        v.profiles = vec![Profile {
            name: "dev".into(),
            description: String::new(),
            env: Vec::new(),
        }];
        v.handle_key(key(KeyCode::Char('a')));
        for c in "dev".chars() {
            v.handle_key(key(KeyCode::Char(c)));
        }
        // Validation passes; the collision against the OTHER row rejects
        // before any persist, keeping the editor open with a clear error.
        assert_eq!(v.handle_key(ctrl_enter()), EnvAction::Continue);
        let Some(Editor::Profile { error, .. }) = &v.editor else {
            panic!("editor must stay open");
        };
        assert!(
            error
                .as_deref()
                .unwrap_or_default()
                .contains("already exists"),
            "got: {error:?}"
        );
    }

    #[test]
    fn esc_discards_the_editor() {
        let mut v = EnvironmentsView::default();
        v.handle_key(key(KeyCode::Char('a')));
        v.handle_key(key(KeyCode::Esc));
        assert!(v.editor.is_none());
        assert!(v.profiles.is_empty());
    }

    #[test]
    fn list_field_add_edit_and_delete_row() {
        // Type + Enter adds; empty Enter on a selected row loads it for
        // in-place editing; Enter commits the replacement; Delete removes.
        let mut v = EnvironmentsView::default();
        v.handle_key(key(KeyCode::Char('a')));
        // Tab to the env list field (Name → Description → Env).
        v.handle_key(key(KeyCode::Tab));
        v.handle_key(key(KeyCode::Tab));
        for c in "A=1".chars() {
            v.handle_key(key(KeyCode::Char(c)));
        }
        v.handle_key(key(KeyCode::Enter));
        for c in "B=2".chars() {
            v.handle_key(key(KeyCode::Char(c)));
        }
        v.handle_key(key(KeyCode::Enter));
        let Some(Editor::Profile { env, .. }) = &v.editor else {
            panic!("editor still open");
        };
        assert_eq!(env, &["A=1".to_string(), "B=2".to_string()]);

        // Edit row 0 in place: select it, then empty Enter pulls it up into
        // the input row.
        v.handle_key(key(KeyCode::Up));
        v.handle_key(key(KeyCode::Enter));
        let Some(Editor::Profile {
            env_input,
            env_slot,
            ..
        }) = &mut v.editor
        else {
            panic!("editor still open");
        };
        assert_eq!(env_slot, &Some(0));
        assert_eq!(env_input, "A=1");
        // Backspace → "A=", type "2" → "A=2", Enter replaces row 0.
        v.handle_key(key(KeyCode::Backspace));
        v.handle_key(key(KeyCode::Char('2')));
        v.handle_key(key(KeyCode::Enter));
        let Some(Editor::Profile { env, env_slot, .. }) = &v.editor else {
            panic!("editor still open");
        };
        assert_eq!(env, &["A=2".to_string(), "B=2".to_string()]);
        assert_eq!(env_slot, &None);

        // Delete removes the selected row.
        v.handle_key(key(KeyCode::Delete));
        let Some(Editor::Profile { env, .. }) = &v.editor else {
            panic!("editor still open");
        };
        assert_eq!(env, &["B=2".to_string()]);
    }

    #[test]
    fn empty_enter_in_last_list_field_saves() {
        // The env list is the profile form's last field: Enter on an empty
        // input with nothing selected submits (create-form convention).
        let _cfg = CfgDir::view("last-field-save");
        let mut v = EnvironmentsView::new();
        v.handle_key(key(KeyCode::Char('a')));
        v.handle_key(key(KeyCode::Char('d')));
        v.handle_key(key(KeyCode::Tab));
        v.handle_key(key(KeyCode::Tab));
        let action = v.handle_key(key(KeyCode::Enter));
        assert!(
            matches!(&action, EnvAction::SaveProfile(p) if p.name == "d"),
            "empty Enter on the last field saves: {action:?}"
        );
        assert!(v.editor.is_none());
        assert_eq!(v.profiles.len(), 1);
    }

    // ---- credential editor (disk-free paths) ----

    #[test]
    fn credential_add_requires_a_secret() {
        let mut v = EnvironmentsView::default();
        v.handle_key(key(KeyCode::Char('2')));
        v.handle_key(key(KeyCode::Char('a')));
        for c in "gh".chars() {
            v.handle_key(key(KeyCode::Char(c)));
        }
        v.handle_key(key(KeyCode::Tab));
        v.handle_key(key(KeyCode::Tab));
        for c in "GH_TOKEN".chars() {
            v.handle_key(key(KeyCode::Char(c)));
        }
        // Secret untouched → the add path needs a non-empty secret.
        assert_eq!(v.handle_key(ctrl_enter()), EnvAction::Continue);
        let Some(Editor::Credential { error, .. }) = &v.editor else {
            panic!("editor must stay open");
        };
        assert!(
            error.as_deref().unwrap_or_default().contains("secret"),
            "got: {error:?}"
        );
    }

    #[test]
    fn credential_rename_is_rejected_keeps_keyring_coherent() {
        let mut v = EnvironmentsView::default();
        v.handle_key(key(KeyCode::Char('2')));
        v.credentials = vec![credential("gh", "GH_TOKEN")];
        v.handle_key(key(KeyCode::Char('e')));
        for _ in 0..2 {
            v.handle_key(key(KeyCode::Backspace));
        }
        for c in "git".chars() {
            v.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(v.handle_key(ctrl_enter()), EnvAction::Continue);
        let Some(Editor::Credential { error, .. }) = &v.editor else {
            panic!("editor must stay open");
        };
        assert!(
            error
                .as_deref()
                .unwrap_or_default()
                .contains("keyring secret"),
            "rename rejected: {error:?}"
        );
        // Nothing persisted: the in-memory row still holds the old name.
        assert_eq!(v.credentials[0].name, "gh");
    }

    // ---- render: secret masking (disk-free) ----

    #[test]
    fn secret_masks_bullets_while_state_keeps_real_chars() {
        let mut v = EnvironmentsView::default();
        v.handle_key(key(KeyCode::Char('2')));
        v.handle_key(key(KeyCode::Char('a')));
        for c in "gh".chars() {
            v.handle_key(key(KeyCode::Char(c)));
        }
        v.handle_key(key(KeyCode::Tab));
        v.handle_key(key(KeyCode::Tab));
        for c in "GH_TOKEN".chars() {
            v.handle_key(key(KeyCode::Char(c)));
        }
        v.handle_key(key(KeyCode::Tab));
        // Chrome text carries its own '•' separators (title bar, nav
        // legend), so the mask is counted as a delta while typing. The
        // secret is still empty here — one bullet per typed char only.
        let bullets_of = |view: &EnvironmentsView| -> (usize, String) {
            let mut terminal = Terminal::new(TestBackend::new(120, 34)).unwrap();
            terminal.draw(|f| render(f, f.area(), view)).unwrap();
            let buf = terminal.backend().buffer().clone();
            let mut bullets = 0;
            let mut text = String::new();
            for y in 0..buf.area.height {
                for x in 0..buf.area.width {
                    let symbol = buf[(x, y)].symbol();
                    text.push_str(symbol);
                    if symbol == "•" {
                        bullets += 1;
                    }
                }
            }
            (bullets, text)
        };
        let (baseline, _) = bullets_of(&v);
        for c in "s3cr3t".chars() {
            v.handle_key(key(KeyCode::Char(c)));
        }
        let (bullets, text) = bullets_of(&v);
        assert_eq!(
            bullets - baseline,
            "s3cr3t".chars().count(),
            "masked at char count"
        );
        assert!(
            !text.contains("s3cr3t"),
            "raw secret never rendered: {text}"
        );
    }

    // ---- save flows (persist: guarded by CfgDir) ----

    #[test]
    fn profile_add_emits_and_persists() {
        let _cfg = CfgDir::view("profile-add");
        let mut v = EnvironmentsView::new();
        assert!(v.profiles.is_empty());
        v.handle_key(key(KeyCode::Char('a')));
        for c in "dev".chars() {
            v.handle_key(key(KeyCode::Char(c)));
        }
        let action = v.handle_key(key(KeyCode::Enter));
        let profile = match &action {
            EnvAction::SaveProfile(p) => *p.clone(),
            other => panic!("expected SaveProfile, got {other:?}"),
        };
        assert_eq!(profile.name, "dev");
        assert!(v.editor.is_none());
        assert_eq!(v.profiles, vec![profile.clone()]);
        match v.status {
            Some((ref m, false)) => assert!(m.contains("saved")),
            _ => panic!("ok status after save"),
        }
        assert_eq!(crate::environments::load_profiles().unwrap(), v.profiles);
    }

    #[test]
    fn profile_edit_replaces_the_row_and_persists() {
        let _cfg = CfgDir::view("profile-edit");
        let mut v = EnvironmentsView::new();
        v.profiles = vec![Profile {
            name: "dev".into(),
            description: "d".into(),
            env: vec!["A=1".into()],
        }];
        v.handle_key(key(KeyCode::Char('e')));
        // Rename the profile (profiles may rename — templates referencing
        // the old name surface the change at next create, visibly).
        for _ in 0..3 {
            v.handle_key(key(KeyCode::Backspace));
        }
        for c in "dev2".chars() {
            v.handle_key(key(KeyCode::Char(c)));
        }
        let action = v.handle_key(ctrl_enter());
        assert!(
            matches!(&action, EnvAction::SaveProfile(p) if p.name == "dev2" && p.env == ["A=1"]),
            "edit emits the updated profile: {action:?}"
        );
        // The row is replaced in place, env entries carried over.
        assert_eq!(v.selected[0], 0);
        assert_eq!(crate::environments::load_profiles().unwrap(), v.profiles);
    }

    #[test]
    fn profile_delete_needs_confirm_then_emits_name() {
        let _cfg = CfgDir::view("profile-del");
        let mut v = EnvironmentsView::new();
        v.profiles = vec![Profile {
            name: "dev".into(),
            description: String::new(),
            env: Vec::new(),
        }];
        v.handle_key(key(KeyCode::Char('d')));
        assert!(v.confirm.is_some(), "confirm armed");
        assert_eq!(v.handle_key(key(KeyCode::Char('n'))), EnvAction::Continue);
        assert!(v.confirm.is_none());
        assert_eq!(v.profiles.len(), 1, "cancel keeps the entry");

        v.handle_key(key(KeyCode::Char('d')));
        let action = v.handle_key(key(KeyCode::Char('y')));
        assert_eq!(action, EnvAction::DeleteProfile("dev".to_string()));
        assert!(v.profiles.is_empty());
        assert!(
            crate::environments::load_profiles().unwrap().is_empty(),
            "delete persisted"
        );
    }

    #[test]
    fn credential_save_none_vs_some_semantics() {
        let _cfg = CfgDir::view("cred-save");
        let mut v = EnvironmentsView::new();
        v.handle_key(key(KeyCode::Char('2')));
        v.credentials = vec![credential("gh", "GH_TOKEN")];

        // Edit with an untouched secret field: None = keep the stored value,
        // and there is no keyring work → no pending marker.
        v.handle_key(key(KeyCode::Char('e')));
        let action = v.handle_key(ctrl_enter());
        assert!(
            matches!(&action, EnvAction::SaveCredential(c, None) if c.name == "gh"),
            "unchanged secret → None: {action:?}"
        );
        assert!(v.async_pending.is_none());
        assert!(matches!(v.status, Some((_, false))));

        // Edit with a typed secret: Some carries it, the pending marker is
        // set, and the metadata is already persisted.
        v.handle_key(key(KeyCode::Char('e')));
        v.handle_key(key(KeyCode::Tab));
        v.handle_key(key(KeyCode::Tab));
        v.handle_key(key(KeyCode::Tab));
        for c in "n3w".chars() {
            v.handle_key(key(KeyCode::Char(c)));
        }
        let action = v.handle_key(ctrl_enter());
        assert!(
            matches!(&action, EnvAction::SaveCredential(_, Some(s)) if s == "n3w"),
            "typed secret → Some: {action:?}"
        );
        assert_eq!(
            v.async_pending.as_deref().map(|p| p.contains("gh")),
            Some(true)
        );

        // The event loop reports the async result: marker cleared, message
        // shown (error styling on failure).
        v.note_async_result(false, "keyring locked");
        assert!(v.async_pending.is_none());
        assert_eq!(v.status.as_ref().map(|(_, e)| *e), Some(true));

        match crate::credentials::load_credentials() {
            Ok(file) => assert_eq!(file.credentials[0].name, "gh"),
            Err(e) => panic!("metadata readable: {e}"),
        }
    }

    #[test]
    fn credential_delete_emits_with_keyring_pending() {
        let _cfg = CfgDir::view("cred-del");
        let mut v = EnvironmentsView::new();
        v.credentials = vec![credential("gh", "GH_TOKEN")];
        v.handle_key(key(KeyCode::Char('2')));
        v.handle_key(key(KeyCode::Char('d')));
        let action = v.handle_key(key(KeyCode::Char('y')));
        assert_eq!(action, EnvAction::DeleteCredential("gh".to_string()));
        assert!(v.credentials.is_empty());
        assert_eq!(
            v.async_pending.as_deref().map(|p| p.contains("deleting")),
            Some(true)
        );
        v.note_async_result(true, "secret deleted");
        assert!(v.async_pending.is_none());
        assert!(
            crate::credentials::load_credentials()
                .unwrap()
                .credentials
                .is_empty(),
            "metadata delete persisted"
        );
    }

    // ---- reload ----

    #[test]
    fn reload_rereads_the_files_and_clamps_selection() {
        let _cfg = CfgDir::view("reload");
        let mut v = EnvironmentsView::new();
        assert!(v.profiles.is_empty());
        let seeded = vec![Profile {
            name: "dev".into(),
            description: String::new(),
            env: Vec::new(),
        }];
        crate::environments::save_profiles(&seeded).unwrap();
        // Stale selection clamps into range on reload.
        v.selected[0] = 9;
        v.reload();
        assert_eq!(v.profiles, seeded);
        assert_eq!(v.selected[0], 0);
    }
}
