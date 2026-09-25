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
#[derive(Debug, Clone, serde::Deserialize)]
pub struct TemplateMeta {
    /// Human-readable display name (e.g. `Opencode`).
    pub name: String,
    /// One-line description shown in the picker.
    pub description: String,
}

/// The spec fields of a template document: the create form's fields, all
/// optional except `image`.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
pub struct TemplateSpec {
    /// OCI image reference (required).
    pub image: String,
    /// Sandbox name pattern; collisions resolve to `-2`, `-3`, …
    pub name: Option<String>,
    /// vCPU count.
    pub cpus: Option<u32>,
    /// Memory limit (`512M`, `2G`, …).
    pub memory: Option<String>,
    /// Working directory inside the sandbox.
    pub workdir: Option<String>,
    /// Mount the TUI's CWD into the sandbox (default: true).
    pub mount_cwd: Option<bool>,
    /// Port specs in the CLI form (`127.0.0.1:3000:3000`).
    pub ports: Vec<String>,
    /// Volume mounts (`SOURCE:DEST[:OPTIONS]`).
    pub volumes: Vec<String>,
    /// Environment variables (`KEY=VALUE`).
    pub env: Vec<String>,
    /// Labels (`KEY=VALUE`).
    pub labels: BTreeMap<String, String>,
    /// Network profile: `public`, `private`, or `host` (default `public`).
    pub net_profile: Option<String>,
    /// Raw net rules (`allow@host`).
    pub net_rules: Vec<String>,
    /// Reserved for golden-image sources; no effect in v1.
    pub rootfs: Option<String>,
}

/// A template document as loaded from disk or from the built-in presets.
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone, serde::Deserialize)]
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
}
