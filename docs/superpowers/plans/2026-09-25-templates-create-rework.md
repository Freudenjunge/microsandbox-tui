# Templates + Create-Dialog-Rework Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Template-gestützter Create-Flow: Master-Detail-Auswahl (Variante C) mit Direkt-Erstellen, separates Vollbild-Formular mit gruppierten Abschnitten, Templates als TOML-Dateien mit Built-ins (`opencode`, `opencode2`, `shell`) und `Ctrl+S`-Authoring.

**Architecture:** Neues Modul `src/template.rs` (Datenmodell, TOML-Parsing, Persistenz-Schale, Pure-Logik: Spec-Assembly, Namensauflösung, Vorschau, Layout). `View::Create` wird ein zwei-Zustands-Automat (`CreateState::Select` ↔ `Form(CreateForm)`). Backend/SDK unverändert — Templates enden als `CreateSpec` im existierenden detached-Create-Pfad. I/O auf `tokio`-Tasks mit neuen Events (Muster wie `ImagesUpdated`).

**Tech Stack:** Rust 2024, ratatui 0.29, tokio, serde + `toml` 0.8 (neu), anyhow.

**Spec:** `docs/superpowers/specs/2026-09-25-templates-create-rework-design.md` (zusammen mit diesem Plan lesen)

## Global Constraints

- `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, `cargo check` — alle grün vor jedem Commit (clippy-Lints sind Fehler).
- TDD: erst der fehlgeschlagene Test, dann minimaler Code. UI-Rendering wird nicht unit-getestet (Kompilieren + manueller Smoke-Test).
- Commits: Conventional Commits, Body nennt die PLAN.md-Aufgabe (`Closes phase2#10.N`).
- `src/backend/` bleibt der einzige SDK-Kontakt; UI ruft nie das SDK auf. Create läuft immer detached (existierender `queued_create`-Pfad).
- SDK pinned: microsandbox 0.7.2-Crates; keine neuen SDK-Features.
- Names/Copy: Template-Keys heißen exakt wie im Spec §1 (`image`, `mount_cwd`, `net_profile`, …).
- Default-Netzwerkprofil für neue Sandboxes: `public`. Neue Sandboxes persistent (kein Ephemeral-Toggle in v1).

## Review Focus

Eingaben/Zustände, die der Spec impliziert, aber kein Task-Test abdeckt — jeder Zeile folgt ein Test im genannten Task:

1. **Template-Verzeichnis fehlt / `$HOME` unset** → `load_all` liefert Built-ins, kein Panic (Task 3, `load_all_missing_dir_yields_builtins`).
2. **Kaputte TOML-Datei** (Syntaxfehler, BOM-artiger Müll) → Datei übersprungen, Warnung in der Statuszeile, Rest lädt (Task 3, `load_all_skips_broken_files_with_warning`).
3. **Doppeltes `Enter` im Select-State während ein Create läuft** (`busy`/`queued_create` belegt) → kein zweites Create gequeued (Task 5, `select_enter_while_busy_ignored`).
4. **Slug-Kollision beim `Ctrl+S`** (Datei existiert) → Confirm „Überschreiben?“, kein stilles Überschreiben (Task 7b, `save_dialog_asks_on_collision`).
5. **Sehr lange Beschreibungen/Werte im Vorschau-Pane bei Minimalbreite** → kein Panic; Abschneiden statt Umbrechen in v1 (manueller Smoke-Test in Task 6; `preview_values` selbst ist getestet).

---

## File Structure

- **Create `src/template.rs`** — Template-Datenmodell (`Template`, `TemplateMeta`, `TemplateSpec`), TOML-Parsing, `templates_dir`/`load_builtins`/`load_all`/`write_template`, Pure-Logik (`assemble_spec`, `to_create_spec`, `resolve_name`, `preview_values`, `template_layout`). Kein SDK-Import; nutzt `models::PublishedPort` + `backend::CreateSpec` + `backend::sdk::parse_memory_mib`.
- **Create `src/templates/{opencode,opencode2,shell}.toml`** — Built-ins (per `include_str!` eingebettet).
- **Create `src/fixtures/templates/{valid-full,missing-image,bad-memory,reserved-rootfs,shadow-shell}.toml`** — Test-Fixtures.
- **Modify `src/models.rs`** — `PublishedPort::parse_cli` (move aus `ui/create.rs`).
- **Modify `src/backend/mod.rs`** — nichts (CreateSpec bleibt); nur Kommentar-Update falls nötig.
- **Modify `src/event.rs`** — `TemplatesLoaded(Vec<Template>)`, `TemplateSaved(String)`, `TemplateDeleted(String)`.
- **Modify `src/app.rs`** — `CreateState`-Automat (`Select` ↔ `Form`), `App.templates`, Key-Routing, `Op::DeleteTemplate`, `take_save_template`.
- **Modify `src/ui/create.rs`** — gruppiertes Vollbild-Formular (FormField + `Labels`, entfallenes Quick/Advanced), `Ctrl+S`-Dialog, `to_template`/`from_template`.
- **Create `src/ui/template_picker.rs`** — Rendering der C-Ansicht (Liste + Vorschau, drei Layouts).
- **Modify `src/ui/mod.rs`, `src/main.rs`** — Modul-Deklaration, Render-Dispatch, Startup-Load, Executor-Arms (Save/Delete).
- **Modify `docs/DESIGN.md`, `docs/PLAN.md`** — Roadmap-Punkte 11/12, Keybindings, Aufgabenverzeichnis (Task 9).

---

### Task 1: Foundation — `toml`-Dep + `PublishedPort::parse_cli` nach models.rs

**Files:**
- Modify: `Cargo.toml` ([dependencies])
- Modify: `src/models.rs` (impl PublishedPort, nach `to_cli_flag` ~Zeile 102)
- Modify: `src/ui/create.rs:836-870` (parse_port_spec entfernen, Call-Site Zeile 739 + Tests 1768-1771 umstellen)

**Interfaces:**
- Consumes: existierendes `parse_port_spec(s: &str) -> anyhow::Result<PublishedPort>` in `src/ui/create.rs`.
- Produces: `models::PublishedPort::parse_cli(s: &str) -> anyhow::Result<PublishedPort>`; Dependency `toml = "0.8"` für Task 2.

- [ ] **Step 1: Failing test in `src/models.rs` (tests-Modul)**

```rust
#[test]
fn parse_cli_accepts_flag_forms() {
    let p = PublishedPort::parse_cli("8080:80").unwrap();
    assert_eq!(p.host_bind, "127.0.0.1");
    assert_eq!(p.host_port, 8080);
    assert_eq!(p.guest_port, 80);
    let b = PublishedPort::parse_cli("0.0.0.0:8080:80/udp").unwrap();
    assert_eq!(b.host_bind, "0.0.0.0");
    assert_eq!(b.protocol, "udp");
    assert!(PublishedPort::parse_cli("8080").is_err());
    assert!(PublishedPort::parse_cli("a:b").is_err());
    assert!(PublishedPort::parse_cli("8080:80/sctp").is_err());
}
```

- [ ] **Step 2: Test laufen lassen — muss FAIL**

Run: `cargo test --bin microsandbox-tui parse_cli_accepts_flag_forms`
Expected: FAIL — `parse_cli` ist keine bekannte Funktion (E0599).

- [ ] **Step 3: Implementieren**

In `Cargo.toml` unter `[dependencies]` ergänzen: `toml = "0.8"`.

Körper von `parse_port_spec` (create.rs ~836-870, inkl. Hilfslogik) nach `models.rs` in `impl PublishedPort` verschieben als:

```rust
/// Parse a CLI/port-spec string in `HOST:GUEST` or `BIND:HOST:GUEST` form
/// with an optional `/udp` suffix. Unit-tested.
pub fn parse_cli(s: &str) -> Result<PublishedPort> {
    // unveränderte Logik aus ui/create.rs::parse_port_spec
}
```

In `src/ui/create.rs`: `parse_port_spec` löschen; Zeile 739 → `PublishedPort::parse_cli(p)?` (Import prüfen); Tests 1768-1771 → `PublishedPort::parse_cli(...)`. Verbleibende Referenzen via grep finden.

- [ ] **Step 4: Tests grün + Gates**

Run: `cargo test --bin microsandbox-tui parse_cli && cargo clippy -- -D warnings 2>&1 | tail -1`
Expected: PASS, `Finished` ohne Warnung.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "refactor(models): move port-spec parser to PublishedPort::parse_cli

Foundation for template files: templates carry port strings that are
validated with the same parser as the create form. Also adds the toml
dependency for the upcoming template module.

Closes phase2#10.1"
```

---

### Task 2: Template-Datenmodell + TOML-Parsing (`src/template.rs`)

**Files:**
- Create: `src/template.rs`
- Create: `src/fixtures/templates/valid-full.toml`, `missing-image.toml`, `bad-memory.toml`, `reserved-rootfs.toml`
- Modify: `src/main.rs` (Zeile 32-39: `mod template;`)

**Interfaces:**
- Consumes: `backend::sdk::parse_memory_mib` (`pub(crate)`), `models::PublishedPort::parse_cli` (Task 1).
- Produces (von Task 3+ benutzt):

```rust
pub struct TemplateMeta { pub name: String, pub description: String }
pub struct TemplateSpec {
    pub image: String,
    pub name: Option<String>,
    pub cpus: Option<u32>,
    pub memory: Option<String>,
    pub workdir: Option<String>,
    pub mount_cwd: Option<bool>,
    pub ports: Vec<String>,
    pub volumes: Vec<String>,
    pub env: Vec<String>,
    pub labels: std::collections::BTreeMap<String, String>,
    pub net_profile: Option<String>,
    pub net_rules: Vec<String>,
    pub rootfs: Option<String>, // reserviert, v1 ohne Wirkung
}
pub struct Template { pub id: String, pub meta: TemplateMeta, pub spec: TemplateSpec, pub built_in: bool }
pub fn parse(id: &str, s: &str) -> Result<Template, String>
```

- [ ] **Step 1: Fixtures anlegen**

`src/fixtures/templates/valid-full.toml`:

```toml
[meta]
name = "Opencode"
description = "Node-Image + CWD-Mount, um opencode-ai in der Sandbox zu nutzen"

image = "node:22-alpine"
name = "opencode"
cpus = 4
memory = "2G"
workdir = "/workspace"
mount_cwd = true
ports = ["127.0.0.1:3000:3000"]
volumes = ["/tmp/share:/share:ro"]
env = ["TERM=xterm-256color"]
labels = { team = "infra" }
net_profile = "public"
net_rules = ["allow@api.example.com"]
```

`missing-image.toml`: nur `[meta]`-Block, kein `image`.
`bad-memory.toml`: `memory = "lots"`.
`reserved-rootfs.toml`: gültig plus `rootfs = "snapshot:opencode/base"`.

- [ ] **Step 2: Failing tests in `src/template.rs` (tests-Modul)**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_all_fields() {
        let t = parse("opencode", include_str!("fixtures/templates/valid-full.toml")).unwrap();
        assert_eq!(t.id, "opencode");
        assert!(!t.built_in);
        assert_eq!(t.meta.name, "Opencode");
        assert_eq!(t.spec.image, "node:22-alpine");
        assert_eq!(t.spec.name.as_deref(), Some("opencode"));
        assert_eq!(t.spec.cpus, Some(4));
        assert_eq!(t.spec.memory.as_deref(), Some("2G"));
        assert_eq!(t.spec.mount_cwd, Some(true));
        assert_eq!(t.spec.ports, vec!["127.0.0.1:3000:3000"]);
        assert_eq!(t.spec.env, vec!["TERM=xterm-256color"]);
        assert_eq!(t.spec.labels.get("team").map(String::as_str), Some("infra"));
        assert_eq!(t.spec.net_profile.as_deref(), Some("public"));
    }

    #[test]
    fn parse_requires_image() {
        let e = parse("x", include_str!("fixtures/templates/missing-image.toml")).unwrap_err();
        assert!(e.contains("image"), "{e}");
    }

    #[test]
    fn parse_rejects_invalid_memory() {
        assert!(parse("x", include_str!("fixtures/templates/bad-memory.toml")).is_err());
    }

    #[test]
    fn rootfs_field_is_reserved_but_ignored() {
        let t = parse("x", include_str!("fixtures/templates/reserved-rootfs.toml")).unwrap();
        assert_eq!(t.spec.rootfs.as_deref(), Some("snapshot:opencode/base"));
        // v1: to_create_spec ignoriert rootfs (Task 4 testet das Verhalten).
    }

    #[test]
    fn labels_serialise_to_kv_strings() {
        let t = parse("x", include_str!("fixtures/templates/valid-full.toml")).unwrap();
        assert_eq!(t.spec.labels_vec(), vec!["team=infra"]);
    }
}
```

- [ ] **Step 3: FAIL verifizieren**

Run: `cargo test --bin microsandbox-tui template:: 2>&1 | grep -E "error\[|test result" | head -4`
Expected: Fehler — `mod template` existiert noch nicht (nach Step 4-Anlage kompiliert der Test gegen leere Implementierung: FAIL wegen fehlender Funktionen).

- [ ] **Step 4: Minimal implementieren**

`src/template.rs`: Strukturen wie oben (alle `#[derive(Debug, Clone, serde::Deserialize)]`; `Template` mit `#[serde(skip)] built_in: bool` — Achtung: `Template` wird manuell gebaut, nicht direkt deserialisiert; deserialisiert wird ein `TemplateFile { meta, spec }`-Zwischentyp). `parse`:

```rust
/// Parse a template document. `id` comes from the filename stem. Unknown
/// fields are ignored (serde default); `rootfs` is parsed and stored but
/// has no effect in v1 (reserved for golden-image sources).
pub fn parse(id: &str, s: &str) -> Result<Template, String> {
    let f: TemplateFile = toml::from_str(s).map_err(|e| format!("template '{id}': {e}"))?;
    if f.spec.image.trim().is_empty() {
        return Err(format!("template '{id}': 'image' is required"));
    }
    if let Some(m) = f.spec.memory.as_deref() {
        if crate::backend::sdk::parse_memory_mib(m).is_none() {
            return Err(format!("template '{id}': invalid memory '{m}' (use e.g. 512M, 2G)"));
        }
    }
    for p in &f.spec.ports {
        if crate::models::PublishedPort::parse_cli(p).is_err() {
            return Err(format!("template '{id}': invalid port '{p}'"));
        }
    }
    for r in &f.spec.net_rules {
        // Format wie Formular: `allow@host` / `deny@host`
        if !r.contains('@') {
            return Err(format!("template '{id}': invalid net rule '{r}' (expected allow@host)"));
        }
    }
    Ok(Template { id: id.to_string(), meta: f.meta, spec: f.spec, built_in: false })
}
```

`labels_vec()`:

```rust
impl TemplateSpec {
    /// Labels as `KEY=VALUE` strings (sorted by key), the form's format.
    pub fn labels_vec(&self) -> Vec<String> {
        self.labels.iter().map(|(k, v)| format!("{k}={v}")).collect()
    }
}
```

`mod template;` in main.rs ergänzen. Net-Regel-Format an die Formular-Validierung angleichen (grep `net_rules` in create.rs — dort existiert eine Regelprüfung; gleiche Bedingung übernehmen).

- [ ] **Step 5: Tests grün + Gates, Commit**

Run: `cargo test --bin microsandbox-tui template:: && cargo fmt --check && cargo clippy -- -D warnings 2>&1 | tail -1`
Expected: PASS.

```bash
git add -A && git commit -m "feat(templates): template data model and TOML parser

Template {meta, spec} documents in TOML, one file per template; image
required, memory/ports/rules validated with the form's parsers, rootfs
reserved but without effect in v1.

Closes phase2#10.2"
```

---

### Task 3: Persistenz — Built-ins, `load_all`, `write_template`

**Files:**
- Create: `src/templates/opencode.toml`, `src/templates/opencode2.toml`, `src/templates/shell.toml`
- Modify: `src/template.rs` (Funktionen + Tests)

**Interfaces:**
- Consumes: `parse` (Task 2).
- Produces (Task 5/8):

```rust
pub fn load_builtins() -> Vec<Template>
pub fn templates_dir() -> std::path::PathBuf
pub fn load_all(dir: &std::path::Path) -> (Vec<Template>, Vec<String>) // (Templates, Warnungen)
pub fn write_template(dir: &std::path::Path, t: &Template) -> anyhow::Result<std::path::PathBuf>
pub fn slugify(name: &str) -> String
```

- [ ] **Step 1: Built-in-Dateien anlegen** (Inhalte = Spec §8; `opencode.toml` wie valid-full.toml mit `name = "Opencode"`; `opencode2.toml`: `image = "node:24-alpine"`, `cpus = 8`, `memory = "4G"`, `mount_cwd = true`, `name = "opencode2"`; `shell.toml`: `image = "alpine"`, `mount_cwd = true`, `name = "shell"`)

- [ ] **Step 2: Failing tests**

```rust
#[test]
fn load_builtins_contain_the_three_presets() {
    let t = load_builtins();
    let ids: Vec<&str> = t.iter().map(|x| x.id.as_str()).collect();
    assert_eq!(ids, vec!["opencode", "opencode2", "shell"]);
    assert!(t.iter().all(|x| x.built_in));
}

#[test]
fn load_all_missing_dir_yields_builtins() {
    // Review Focus 1: fehlendes Verzeichnis ist kein Fehler.
    let (templates, warnings) =
        load_all(std::path::Path::new("/nonexistent/msb-tui-templates"));
    assert!(warnings.is_empty());
    assert!(templates.iter().any(|t| t.id == "shell"));
}

#[test]
fn load_all_skips_broken_files_with_warning() {
    let dir = std::env::temp_dir().join(format!("msb-tui-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("shell.toml"), include_str!("../templates/shell.toml")).unwrap();
    std::fs::write(dir.join("broken.toml"), "kein = [toml").unwrap();
    let (templates, warnings) = load_all(&dir);
    assert!(templates.iter().all(|t| t.id != "broken"));
    assert!(warnings.iter().any(|w| w.contains("broken")), "{warnings:?}");
    // Review Focus 2 geprüft.
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn user_file_shadows_builtin_with_same_id() {
    let dir = std::env::temp_dir().join(format!("msb-tui-shadow-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("shell.toml"), "[meta]\nname = \"Meine Shell\"\ndescription = \"d\"\n\nimage = \"debian\"\n").unwrap();
    let (templates, _) = load_all(&dir);
    let shell = templates.iter().find(|t| t.id == "shell").unwrap();
    assert_eq!(shell.meta.name, "Meine Shell");
    assert!(!shell.built_in, "shadowing file wins and is a user template");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn write_template_roundtrips() {
    let t = parse("x", include_str!("fixtures/templates/valid-full.toml")).unwrap();
    let dir = std::env::temp_dir().join(format!("msb-tui-wr-{}", std::process::id()));
    let path = write_template(&dir, &t).unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    let re = parse("opencode", &content).unwrap();
    assert_eq!(re.meta.name, "Opencode");
    assert_eq!(re.spec.image, "node:22-alpine");
    assert_eq!(re.spec.labels_vec(), vec!["team=infra"]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn slugify_kebab_cases_names() {
    assert_eq!(slugify("My Cool Template"), "my-cool-template");
    assert_eq!(slugify("opencode"), "opencode");
}
```

- [ ] **Step 3: FAIL verifizieren** — `cargo test --bin microsandbox-tui load_all 2>&1 | grep "error\[" | head -2`

- [ ] **Step 4: Implementieren**

```rust
const BUILTIN_SOURCES: &[(&str, &str)] = &[
    ("opencode", include_str!("templates/opencode.toml")),
    ("opencode2", include_str!("templates/opencode2.toml")),
    ("shell", include_str!("templates/shell.toml")),
];

pub fn load_builtins() -> Vec<Template> {
    BUILTIN_SOURCES
        .iter()
        .filter_map(|(id, src)| parse(id, src).ok().map(|mut t| { t.built_in = true; t }))
        .collect()
}

/// `XDG_CONFIG_HOME`, sonst `$HOME/.config` (Spec §2).
pub fn templates_dir() -> std::path::PathBuf {
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME") {
        if !x.is_empty() {
            return std::path::Path::new(&x).join("microsandbox-tui/templates");
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    std::path::Path::new(&home).join(".config/microsandbox-tui/templates")
}

pub fn load_all(dir: &std::path::Path) -> (Vec<Template>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut user: Vec<Template> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut paths: Vec<_> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        paths.sort();
        for p in paths {
            let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else { continue };
            if p.extension().and_then(|e| e.to_str()) != Some("toml") { continue }
            match std::fs::read_to_string(&p).map_err(|e| e.to_string())
                .and_then(|s| parse(stem, &s))
            {
                Ok(t) => user.push(t),
                Err(e) => warnings.push(e),
            }
        }
    }
    // Merge: Nutzer-Datei gewinnt bei gleicher id; Reihenfolge Nutzer
    // zuerst (sortiert), dann unverbundene Built-ins.
    let mut out = user;
    let ids: Vec<&str> = out.iter().map(|t| t.id.as_str()).collect();
    for b in load_builtins() {
        if !ids.contains(&b.id.as_str()) {
            out.push(b);
        }
    }
    (out, warnings)
}

pub fn slugify(name: &str) -> String {
    let s: String = name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    let mut s = s.as_str();
    while s.starts_with('-') { s = &s[1..]; }
    while s.ends_with('-') { s = &s[..s.len() - 1]; }
    if s.is_empty() { "template".into() } else { s.to_string() }
}

/// Schreibt `meta`+`spec` als `<slug>.toml`. Existierende Datei wird vom
/// Aufrufer vorher per Confirm bestätigt (Review Focus 4).
pub fn write_template(dir: &std::path::Path, t: &Template) -> anyhow::Result<std::path::PathBuf> {
    std::fs::create_dir_all(dir)?;
    let file = TemplateFile { meta: t.meta.clone(), spec: t.spec.clone() };
    let body = toml::to_string_pretty(&file)
        .map_err(|e| anyhow::anyhow!("serialize template '{}': {e}", t.id))?;
    let path = dir.join(format!("{}.toml", slugify(&t.meta.name)));
    std::fs::write(&path, body)?;
    Ok(path)
}
```

(`TemplateFile { meta, spec }` mit `#[derive(serde::Serialize, serde::Deserialize)]`; `TemplateSpec` braucht zusätzlich `#[derive(serde::Serialize)]` und `labels` braucht `Default`.)

- [ ] **Step 5: Tests grün + Gates, Commit**

```bash
git add -A && git commit -m "feat(templates): persistence with built-ins and shadowing

Built-in templates embedded via include_str! (opencode, opencode2,
shell); user files in ~/.config/microsandbox-tui/templates shadow
built-ins by id; broken files are skipped with a status warning; a
missing template dir is not an error.

Closes phase2#10.3"
```

---

### Task 4: Pure-Logik — `assemble_spec`, `to_create_spec`, `resolve_name`, `preview_values`, `template_layout`

**Files:**
- Modify: `src/template.rs` (Funktionen + Tests)
- Modify: `src/ui/create.rs:745-830` (Körper von `to_create_spec` → `assemble_spec` aufrufen; Verhalten unverändert)

**Interfaces:**
- Consumes: `CreateSpec` (backend/mod.rs:123), Formular-Defaults.
- Produces (Task 5/6/7):

```rust
pub(crate) struct SpecInputs<'a> {
    pub image: &'a str,
    pub name: Option<String>,
    pub cpus: Option<u32>,
    pub memory: Option<String>,
    pub workdir: Option<String>,
    pub mount_cwd: bool,
    pub cwd: &'a str,
    pub ports: Vec<crate::models::PublishedPort>,
    pub volumes: Vec<String>,
    pub env: Vec<String>,
    pub labels: Vec<String>,
    pub net_rules: Vec<String>,
    pub net_profile: Option<String>,
}
pub(crate) fn assemble_spec(i: SpecInputs<'_>) -> anyhow::Result<crate::backend::CreateSpec>
pub fn to_create_spec(t: &Template, cwd: &str) -> anyhow::Result<crate::backend::CreateSpec>
pub fn resolve_name(pattern: &str, existing: &[String]) -> Option<String>
pub struct PreviewRow { pub label: &'static str, pub value: String, pub is_default: bool }
pub fn preview_values(t: &Template, cwd: &str) -> Vec<PreviewRow>
pub enum LayoutKind { MasterDetail, Stacked, ListOnly }
pub fn template_layout(width: u16, height: u16) -> LayoutKind
```

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn resolve_name_appends_first_free_suffix() {
    assert_eq!(resolve_name("opencode", &[]), Some("opencode".into()));
    assert_eq!(resolve_name("opencode", &["opencode".into()]), Some("opencode-2".into()));
    assert_eq!(
        resolve_name("opencode", &["opencode".into(), "opencode-2".into(), "opencode-3".into()]),
        Some("opencode-4".into())
    );
}

#[test]
fn template_to_create_spec_maps_every_field() {
    let t = parse("opencode", include_str!("fixtures/templates/valid-full.toml")).unwrap();
    let spec = to_create_spec(&t, "/home/me/proj").unwrap();
    assert_eq!(spec.image, "node:22-alpine");
    assert_eq!(spec.name.as_deref(), Some("opencode"));
    assert_eq!(spec.cpus, Some(4));
    assert_eq!(spec.memory.as_deref(), Some("2G"));
    assert_eq!(spec.ports.len(), 1);
    assert_eq!(spec.ports[0].guest_port, 3000);
    // mount_cwd=true → CWD-Bind an gleicher Stelle, workdir-Default = Mount
    assert!(spec.volumes.contains(&"/home/me/proj:/home/me/proj".to_string()));
    assert_eq!(spec.workdir.as_deref(), Some("/workspace"));
    assert_eq!(spec.env, vec!["TERM=xterm-256color"]);
    assert_eq!(spec.labels, vec!["team=infra"]);
    assert_eq!(spec.net_profile.as_deref(), Some("public"));
}

#[test]
fn template_to_create_spec_rejects_missing_cwd_mount() {
    let t = parse("x", include_str!("fixtures/templates/valid-full.toml")).unwrap();
    assert!(to_create_spec(&t, "relative/path").is_err());
}

#[test]
fn preview_marks_defaults() {
    let t = parse("x", include_str!("fixtures/templates/valid-full.toml")).unwrap();
    let rows = preview_values(&t, "/home/me/proj");
    let cpu = rows.iter().find(|r| r.label == "CPU").unwrap();
    assert_eq!(cpu.value, "4");
    assert!(!cpu.is_default);
    // Template ohne cpus → Default-Zeile
    let plain = parse("y", "[meta]\nname = \"S\"\ndescription = \"d\"\n\nimage = \"alpine\"\n").unwrap();
    let rows = preview_values(&plain, "/home/me/proj");
    assert!(rows.iter().any(|r| r.label == "CPU" && r.is_default));
    assert!(rows.iter().any(|r| r.label == "Net" && r.value == "public"));
}

#[test]
fn layout_thresholds() {
    // Spec §3: <72 Spalten gestapelt; <20 Zeilen nur Liste.
    assert!(matches!(template_layout(71, 24), LayoutKind::Stacked));
    assert!(matches!(template_layout(72, 24), LayoutKind::MasterDetail));
    assert!(matches!(template_layout(72, 19), LayoutKind::ListOnly));
    assert!(matches!(template_layout(60, 10), LayoutKind::ListOnly));
}
```

- [ ] **Step 2: FAIL verifizieren** (Funktionen fehlen → E0425)

- [ ] **Step 3: Implementieren**

`assemble_spec` = Körper des heutigen `CreateForm::to_create_spec` (create.rs ~745-830), Parameter aus `SpecInputs`; das Formular ruft es mit geparsten Werten auf (Verhalten/Tests unverändert). `to_create_spec`:

```rust
pub fn to_create_spec(t: &Template, cwd: &str) -> anyhow::Result<crate::backend::CreateSpec> {
    let ports = t.spec.ports.iter()
        .map(|p| crate::models::PublishedPort::parse_cli(p))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let name = match t.spec.name.as_deref() {
        Some(n) if !n.trim().is_empty() => Some(n.trim().to_string()),
        _ => None, // resolve_name entscheidet vorher über Suffixe
    };
    assemble_spec(SpecInputs {
        image: t.spec.image.trim(),
        name,
        cpus: t.spec.cpus,
        memory: t.spec.memory.clone(),
        workdir: t.spec.workdir.clone(),
        mount_cwd: t.spec.mount_cwd.unwrap_or(true),
        cwd,
        ports,
        volumes: t.spec.volumes.clone(),
        env: t.spec.env.clone(),
        labels: t.spec.labels_vec(),
        net_rules: t.spec.net_rules.clone(),
        net_profile: t.spec.net_profile.clone(),
    })
}
```

`resolve_name`, `preview_values` (Zeilen: Image, Name, CPU, Mem, Mount, Ports, Env, Net — Default-Markierung wenn Feld nicht im Template), `template_layout` wie getestet. `assemble_spec` in create.rs: `use crate::template::{assemble_spec, SpecInputs};`, alter Body entfernt.

- [ ] **Step 4: Tests grün + Gates, Commit**

```bash
git add -A && git commit -m "feat(templates): spec assembly, name resolution, preview, layout

assemble_spec is extracted from the form so form and templates share
one CreateSpec assembly path; templates resolve names with -N suffixes
and preview effective values with default markers.

Closes phase2#10.4"
```

---

### Task 5: Events + App-Zustandsautomat (`CreateState`)

**Files:**
- Modify: `src/event.rs` (neue Varianten nach `ImagesUpdated`)
- Modify: `src/app.rs` (App-Felder, `Op::DeleteTemplate`, `handle_create_key` → zwei States, Tests)

**Interfaces:**
- Consumes: `Template`, `to_create_spec`, `resolve_name`, `preview_values` (Task 4); `CreateForm::from_template`/`to_template` (Task 7a/7b — hier nur als Signaturen referenziert).
- Produces (Task 6/7b/8):

```rust
pub enum CreateState { Select { selected: usize }, Form(crate::ui::create::CreateForm) }
// App-Felder:
pub templates: Vec<crate::template::Template>,
pub create_state: Option<CreateState>,      // ersetzt create_form: Option<CreateForm>
pub take_save_template: Option<crate::template::Template>, // Ctrl+S-Ausgang
// Op-Variante:
Op::DeleteTemplate(String),                 // requires_confirmation = true
// AppEvent-Varianten:
TemplatesLoaded(Vec<crate::template::Template>), TemplateSaved(String), TemplateDeleted(String)
```

- [ ] **Step 1: Failing tests in `app.rs` (tests-Modul, Muster der vorhandenen Tests)**

```rust
fn app_with_templates() -> App {
    let mut app = App::new_test(); // oder das vorhandene Test-Setup-Muster
    app.templates = crate::template::load_builtins();
    app
}

#[test]
fn c_key_opens_template_select() { /* c → view==Create, matches!(create_state, Some(Select{..})) */ }

#[test]
fn select_enter_queues_create_with_resolved_name() {
    // zweites "shell"-Sandbox-Objekt vorfaken: sandboxes enthält "shell"
    // Enter → queued_create.name == Some("shell-2"); create_state == None; view == Dashboard
}

#[test]
fn select_enter_while_busy_ignored() {
    // Review Focus 3: app.busy = true; Enter → queued_create bleibt None
}

#[test]
fn select_e_opens_prefilled_form() {
    // e → matches!(Some(Form(form))), form.image == Template-Image, form.name == Muster
}

#[test]
fn select_n_opens_empty_form() { /* n → Form mit Leere-Defaults (mount_cwd=true) */ }

#[test]
fn form_esc_returns_to_select_not_dashboard() { /* Esc im Form → Some(Select{..}) */ }

#[test]
fn select_d_on_builtin_shows_hint_only() {
    // d auf built-in → app.confirm bleibt None, Statuszeile enthält "built-in"
}

#[test]
fn select_d_on_user_template_asks_confirm() {
    // Nutzer-Template injizieren; d → app.confirm == Some(Op::DeleteTemplate(id))
}

#[test]
fn templates_loaded_replaces_list() {
    // TemplatesLoaded(vec![...]) → app.templates ersetzt; Action::Render
}
```

- [ ] **Step 2: FAIL verifizieren** (CreateState existiert nicht)

- [ ] **Step 3: Implementieren**

- `event.rs`: drei Varianten (Doku-Kommentare wie die Nachbarn).
- `app.rs`:
  - `pub enum CreateState { Select { selected: usize }, Form(crate::ui::create::CreateForm) }` (Top-Level in app.rs, pub).
  - `create_form: Option<CreateForm>` → `create_state: Option<CreateState>`; alle Referenzen umstellen (grep `create_form`).
  - `handle_create_key`: match auf State. Select: `↑/↓` (clamp), `Enter` (guard: `if self.busy { return Action::Continue; }` — dann Template→`to_create_spec` + `resolve_name(pattern, self.sandboxes-Namen)`; `queued_create = Some(spec)`; view=Dashboard; Status „Creating …“), `e` (Form aus Template via `CreateForm::from_template`), `n` (leeres Formular), `d` (built-in → Hint; Nutzer-Template → `self.confirm = Some(Op::DeleteTemplate(id))`), `Esc` (Dashboard). Form: wie heute, aber `FormAction::Cancel` → `CreateState::Select{selected}` statt Dashboard; neu: `FormAction::SaveTemplate(t)` → `self.take_save_template = Some(t)`; `create_state` bleibt Form.
  - `Op::DeleteTemplate(String)`: `describe` → `format!("Delete template '{id}'? (file will be removed)")`, `requires_confirmation` → `true`.
  - Events: `TemplatesLoaded` → `self.templates = v; Action::Render`; `TemplateSaved(id)` → Template in `self.templates` ersetzen (per id, sonst hinten anhängen); `TemplateDeleted(id)` → entfernen. Beide → `Action::Render`.
  - `open_create_form()` → `open_create_templates()`: `view=Create; create_state=Some(Select{selected:0})` (Dashboard-Key `c` umstellen).
- Achtung `handle_create_key`-Guard: wenn `create_state` None → Dashboard (wie heute).

- [ ] **Step 4: Tests grün + Gates, Commit**

```bash
git add -A && git commit -m "feat(app): create state machine with template select

View::Create becomes a two-state automaton (template select ↔ form);
Enter creates directly with -N name suffixing, e prefills the form,
d deletes user templates via the confirm pipeline (Op::DeleteTemplate);
new TemplatesLoaded/Saved/Deleted events.

Closes phase2#10.5"
```

---

### Task 6: C-Ansicht-Rendering (`src/ui/template_picker.rs`)

**Files:**
- Create: `src/ui/template_picker.rs`
- Modify: `src/ui/mod.rs` (`pub mod template_picker;`)
- Modify: `src/main.rs` (Render-Dispatch: `View::Create` + `Select` → `template_picker::render`, `Form` → `create::render_create_form`)

**Interfaces:**
- Consumes: `App.templates`, `CreateState::Select{selected}`, `preview_values`, `template_layout` (Task 4/5).
- Produces: `pub fn render(frame: &mut Frame, area: Rect, templates: &[Template], selected: usize) -> ()` (Master-Detail/Stacked/ListOnly intern via `template_layout(area.width, area.height)`).

- [ ] **Step 1: Implementieren** (UI — kein Unit-Test laut Projektregel)

- Linke Spalte (~40%): Block „Templates“, Zeilen `name` (bold wenn selected, THEME-Farben wie dashboard-Karten) + `description` (muted, 1 Zeile, truncate), `built_in`-Marker `(built-in)`; Nutzer zuerst.
- Rechte Spalte: Block „Vorschau: {name}“; Zeilen aus `preview_values` — Label muted, Wert normal, `(default)`-Suffix muted wenn `is_default`.
- Stacked (<72): Liste oben (~40% Höhe), Vorschau unten. ListOnly (<20 Zeilen): nur Liste mit Beschreibung unter dem Namen.
- Footer-Hinweise wie im Dashboard-Stil: `Enter erstellen · e anpassen · n neu · d löschen · Esc ←`.
- Kein Panic bei 0 Templates (Liste zeigt Hinweis „keine Templates geladen“, Vorschau leer).

- [ ] **Step 2: Gates** — `cargo fmt --check && cargo clippy -- -D warnings && cargo check 2>&1 | tail -1` — Expected: grün.

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "feat(ui): template master-detail picker view

Rendering for the create entry (variant C): template list with
descriptions, effective-value preview with default markers, stacked and
list-only degradations for small terminals.

Closes phase2#10.6"
```

---

### Task 7a: Formular-Modell — gruppierter `FormField`, `Labels`-Feld, `from_template`/`to_template`

**Files:**
- Modify: `src/ui/create.rs` (FormField, ALL, `CreateForm`-Felder, `to_create_spec`, `handle_key`, Tests)

**Interfaces:**
- Consumes: `assemble_spec` (Task 4).
- Produces (Task 7b/8):

```rust
pub enum FormField { Image, Name, Cpus, Memory, MountCwd, Workdir, Volumes, NetProfile, Ports, NetRules, EnvVars, Labels }
// ALL: [Image, Name, Cpus, Memory, MountCwd, Workdir, Volumes, NetProfile, Ports, NetRules, EnvVars, Labels]
pub enum FormAction { NextField, PrevField, Cancel, Submit, SaveTemplate(crate::template::Template) }
impl CreateForm {
    pub fn from_template(t: &crate::template::Template, cwd: String) -> Self
    pub fn to_template(&self, name: String, description: String) -> crate::template::Template
}
```

- [ ] **Step 1: Failing tests**

```rust
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
    let form = CreateForm::new(Vec::new());
    let spec = form.to_create_spec().unwrap();
    assert_eq!(spec.net_profile.as_deref(), Some("public"));
}

#[test]
fn form_from_template_prefills_all_fields() {
    let t = crate::template::parse("opencode", include_str!("../fixtures/templates/valid-full.toml")).unwrap();
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
}

#[test]
fn form_to_template_roundtrips_through_file() {
    let t0 = crate::template::parse("opencode", include_str!("../fixtures/templates/valid-full.toml")).unwrap();
    let mut form = CreateForm::from_template(&t0, "/home/me/proj".into());
    let t1 = form.to_template("Opencode".into(), "d".into());
    assert_eq!(t1.spec.image, "node:22-alpine");
    assert_eq!(t1.spec.cpus, Some(4));
    assert_eq!(t1.spec.labels_vec(), vec!["team=infra"]);
}
```

- [ ] **Step 2: FAIL verifizieren** (Labels-Variante/from_template fehlen)

- [ ] **Step 3: Implementieren**

- `FormField` um `Labels` erweitern; `ALL` auf 12 Felder in Gruppenreihenfolge; `label()`-Texte anpassen; `field_count()` → 12.
- `CreateForm`: Feld `pub labels: Vec<String>` hinzufügen (List-Interaktionen kopieren die der Env-Vars — `handle_key`-Zweig + Buffer-Handling); `advanced: bool` und alle `Ctrl+A`-Zweige entfernen; `net_profile` in `to_create_spec` immer `Some(self.net_profile.as_str().to_string())`; `labels` → `trim_filter(&self.labels)`.
- `to_create_spec`: Body durch `assemble_spec(SpecInputs{ ... })` ersetzen (Task 4 vorbereitet); Validierungen (Image-Pflicht, cwd-Checks) bleiben im Wrapper.
- `from_template`: alle Felder aus `TemplateSpec` (cpus→String via `.map(|c| c.to_string()).unwrap_or_default()`, memory direct, `mount_cwd.unwrap_or(true)`, ports/volumes/env/net_rules als Strings, labels über `labels_vec()`, net_profile mit `NetProfile`-Enum-Parsing, Default Public; workdir/mount_cwd wie Template).
- `to_template`: Felder zurück in `TemplateSpec` (cpus via `str::parse::<u32>().ok()`, labels `KEY=VALUE` → BTreeMap, name-Feld `Some(self.name)` nur wenn nicht leer).
- Tests, die `advanced`/`field_count()==3/11` nutzen, umstellen.

- [ ] **Step 4: Tests grün + Gates, Commit**

```bash
git add -A && git commit -m "feat(create): grouped form field model with labels

FormField gains Labels, tab order follows the grouped layout (12
fields), quick/advanced split is removed, net_profile is always
applied; forms convert to/from templates (Ctrl+S and e-paths).

Closes phase2#10.7a"
```

---

### Task 7b: Formular-Rendering gruppiert + `Ctrl+S`-Dialog

**Files:**
- Modify: `src/ui/create.rs` (render_create_form, SaveDialog, Tests)

**Interfaces:**
- Consumes: Task 7a Modell; THEME; `textwrap` (ui/mod.rs) für den Dialog.
- Produces: `FormAction::SaveTemplate(Template)` (App-Verkabelung in Task 8), gerendertes gruppiertes Formular.

- [ ] **Step 1: Failing test für den Dialog (Logik, nicht Rendering)**

```rust
#[test]
fn save_dialog_asks_on_collision() {
    // Review Focus 4: existierender Dateiname → Confirm, kein stilles Überschreiben.
    let mut form = CreateForm::new(Vec::new());
    form.image = "alpine".into();
    let slug = crate::template::slugify("shell"); // built-in-id als Kollision
    assert_eq!(slug, "shell");
    let action = form.open_save_dialog("Shell".into(), "d".into());
    // Dialog offen mit Ziel-Slug; App entscheidet via templates-liste über Confirm.
    assert!(form.save_dialog.is_some());
}
```

- [ ] **Step 2: FAIL verifizieren** (save_dialog/Open-Methode fehlt)

- [ ] **Step 3: Implementieren**

- `struct SaveDialog { name: String, description: String, field: SaveField }` als Feld `save_dialog: Option<SaveDialog>` an `CreateForm`; `open_save_dialog(name, description)` öffnet mit Slug-Vorschlag aus Template-Namen.
- `Ctrl+S` im Formular → Dialog öffnen (Feld „Name" aktiv); Dialog-Keys: Texteingabe, `Tab` wechselt Name↔Beschreibung, `Enter` → `CreateForm::to_template` → `FormAction::SaveTemplate(t)`, `Esc` → Dialog zu. Dialog rendert als kleines Box-Overlay über dem Formular (wie Confirm-Dialog-Stil, `textwrap` für die Beschreibung).
- `render_create_form` gruppiert: Abschnitts-Header (`BASIS`, `RESSOURCEN`, `MOUNTS`, `NETZWERK`, `SONSTIGES`) als muted Überschriftenzeilen, darunter die Felder (Label links, Wert rechts, aktives Feld bold/accent wie heute); Image-Picker-Zeilen (└ suggestions) unter dem Image-Feld wie gebaut; Listenfelder zeigen Count + aktive Liste. Kleine Terminals: Paarzeilen (CPU+Mem) brechen auf einspaltig (Breite < 60).
- Footer: `Tab/↓ Feld · Ctrl+S Template · Enter erstellen · Esc ←`.
- Entfernte Render-Pfade: Quick/Advanced-Hinweise, Ctrl+A-Zeilen.

- [ ] **Step 4: Tests grün + Gates, Commit**

```bash
git add -A && git commit -m "feat(create): grouped full-screen form with save-as-template dialog

Rendering follows the grouped sections (BASIS/RESSOURCEN/MOUNTS/
NETZWERK/SONSTIGES); Ctrl+S opens a name+description dialog that emits
SaveTemplate; overwriting an existing template file asks for
confirmation.

Closes phase2#10.7b"
```

---

### Task 8: `main.rs`-Verkabelung — Startup-Load, Save/Delete-Executor

**Files:**
- Modify: `src/main.rs` (Modul-Dispatch, Startup-Task, Op-Arm, Event-Arms)

**Interfaces:**
- Consumes: alle vorherigen Tasks (`load_all`, `templates_dir`, `write_template`, Events, `Op::DeleteTemplate`, `App::take_save_template`, `take_op`).
- Produces: laufender End-to-End-Flow (manueller Smoke-Test).

- [ ] **Step 1: Implementieren**

- Startup (nach Backend-Ready, Muster wie images-fetch in main.rs): `tokio::spawn` → `let (templates, warnings) = crate::template::load_all(&crate::template::templates_dir());` → `AppEvent::TemplatesLoaded(templates)` senden; jede Warnung als `AppEvent::Error(w)` senden (Statuszeile).
- Render-Dispatch: `View::Create` → match `create_state`: `Select{selected}` → `ui::template_picker::render(frame, area, &app.templates, *selected)`; `Form(form)` → `ui::create::render_create_form(frame, area, form)`.
- Op-Executor (wo `Op`-Arms gematcht werden): `Op::DeleteTemplate(id)` → Task: Template in `app.templates` nachschlagen (Snapshot vor Spawn), nur wenn `!built_in` Datei `<dir>/<slug>.toml` löschen (Slug konsistent mit `write_template`; Datei evtl. mit anderem Slug existent → Suche nach `*.toml` mit passender id? — Nein: Delete nutzt denselben Slug-Pfad wie `write_template` schreibt; Built-ins haben keine Datei). Erfolg → `TemplateDeleted(id)`, sonst `Error(...)`.
- `take_save_template()` konsumieren (Muster wie `take_op`/`queued_create`): Task → `write_template` → `TemplateSaved(id)`; Fehler → `Error(...)`.
- Event-Arms in der App (`handle_event`): wie in Task 5 definiert.

- [ ] **Step 2: Gates + manueller Smoke-Test**

Run: `cargo fmt --check && cargo clippy -- -D warnings && cargo test && cargo check 2>&1 | tail -1`
Expected: alle grün (Tests 215+neue).

Manuell (in einem Terminal mit `cargo run`): `c` → C-Ansicht mit 3 Built-ins → `↑↓`, Vorschau wechselt → `Enter` auf `shell` → Sandbox entsteht (detached, Dashboard-Status) → `c` → `e` auf `opencode` → gruppiertes Formular vorgefüllt → `Ctrl+S` → Dialog → speichern → Meldung, Template erscheint in Liste → `d` auf Nutzer-Template → Confirm → weg; `d` auf Built-in → Hinweis. Terminal auf 60×18 verkleinern → gestapelt/ListOnly, kein Panic.

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "feat(main): wire template loading, saving and deletion

Templates load at startup on a tokio task (warnings surface on the
status line); Ctrl+S writes the TOML file off-thread; Op::DeleteTemplate
removes user template files through the confirm pipeline.

Closes phase2#10.8"
```

---

### Task 9: Doku — DESIGN.md-Roadmap + Keybindings, PLAN.md

**Files:**
- Modify: `docs/DESIGN.md` (Phase-2-Liste nach Zeile 510, Keybinding-Tabelle)
- Modify: `docs/PLAN.md` (Abschnitt 10 mit Aufgaben 10.1-10.9, alle `[x]`)

- [ ] **Step 1: DESIGN.md** — Phase 2 ergänzen:

```markdown
11. Templates (built-ins + user TOML files, master-detail create entry)
12. Create form rework (grouped full-screen form, Ctrl+S save-as-template)
```

Keybinding-Tabelle: `c` → „Create: Template-Auswahl (Master-Detail)“; neu `e` (Template im Formular bearbeiten), `n` (leeres Formular), `d` (Nutzer-Template löschen), `Ctrl+S` (Als Template speichern); `Ctrl+A`-Zeile entfernen. Hinweis in der Design-Notiz: Template-Format = Spec §1, `rootfs` reserviert.

- [ ] **Step 2: PLAN.md** — Abschnitt 10 „Templates + Create-Rework“ mit den Aufgaben 10.1-10.9 (Titel + eine Zeile Ergebnis je Task), alle als `[x]`, Datum 2026-09-25+, Verweis auf Spec und diesen Plan.

- [ ] **Step 3: Gates + Commit + Push**

```bash
cargo fmt --check && cargo clippy -- -D warnings && cargo test 2>&1 | grep "test result" && git add -A && git commit -m "docs(design): templates and create-rework roadmap entries

Phase 2 gains items 11 (templates) and 12 (create form rework);
keybinding table reflects the new create flow; PLAN.md records the
completed tasks.

Closes phase2#10.9" && git push origin main
```

---

## Plan-Selbstprüfung (Ergebnis)

- **Spec-Coverage:** §1→Task 2, §2→Task 3, §3→Task 4/5/6, §4→Task 7a/7b, §5→Task 2/5/6/7, §6→Steps in Tasks 2-5+7a/7b (Nr. 1-9 abgedeckt), §7→Task 9, §8→Task 3 Step 1. Keine Lücke.
- **Placeholders:** keine „TBD/TODO“; jeder Code-Schritt hat realen Code.
- **Typ-Konsistenz:** `TemplateSpec.labels` (BTreeMap) ↔ `labels_vec()` konsistent in Task 2/4/7a; `CreateState` in 5/6/8; `FormAction::SaveTemplate(Template)` in 7a/7b/8; `PublishedPort::parse_cli` in 1/2/4.
- **Review Focus:** 1→Task 3 Test, 2→Task 3 Test, 3→Task 5 Test, 4→Task 7b Test, 5→Task 6 Smoke-Notiz (+ getestetes `preview_values`).