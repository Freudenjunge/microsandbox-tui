//! Sandbox templates: saved create configurations as TOML documents.
//!
//! One file per template in `~/.config/microsandbox-tui/templates/`
//! (plus built-ins embedded into the binary). A template maps 1:1 onto the
//! create form's fields and becomes a `CreateSpec` (see
//! [`crate::template::to_create_spec`]). The `rootfs` key is reserved for
//! future golden-image sources and has no effect yet.
//
// TODO(phase2#10.8): remove this allow once main.rs wires up every item —
// until then consumers land task by task and clippy -D warnings would
// reject the not-yet-reached ones.
#![allow(dead_code)]

use std::collections::BTreeMap;

/// Display metadata for a template (`[meta]` section).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TemplateMeta {
    /// Human-readable display name (e.g. `Opencode`).
    pub name: String,
    /// One-line description shown in the picker.
    pub description: String,
}

/// The spec fields of a template document: the create form's fields, all
/// optional except `image`.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct TemplateSpec {
    /// OCI image reference (required).
    pub image: String,
    /// Sandbox name pattern; collisions resolve to `-2`, `-3`, …
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// vCPU count.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpus: Option<u32>,
    /// Memory limit (`512M`, `2G`, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,
    /// Working directory inside the sandbox.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    /// Mount the TUI's CWD into the sandbox (default: true).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mount_cwd: Option<bool>,
    /// Port specs in the CLI form (`127.0.0.1:3000:3000`).
    pub ports: Vec<String>,
    /// Volume mounts (`SOURCE:DEST[:OPTIONS]`).
    pub volumes: Vec<String>,
    /// Environment variables (`KEY=VALUE`).
    pub env: Vec<String>,
    /// Labels (`KEY=VALUE`).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    /// Network profile: `public`, `private`, or `host` (default `public`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub net_profile: Option<String>,
    /// Raw net rules (`allow@host`).
    pub net_rules: Vec<String>,
    /// Reserved for golden-image sources; no effect in v1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rootfs: Option<String>,
}

/// A template document as loaded from disk or from the built-in presets.
#[derive(Debug, Clone, PartialEq)]
pub struct Template {
    /// Identity: the file name stem (also the shadowing key).
    pub id: String,
    /// Display metadata.
    pub meta: TemplateMeta,
    /// Spec fields.
    pub spec: TemplateSpec,
    /// True when embedded in the binary (not deletable, shadowable).
    pub built_in: bool,
}

/// Serde view of a template document on disk (`[meta]` + spec fields).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct TemplateFile {
    meta: TemplateMeta,
    spec: TemplateSpec,
}

impl TemplateSpec {
    /// Labels as `KEY=VALUE` strings (sorted by key), the form's format.
    pub fn labels_vec(&self) -> Vec<String> {
        self.labels
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect()
    }
}

/// Parse a template document. `id` comes from the filename stem. Unknown
/// fields are ignored (serde default); `rootfs` is parsed and stored but
/// has no effect in v1 (reserved for golden-image sources).
pub fn parse(id: &str, s: &str) -> Result<Template, String> {
    let f: TemplateFile = toml::from_str(s).map_err(|e| format!("template '{id}': {e}"))?;
    if f.spec.image.trim().is_empty() {
        return Err(format!("template '{id}': 'image' is required"));
    }
    if let Some(m) = f.spec.memory.as_deref()
        && crate::backend::sdk::parse_memory_mib(m).is_none()
    {
        return Err(format!(
            "template '{id}': invalid memory '{m}' (use e.g. 512M, 2G)"
        ));
    }
    for p in &f.spec.ports {
        if crate::models::PublishedPort::parse_cli(p).is_err() {
            return Err(format!("template '{id}': invalid port '{p}'"));
        }
    }
    for r in &f.spec.net_rules {
        // Format wie die Formular-Eingabe: `allow@host`.
        if !r.contains('@') {
            return Err(format!(
                "template '{id}': invalid net rule '{r}' (expected allow@host)"
            ));
        }
    }
    Ok(Template {
        id: id.to_string(),
        meta: f.meta,
        spec: f.spec,
        built_in: false,
    })
}

/// Built-in templates, embedded at compile time — they double as living
/// examples of the file format (copy one to start your own).
const BUILTIN_SOURCES: &[(&str, &str)] = &[
    ("opencode", include_str!("templates/opencode.toml")),
    ("opencode2", include_str!("templates/opencode2.toml")),
    ("shell", include_str!("templates/shell.toml")),
];

/// The built-in templates, in stable display order.
pub fn load_builtins() -> Vec<Template> {
    BUILTIN_SOURCES
        .iter()
        .filter_map(|(id, src)| {
            parse(id, src).ok().map(|mut t| {
                t.built_in = true;
                t
            })
        })
        .collect()
}

/// Template directory: `XDG_CONFIG_HOME`, else `$HOME/.config` (Spec §2).
pub fn templates_dir() -> std::path::PathBuf {
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME")
        && !x.is_empty()
    {
        return std::path::Path::new(&x).join("microsandbox-tui/templates");
    }
    let home = std::env::var("HOME").unwrap_or_default();
    std::path::Path::new(&home).join(".config/microsandbox-tui/templates")
}

/// Load every template: user files from `dir` (sorted, broken files are
/// skipped with a warning) merged over the built-ins — a user file with
/// the same id shadows the built-in. A missing/unreadable directory is
/// not an error; the built-ins still load.
pub fn load_all(dir: &std::path::Path) -> (Vec<Template>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut user: Vec<Template> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut paths: Vec<_> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        paths.sort();
        for p in paths {
            let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if p.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            match std::fs::read_to_string(&p)
                .map_err(|e| e.to_string())
                .and_then(|s| parse(stem, &s))
            {
                Ok(t) => user.push(t),
                Err(e) => warnings.push(e),
            }
        }
    }
    // Merge: the user file wins on an id collision; display order is
    // user files first (sorted), then unshadowed built-ins.
    let mut out = user;
    let ids: std::collections::HashSet<String> = out.iter().map(|t| t.id.clone()).collect();
    for b in load_builtins() {
        if !ids.contains(&b.id) {
            out.push(b);
        }
    }
    (out, warnings)
}

/// Kebab-case slug for a template's display name (the save file name).
pub fn slugify(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let mut s = s.as_str();
    while s.starts_with('-') {
        s = &s[1..];
    }
    while s.ends_with('-') {
        s = &s[..s.len() - 1];
    }
    if s.is_empty() {
        "template".into()
    } else {
        s.to_string()
    }
}

/// Write `meta`+`spec` as `<slug>.toml` into `dir`. Overwriting an
/// existing file is confirmed by the caller first.
pub fn write_template(dir: &std::path::Path, t: &Template) -> anyhow::Result<std::path::PathBuf> {
    std::fs::create_dir_all(dir)?;
    let file = TemplateFile {
        meta: t.meta.clone(),
        spec: t.spec.clone(),
    };
    let body = toml::to_string_pretty(&file)
        .map_err(|e| anyhow::anyhow!("serialize template '{}': {e}", t.id))?;
    let path = dir.join(format!("{}.toml", slugify(&t.meta.name)));
    std::fs::write(&path, body)?;
    Ok(path)
}

/// Trim and filter empty strings from a list (shared by form and template
/// spec assembly).
pub(crate) fn trim_filter(list: &[String]) -> Vec<String> {
    list.iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Everything needed to assemble a [`CreateSpec`], already parsed: the
/// create form and templates both converge here so there is exactly one
/// assembly path.
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

/// Assemble a `CreateSpec`: workspace mount at the same absolute path
/// (sbx-style, paths match host/guest so stack traces line up), workdir
/// defaulting to the mount, empty list entries filtered out. The exists
/// check for the CWD lives with the form (it validates the live
/// environment at submit time); this stays pure.
pub(crate) fn assemble_spec(i: SpecInputs<'_>) -> anyhow::Result<crate::backend::CreateSpec> {
    let mut workspace_mount: Option<String> = None;
    if i.mount_cwd {
        let cwd = i.cwd.trim();
        if !cwd.starts_with('/') {
            anyhow::bail!("Workspace mount: {cwd} is not an absolute path");
        }
        workspace_mount = Some(cwd.to_string());
    }
    // Explicit workdir wins; with the workspace mount on (and no explicit
    // entry) the mounted path is the default — sbx starts the sandbox in
    // its primary workspace. No mount means no explicit workdir: the SDK
    // stats the workdir inside the guest rootfs at create time, and plain
    // OCI images don't contain `/home/agent/workspace` (it's baked into
    // sbx template images only) — mountless sandboxes start in the image's
    // own working directory, exactly like `sbx run` without a workspace.
    let workdir = {
        let w = i.workdir.as_deref().unwrap_or("").trim();
        if !w.is_empty() {
            Some(w.to_string())
        } else {
            workspace_mount.clone()
        }
    };
    let mut volumes = trim_filter(&i.volumes);
    if let Some(cwd) = workspace_mount {
        let spec = format!("{cwd}:{cwd}");
        if !volumes.contains(&spec) {
            volumes.insert(0, spec);
        }
    }
    Ok(crate::backend::CreateSpec {
        image: i.image.to_string(),
        name: i.name,
        cpus: i.cpus,
        memory: i.memory,
        workdir,
        ports: i.ports,
        volumes,
        env: trim_filter(&i.env),
        labels: trim_filter(&i.labels),
        net_profile: i.net_profile,
        net_rules: trim_filter(&i.net_rules),
    })
}

/// Build the `CreateSpec` a template stands for (rootfs is reserved and
/// has no effect in v1). `cwd` is the workspace mount source.
pub fn to_create_spec(t: &Template, cwd: &str) -> anyhow::Result<crate::backend::CreateSpec> {
    let ports = t
        .spec
        .ports
        .iter()
        .map(|p| crate::models::PublishedPort::parse_cli(p))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let name = match t.spec.name.as_deref() {
        Some(n) if !n.trim().is_empty() => Some(n.trim().to_string()),
        _ => None, // resolve_name decides on suffixes before this
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

/// Resolve a template's name pattern against existing sandbox names:
/// free → as-is, taken → first free `pattern-2`, `pattern-3`, …
pub fn resolve_name(pattern: &str, existing: &[String]) -> Option<String> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return None;
    }
    if !existing.iter().any(|n| n == pattern) {
        return Some(pattern.to_string());
    }
    for n in 2.. {
        let candidate = format!("{pattern}-{n}");
        if !existing.iter().any(|n2| n2 == &candidate) {
            return Some(candidate);
        }
    }
    None
}

/// One row of the template preview: the effective value that would be
/// created, flagged when it is a default (not set by the template).
pub struct PreviewRow {
    pub label: &'static str,
    pub value: String,
    pub is_default: bool,
}

/// Effective values of what a template would create (Spec §3): template
/// values win, unset fields fall back to the documented defaults and are
/// flagged.
pub fn preview_values(t: &Template, cwd: &str) -> Vec<PreviewRow> {
    let s = &t.spec;
    let mut rows = Vec::with_capacity(8);
    rows.push(PreviewRow {
        label: "Image",
        value: s.image.clone(),
        is_default: false,
    });
    rows.push(PreviewRow {
        label: "Name",
        value: s.name.clone().unwrap_or_else(|| "(auto)".into()),
        is_default: s.name.is_none(),
    });
    rows.push(PreviewRow {
        label: "CPU",
        value: s
            .cpus
            .map(|c| c.to_string())
            .unwrap_or_else(|| "(default)".into()),
        is_default: s.cpus.is_none(),
    });
    rows.push(PreviewRow {
        label: "Mem",
        value: s.memory.clone().unwrap_or_else(|| "(default)".into()),
        is_default: s.memory.is_none(),
    });
    rows.push(PreviewRow {
        label: "Mount",
        value: if s.mount_cwd.unwrap_or(true) {
            format!("CWD → {cwd}")
        } else {
            "(none)".into()
        },
        is_default: s.mount_cwd.is_none(),
    });
    rows.push(PreviewRow {
        label: "Ports",
        value: if s.ports.is_empty() {
            "(none)".into()
        } else {
            s.ports.join(", ")
        },
        is_default: s.ports.is_empty(),
    });
    rows.push(PreviewRow {
        label: "Env",
        value: if s.env.is_empty() {
            "(none)".into()
        } else {
            s.env.join(", ")
        },
        is_default: s.env.is_empty(),
    });
    rows.push(PreviewRow {
        label: "Net",
        value: s.net_profile.clone().unwrap_or_else(|| "public".into()),
        is_default: s.net_profile.is_none(),
    });
    rows
}

/// How the template picker lays out at the given terminal size (Spec §3):
/// master-detail at ≥72 columns and ≥20 rows, stacked list-over-preview
/// when narrower, list-only when shorter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutKind {
    MasterDetail,
    Stacked,
    ListOnly,
}

pub fn template_layout(width: u16, height: u16) -> LayoutKind {
    if height < 20 {
        LayoutKind::ListOnly
    } else if width < 72 {
        LayoutKind::Stacked
    } else {
        LayoutKind::MasterDetail
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_all_fields() {
        let t = parse(
            "opencode",
            include_str!("fixtures/templates/valid-full.toml"),
        )
        .unwrap();
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
        std::fs::write(dir.join("shell.toml"), include_str!("templates/shell.toml")).unwrap();
        std::fs::write(dir.join("broken.toml"), "kein = [toml").unwrap();
        let (templates, warnings) = load_all(&dir);
        assert!(templates.iter().all(|t| t.id != "broken"));
        assert!(
            warnings.iter().any(|w| w.contains("broken")),
            "{warnings:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn user_file_shadows_builtin_with_same_id() {
        let dir = std::env::temp_dir().join(format!("msb-tui-shadow-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("shell.toml"),
            "[meta]\nname = \"Meine Shell\"\ndescription = \"d\"\n\n[spec]\nimage = \"debian\"\n",
        )
        .unwrap();
        let (templates, _) = load_all(&dir);
        let shell = templates.iter().find(|t| t.id == "shell").unwrap();
        assert_eq!(shell.meta.name, "Meine Shell");
        assert!(
            !shell.built_in,
            "shadowing file wins and is a user template"
        );
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

    #[test]
    fn trim_filter_removes_blanks() {
        let list = vec![" a ".into(), String::new(), "b".into()];
        assert_eq!(trim_filter(&list), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn resolve_name_appends_first_free_suffix() {
        assert_eq!(resolve_name("opencode", &[]), Some("opencode".into()));
        assert_eq!(
            resolve_name("opencode", &["opencode".into()]),
            Some("opencode-2".into())
        );
        assert_eq!(
            resolve_name(
                "opencode",
                &["opencode".into(), "opencode-2".into(), "opencode-3".into()]
            ),
            Some("opencode-4".into())
        );
    }

    #[test]
    fn template_to_create_spec_maps_every_field() {
        let t = parse(
            "opencode",
            include_str!("fixtures/templates/valid-full.toml"),
        )
        .unwrap();
        let spec = to_create_spec(&t, "/home/me/proj").unwrap();
        assert_eq!(spec.image, "node:22-alpine");
        assert_eq!(spec.name.as_deref(), Some("opencode"));
        assert_eq!(spec.cpus, Some(4));
        assert_eq!(spec.memory.as_deref(), Some("2G"));
        assert_eq!(spec.ports.len(), 1);
        assert_eq!(spec.ports[0].guest_port, 3000);
        // mount_cwd=true → CWD-Bind an gleicher Stelle, workdir-Default = Mount
        assert!(
            spec.volumes
                .contains(&"/home/me/proj:/home/me/proj".to_string())
        );
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
        let plain = parse(
            "y",
            "[meta]\nname = \"S\"\ndescription = \"d\"\n\n[spec]\nimage = \"alpine\"\n",
        )
        .unwrap();
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
}
