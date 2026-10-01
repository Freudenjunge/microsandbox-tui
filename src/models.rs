//! Data models mirroring `msb` CLI JSON output.
//!
//! Every struct here is paired with a fixture test in this module that parses a
//! real response captured from `msb 0.7.2`. Do not guess field names; parse the
//! fixtures.
//!
//! NOTE(dead_code): types are parsed by tests today and gain runtime callers in
//! Task 2 (backend) and Task 4 (app). Remove this allow once the backend lands.

#![allow(dead_code)]

use std::collections::HashMap;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};

// ---------- shared types ----------

/// Normalized sandbox state across `ls`/`inspect` (PascalCase) and `metrics`
/// (lowercase) endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum SandboxState {
    Running,
    Stopped,
    Paused,
    Exited,
    Created,
    Crashed,
    Stalled,
    Unknown(String),
}

impl<'de> Deserialize<'de> for SandboxState {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(Self::from_str(&raw))
    }
}

impl SandboxState {
    /// Map a raw state string (any case) to a normalized state.
    pub fn from_str(raw: &str) -> Self {
        match raw.to_lowercase().as_str() {
            "running" => Self::Running,
            "stopped" => Self::Stopped,
            "paused" => Self::Paused,
            "exited" => Self::Exited,
            "created" => Self::Created,
            "crashed" => Self::Crashed,
            "stalled" => Self::Stalled,
            other => Self::Unknown(other.to_owned()),
        }
    }

    /// Whether the sandbox is currently executing.
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running)
    }

    /// Whether the sandbox can be started.
    pub fn is_startable(&self) -> bool {
        matches!(
            self,
            Self::Stopped | Self::Created | Self::Crashed | Self::Exited
        )
    }
}

impl std::fmt::Display for SandboxState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Paused => "paused",
            Self::Exited => "exited",
            Self::Created => "created",
            Self::Crashed => "crashed",
            Self::Stalled => "stalled",
            Self::Unknown(v) => v.as_str(),
        };
        f.write_str(s)
    }
}

/// One published port mapping (as reported by `msb inspect`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedPort {
    /// Address the host listener is bound to ("127.0.0.1", "0.0.0.0", ...).
    pub host_bind: String,
    /// Port opened on the host.
    pub host_port: u16,
    /// Port inside the sandbox.
    pub guest_port: u16,
    /// "tcp" or "udp".
    pub protocol: String,
}

impl PublishedPort {
    /// Parse a CLI/port-spec string in `HOST:GUEST` or `BIND:HOST:GUEST`
    /// form with an optional `/udp` suffix. Unit-tested.
    pub fn parse_cli(s: &str) -> Result<PublishedPort> {
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

    /// Render this mapping in the CLI form `-p` expects:
    /// `HOST:GUEST`, or `BIND:HOST:GUEST` when the bind is not the loopback
    /// default, with an optional `/udp` suffix.
    pub fn to_cli_flag(&self) -> String {
        let mut s = match self.host_bind.as_str() {
            "" | "127.0.0.1" => format!("{}:{}", self.host_port, self.guest_port),
            bind => format!("{}:{}:{}", bind, self.host_port, self.guest_port),
        };
        if self.protocol != "tcp" {
            s.push('/');
            s.push_str(&self.protocol);
        }
        s
    }

    /// Parse a port string in `msb ps` form, e.g. `127.0.0.1:8080->80/tcp` or
    /// `9090->90/udp`. Returns `Err` on malformed input.
    pub fn parse_ps(raw: &str) -> Result<Self> {
        let (mapping, protocol) = match raw.split_once('/') {
            Some((m, p)) => (m, p.to_lowercase()),
            None => (raw, "tcp".to_string()),
        };
        let (left, guest) = mapping
            .split_once("->")
            .with_context(|| format!("port mapping missing '->': {raw}"))?;
        let guest_port: u16 = guest
            .trim()
            .parse()
            .with_context(|| format!("invalid guest port in: {raw}"))?;
        let (host_bind, host_port) = if let Some((bind, port)) = left.rsplit_once(':') {
            if bind.contains('.') {
                (
                    bind.to_string(),
                    port.trim()
                        .parse::<u16>()
                        .with_context(|| format!("invalid host port in: {raw}"))?,
                )
            } else {
                (
                    "127.0.0.1".to_string(),
                    left.trim()
                        .parse::<u16>()
                        .with_context(|| format!("invalid host port in: {raw}"))?,
                )
            }
        } else {
            (
                "127.0.0.1".to_string(),
                left.trim()
                    .parse::<u16>()
                    .with_context(|| format!("invalid host port in: {raw}"))?,
            )
        };
        Ok(Self {
            host_bind,
            host_port,
            guest_port,
            protocol,
        })
    }

    /// Short display for dashboard cards, e.g. `127.0.0.1:8080→80`.
    pub fn display(&self) -> String {
        let proto = if self.protocol == "tcp" {
            String::new()
        } else {
            format!("/{}", self.protocol)
        };
        format!(
            "{}:{}→{}{}",
            self.host_bind, self.host_port, self.guest_port, proto
        )
    }
}

/// Fixture JSON for cross-module tests. Lives in `models::tests` because
/// fixtures are `include_str!`-loaded there.
#[cfg(test)]
pub(crate) fn test_fixture(name: &str) -> &'static str {
    crate::models::tests::fixture(name)
}

// ---------- msb ls ----------

/// One row of `msb ls --format json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SandboxSummary {
    pub created_at: DateTime<Utc>,
    pub image: String,
    pub name: String,
    pub status: SandboxState,
    /// Working directory inside the sandbox (runtime default when `None`).
    #[serde(default)]
    pub workdir: Option<String>,
    /// Mounted filesystems, compact `SOURCE:GUEST` display form
    /// (`SOURCE:GUEST:ro` when read-only); empty for mountless sandboxes.
    #[serde(default)]
    pub mounts: Vec<String>,
}

impl SandboxSummary {
    /// Compact display of the mounts for a dashboard card: the guest paths
    /// with a `←`-style source annotation only for bind mounts (named
    /// volumes read by their name). Empty → `—`.
    pub fn mounts_display(&self) -> String {
        if self.mounts.is_empty() {
            return "—".to_string();
        }
        self.mounts.join(" ")
    }
}

/// Compact one-token display of a mount for dashboard cards.
///
/// - Bind: `HOST⇄GUEST` (writable) / `HOST⇢GUEST` (read-only) — the arrows
///   distinguish the sbx workspace bind (same path both sides) at a glance.
/// - Named volume: `name:GUEST`.
/// - Other kinds (Disk/Tmpfs/…): the guest path only.
pub fn mount_display(kind: &str, source: &str, guest: &str, readonly: bool) -> String {
    match kind {
        "Bind" => {
            let arrow = if readonly { "⇢" } else { "⇄" };
            format!("{source}{arrow}{guest}")
        }
        "Named" => format!("{source}:{guest}"),
        _ => guest.to_string(),
    }
}

// ---------- msb ps ----------

/// One row of `msb ps --format json`. Ports here are strings such as
/// `127.0.0.1:8080->80/tcp` (unlike `inspect`, which uses structured objects).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SandboxStatusRow {
    pub command: String,
    pub cpus: u32,
    pub image: String,
    pub max_cpus: u32,
    pub max_memory_mib: u64,
    pub memory_mib: u64,
    pub name: String,
    #[serde(default)]
    pub ports: Vec<String>,
    pub status: SandboxState,
    pub thp: String,
}

impl SandboxStatusRow {
    /// Ports normalized to structured mappings.
    pub fn parsed_ports(&self) -> Result<Vec<PublishedPort>> {
        self.ports
            .iter()
            .map(|p| PublishedPort::parse_ps(p))
            .collect()
    }
}

// ---------- msb inspect ----------

/// Full result of `msb inspect <name> --format json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SandboxInspect {
    /// Configuration of the currently booted VM.
    pub active_config: SandboxConfig,
    /// Persisted configuration used for the next start.
    pub config: SandboxConfig,
    pub created_at: DateTime<Utc>,
    pub name: String,
    pub pause: PauseInfo,
    #[serde(default)]
    pub pending_changes: Vec<serde_json::Value>,
    pub status: SandboxState,
    pub updated_at: DateTime<Utc>,
}

/// Pause state block from `msb inspect`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PauseInfo {
    pub capture_unavailable: Option<serde_json::Value>,
    pub paused: bool,
    pub recovery_required: bool,
}

/// Sandbox configuration (shared by `config` and `active_config`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SandboxConfig {
    pub deployment_profile: String,
    #[serde(default, deserialize_with = "null_to_default")]
    pub env: Vec<EnvVar>,
    pub external_mount_policy: String,
    pub image: ImageSpec,
    pub init: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "null_to_default")]
    pub labels: HashMap<String, String>,
    pub lifecycle: Lifecycle,
    #[serde(default, deserialize_with = "null_to_default")]
    pub manifest_digest: String,
    #[serde(default, deserialize_with = "null_to_default")]
    pub mounts: Vec<Mount>,
    pub name: String,
    pub network: NetworkConfig,
    #[serde(default, deserialize_with = "null_to_default")]
    pub patches: Vec<serde_json::Value>,
    pub pull_policy: String,
    pub resources: Resources,
    #[serde(default, deserialize_with = "null_to_default")]
    pub rlimits: Vec<serde_json::Value>,
    pub runtime: RuntimeConfig,
    pub security_profile: String,
}

/// Deserialize an explicit JSON `null` as the field's default value.
///
/// msb 0.7.2 persists unset optional fields as explicit `null`s (not absent
/// keys): `"workdir": null`, `"hostname": null`, — and, for CLI-created
/// sandboxes whose image resolves no workload, `runtime.cmd: null`
/// (`SandboxRuntimeOptions.cmd` is `Option<Vec<String>>`). `#[serde(default)]`
/// alone only covers *absent* keys, so an explicit null fails the DTO parse
/// with "invalid type: null, expected a sequence/column…" and (pre-2.8
/// semantics) took the whole sandbox list down. The SDK's spec mapper treats
/// these nulls as "unset" (`unwrap_or_default`); mirroring that at the
/// deserialization boundary keeps the JSON and the typed mapper consistent.
pub(crate) fn null_to_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de> + Default,
{
    let v: Option<T> = serde::Deserialize::deserialize(d)?;
    Ok(v.unwrap_or_default())
}

/// `shell: null` → the SDK default shell, mirroring the spec mapper's
/// `unwrap_or_else(|| "/bin/sh".to_string())` (`SandboxRuntimeOptions.shell`
/// is `Option<String>` and persists unset as null).
pub(crate) fn null_to_shell<'de, D>(d: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v: Option<String> = serde::Deserialize::deserialize(d)?;
    Ok(v.unwrap_or_else(|| "/bin/sh".to_string()))
}

/// `metrics_sample_interval_ms: null` → the SDK default (1000 ms), mirroring
/// the spec mapper's `unwrap_or(1000)` (SDK `None` means "disable sampling";
/// the DTO field is display-only).
pub(crate) fn null_to_metrics_interval<'de, D>(d: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v: Option<u64> = serde::Deserialize::deserialize(d)?;
    Ok(v.unwrap_or(1000))
}

/// `KEY=VALUE` entry from sandbox config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvVar {
    pub key: String,
    pub value: String,
}

/// Image source of a sandbox. `msb inspect` encodes this as an externally
/// tagged map (`{"Oci": {...}}`); unknown future image kinds are captured in
/// `other` instead of failing deserialization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageSpec {
    #[serde(rename = "Oci", default)]
    pub oci: Option<OciImageSpec>,
    #[serde(flatten)]
    pub other: HashMap<String, serde_json::Value>,
}

impl ImageSpec {
    /// Human-readable image reference, regardless of image kind.
    pub fn reference(&self) -> String {
        if let Some(oci) = &self.oci {
            return oci.reference.clone();
        }
        if let Some((kind, value)) = self.other.iter().next() {
            return format!("{kind}:{value:?}");
        }
        "unknown".to_string()
    }
}

/// OCI image reference + root disk spec.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OciImageSpec {
    pub reference: String,
    pub root_disk: RootDiskSpec,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// Root disk settings of an OCI image sandbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootDiskSpec {
    pub kind: String,
    #[serde(default)]
    pub size_mib: Option<u64>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// Lifecycle limits of a sandbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lifecycle {
    pub ephemeral: bool,
    #[serde(default)]
    pub idle_timeout_secs: Option<u64>,
    #[serde(default)]
    pub max_duration_secs: Option<u64>,
}

/// A filesystem mount. `type` distinguishes `Named`, `Bind`, `Disk`, `Tmpfs`
/// etc.; type-specific fields are optional and unknown keys are kept in `extra`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mount {
    #[serde(rename = "type")]
    pub kind: String,
    /// Mount point inside the guest.
    pub guest: String,
    /// Volume name for `Named` mounts.
    #[serde(default)]
    pub name: Option<String>,
    /// Host path for `Bind`/`Disk` mounts.
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub options: MountOptions,
    #[serde(default)]
    pub quota_mib: Option<u64>,
    #[serde(default)]
    pub follow_root_symlinks: Option<bool>,
    #[serde(default)]
    pub host_permissions: Option<String>,
    #[serde(default)]
    pub stat_virtualization: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl Mount {
    /// Whether this mount is read-only.
    pub fn is_readonly(&self) -> bool {
        self.options.readonly
    }

    /// Display source: volume name for named mounts, host path otherwise.
    pub fn source(&self) -> &str {
        if let Some(n) = &self.name {
            n
        } else if let Some(h) = &self.host {
            h
        } else {
            ""
        }
    }
}

/// Filesystem options on a mount.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MountOptions {
    #[serde(default)]
    pub readonly: bool,
    #[serde(default)]
    pub noexec: bool,
    #[serde(default)]
    pub nosuid: bool,
    #[serde(default)]
    pub nodev: bool,
}

/// Network configuration, including published ports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub enabled: bool,
    #[serde(default, deserialize_with = "null_to_default")]
    pub max_connections: Option<u32>,
    #[serde(default, deserialize_with = "null_to_default")]
    pub ports: Vec<PublishedPort>,
    #[serde(default)]
    pub strict: bool,
    #[serde(default)]
    pub trust_host_cas: bool,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// CPU and memory allocation (+ boot-time ceilings for live resize).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resources {
    pub cpus: u32,
    pub max_cpus: u32,
    pub max_memory_mib: u64,
    pub memory_mib: u64,
}

/// Runtime behavior: default command, shell, user, workdir, scripts.
///
/// Optional fields (`workdir`, `user`, `hostname`, …) are `Option` because
/// the SDK stores `null` for "unset" — a bare `String` would reject every
/// sandbox created without an explicit workdir (smoke-test finding 2.8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeConfig {
    /// Image/command workload; CLI-created sandboxes without a resolvable
    /// command persist `cmd: null` (SDK `Option<Vec<String>>`).
    #[serde(default, deserialize_with = "null_to_default")]
    pub cmd: Vec<String>,
    #[serde(default)]
    pub disable_metrics_sample: bool,
    #[serde(default)]
    pub entrypoint: Option<serde_json::Value>,
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default)]
    pub log_level: Option<String>,
    #[serde(default, deserialize_with = "null_to_metrics_interval")]
    pub metrics_sample_interval_ms: u64,
    #[serde(default, deserialize_with = "null_to_default")]
    pub scripts: HashMap<String, serde_json::Value>,
    #[serde(default, deserialize_with = "null_to_shell")]
    pub shell: String,
    #[serde(default)]
    pub user: Option<String>,
    pub workdir: Option<String>,
}

// ---------- msb metrics ----------

/// One metrics sample from `msb metrics --format json` (or one line of
/// `msb metrics --follow`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
    pub cpu_percent: f64,
    pub cpus: u32,
    #[serde(default)]
    pub disk_read_bytes: u64,
    #[serde(default)]
    pub disk_write_bytes: u64,
    #[serde(default)]
    pub memory_available_bytes: u64,
    pub memory_bytes: u64,
    #[serde(default)]
    pub memory_host_resident_bytes: u64,
    #[serde(default)]
    pub memory_limit_bytes: u64,
    pub name: String,
    #[serde(default)]
    pub net_rx_bytes: u64,
    #[serde(default)]
    pub net_tx_bytes: u64,
    pub state: SandboxState,
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub upper_free_bytes: Option<u64>,
    #[serde(default)]
    pub upper_host_allocated_bytes: Option<u64>,
    #[serde(default)]
    pub upper_used_bytes: Option<u64>,
    #[serde(default)]
    pub uptime_secs: f64,
    #[serde(default)]
    pub vcpu_time_ns: u64,
}

impl Metrics {
    /// Fraction of the memory limit currently used (0.0–1.0), if a limit exists.
    pub fn memory_fraction(&self) -> Option<f64> {
        (self.memory_limit_bytes > 0)
            .then(|| self.memory_bytes as f64 / self.memory_limit_bytes as f64)
    }
}

// ---------- msb volumes ----------

/// One row of `msb volumes --format json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Volume {
    #[serde(default)]
    pub capacity_bytes: Option<u64>,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub disk_format: Option<String>,
    #[serde(default)]
    pub disk_fstype: Option<String>,
    /// "dir" or "disk".
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub quota_mib: Option<u64>,
    #[serde(default)]
    pub used_bytes: u64,
}

// ---------- msb images ----------

/// One row of `msb images --format json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Image {
    pub architecture: String,
    pub created_at: DateTime<Utc>,
    pub digest: String,
    pub layer_count: u32,
    pub os: String,
    pub reference: String,
    pub size_bytes: u64,
}

impl Image {
    /// Human-readable display name for the picker: strips the registry
    /// host prefix and shortens digest-pinned references
    /// (`docker.io/library/debian@sha256:d5ce19d…` → `debian@sha256:d5ce19d`).
    /// The full [`Image::reference`] stays valid for creation — this is
    /// display-only.
    pub fn display_name(&self) -> String {
        display_name_for(&self.reference)
    }
}

/// Display transformation for image references (unit-tested).
fn display_name_for(reference: &str) -> String {
    let stripped = reference
        .strip_prefix("docker.io/library/")
        .unwrap_or(reference);
    if let Some((name, hex)) = stripped.split_once("@sha256:") {
        let short: String = hex.chars().take(7).collect();
        return format!("{name}@sha256:{short}");
    }
    stripped.to_string()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Fixture access for cross-module tests (`pub(crate)` in test builds).
    pub(crate) fn fixture(name: &str) -> &'static str {
        match name {
            "sandbox-list" => include_str!("fixtures/sandbox-list.json"),
            "sandbox-status" => include_str!("fixtures/sandbox-status.json"),
            "sandbox-inspect" => include_str!("fixtures/sandbox-inspect.json"),
            "sandbox-stored-null-workdir" => {
                include_str!("fixtures/sandbox-stored-null-workdir.json")
            }
            "sandbox-stored-cli-null-cmd" => {
                include_str!("fixtures/sandbox-stored-cli-null-cmd.json")
            }
            "metrics" => include_str!("fixtures/metrics.json"),
            "volumes" => include_str!("fixtures/volumes.json"),
            "images" => include_str!("fixtures/images.json"),
            other => panic!("unknown fixture: {other}"),
        }
    }

    // ---- SandboxState normalization ----

    #[test]
    fn sandbox_state_normalizes_case() {
        assert_eq!(SandboxState::from_str("Running"), SandboxState::Running);
        assert_eq!(SandboxState::from_str("running"), SandboxState::Running);
        assert_eq!(SandboxState::from_str("Stopped"), SandboxState::Stopped);
        assert_eq!(SandboxState::from_str("paused"), SandboxState::Paused);
        assert_eq!(
            SandboxState::from_str("migrating"),
            SandboxState::Unknown("migrating".into())
        );
    }

    #[test]
    fn sandbox_state_predicates() {
        assert!(SandboxState::Running.is_running());
        assert!(SandboxState::Stopped.is_startable());
        assert!(!SandboxState::Running.is_startable());
    }

    #[test]
    fn sandbox_state_deserializes_from_json() {
        let s: SandboxState = serde_json::from_str("\"Running\"").unwrap();
        assert_eq!(s, SandboxState::Running);
        let s: SandboxState = serde_json::from_str("\"exited\"").unwrap();
        assert_eq!(s, SandboxState::Exited);
    }

    // ---- PublishedPort ----

    #[test]
    fn published_port_to_cli_flag() {
        let p = PublishedPort {
            host_bind: "127.0.0.1".into(),
            host_port: 8080,
            guest_port: 80,
            protocol: "tcp".into(),
        };
        assert_eq!(p.to_cli_flag(), "8080:80");

        let p = PublishedPort {
            host_bind: "0.0.0.0".into(),
            host_port: 9090,
            guest_port: 90,
            protocol: "udp".into(),
        };
        assert_eq!(p.to_cli_flag(), "0.0.0.0:9090:90/udp");
    }

    #[test]
    fn published_port_parse_ps_full_bind() {
        let p = PublishedPort::parse_ps("127.0.0.1:8080->80/tcp").unwrap();
        assert_eq!(
            p,
            PublishedPort {
                host_bind: "127.0.0.1".into(),
                host_port: 8080,
                guest_port: 80,
                protocol: "tcp".into(),
            }
        );
    }

    #[test]
    fn published_port_parse_ps_wildcard_udp() {
        let p = PublishedPort::parse_ps("0.0.0.0:9090->90/udp").unwrap();
        assert_eq!(p.host_bind, "0.0.0.0");
        assert_eq!(p.host_port, 9090);
        assert_eq!(p.guest_port, 90);
        assert_eq!(p.protocol, "udp");
    }

    #[test]
    fn published_port_parse_ps_no_bind_defaults_loopback() {
        let p = PublishedPort::parse_ps("8080->80").unwrap();
        assert_eq!(p.host_bind, "127.0.0.1");
        assert_eq!(p.host_port, 8080);
        assert_eq!(p.protocol, "tcp");
    }

    #[test]
    fn published_port_parse_ps_rejects_garbage() {
        assert!(PublishedPort::parse_ps("8080").is_err());
        assert!(PublishedPort::parse_ps("abc->def").is_err());
        assert!(PublishedPort::parse_ps("8080->notaport").is_err());
    }

    #[test]
    fn published_port_display() {
        let p = PublishedPort {
            host_bind: "127.0.0.1".into(),
            host_port: 8080,
            guest_port: 80,
            protocol: "tcp".into(),
        };
        assert_eq!(p.display(), "127.0.0.1:8080→80");
        let p = PublishedPort {
            host_bind: "0.0.0.0".into(),
            host_port: 9090,
            guest_port: 90,
            protocol: "udp".into(),
        };
        assert_eq!(p.display(), "0.0.0.0:9090→90/udp");
    }

    // ---- fixtures: msb ls ----

    /// Minimal SandboxSummary for display-helper tests.
    fn summary_shape() -> SandboxSummary {
        SandboxSummary {
            created_at: DateTime::<Utc>::UNIX_EPOCH,
            image: "alpine".into(),
            name: "probe".into(),
            status: SandboxState::Running,
            workdir: None,
            mounts: Vec::new(),
        }
    }

    #[test]
    fn parse_sandbox_list_fixture() {
        let rows: Vec<SandboxSummary> = serde_json::from_str(fixture("sandbox-list")).unwrap();
        assert_eq!(rows.len(), 1);
        let s = &rows[0];
        assert_eq!(s.name, "tui-fixture");
        assert_eq!(s.image, "alpine");
        assert_eq!(s.status, SandboxState::Running);
        assert_eq!(s.created_at.format("%Y-%m-%d").to_string(), "2026-09-20");
        // `mounts` is not part of the ls JSON — defaults to empty.
        assert!(s.mounts.is_empty());
        assert_eq!(s.mounts_display(), "—");
    }

    // ---- mount display (dashboard card) ----

    #[test]
    fn mount_display_forms() {
        // Bind: ⇄ writable, ⇢ read-only (sbx workspace is a same-path bind).
        assert_eq!(
            mount_display("Bind", "/home/user/proj", "/home/user/proj", false),
            "/home/user/proj⇄/home/user/proj"
        );
        assert_eq!(
            mount_display("Bind", "/tmp/x", "/data", true),
            "/tmp/x⇢/data"
        );
        // Named volume: name:guest.
        assert_eq!(
            mount_display("Named", "tui-data", "/data", false),
            "tui-data:/data"
        );
        // Other kinds (Disk/Tmpfs/…): guest path only.
        assert_eq!(mount_display("Tmpfs", "", "/cache", false), "/cache");
    }

    #[test]
    fn mounts_display_joins_and_defaults() {
        let mut s = summary_shape();
        s.mounts = vec!["/home/u/proj⇄/home/u/proj".into(), "tui-data:/data".into()];
        assert_eq!(
            s.mounts_display(),
            "/home/u/proj⇄/home/u/proj tui-data:/data"
        );
        s.mounts.clear();
        assert_eq!(s.mounts_display(), "—");
    }

    // ---- fixtures: msb ps ----

    #[test]
    fn parse_sandbox_status_fixture() {
        let row: SandboxStatusRow = serde_json::from_str(fixture("sandbox-status")).unwrap();
        assert_eq!(row.name, "tui-fixture");
        assert_eq!(row.image, "alpine");
        assert_eq!(row.cpus, 2);
        assert_eq!(row.max_cpus, 2);
        assert_eq!(row.memory_mib, 768);
        assert_eq!(row.max_memory_mib, 768);
        assert_eq!(row.status, SandboxState::Running);
        assert_eq!(row.thp, "madvise");
        assert_eq!(row.command, "\"/bin/sh\"");

        let ports = row.parsed_ports().unwrap();
        assert_eq!(ports.len(), 2);
        assert_eq!(ports[0].to_cli_flag(), "8080:80");
        assert_eq!(ports[1].to_cli_flag(), "0.0.0.0:9090:90/udp");
    }

    // ---- fixtures: msb inspect ----

    #[test]
    fn parse_sandbox_inspect_fixture() {
        let insp: SandboxInspect = serde_json::from_str(fixture("sandbox-inspect")).unwrap();
        assert_eq!(insp.name, "tui-fixture");
        assert_eq!(insp.status, SandboxState::Running);
        assert_eq!(insp.pending_changes.len(), 0);
        assert!(!insp.pause.paused);
        assert!(!insp.pause.recovery_required);

        let cfg = &insp.active_config;
        assert_eq!(cfg.name, "tui-fixture");
        assert_eq!(cfg.deployment_profile, "single_tenant");
        assert_eq!(cfg.image.reference(), "alpine");
        assert_eq!(
            cfg.image.oci.as_ref().unwrap().root_disk.size_mib,
            Some(4096)
        );
        assert!(cfg.other_config_is_empty());
    }

    #[test]
    fn inspect_env_and_labels() {
        let insp: SandboxInspect = serde_json::from_str(fixture("sandbox-inspect")).unwrap();
        let cfg = &insp.active_config;
        assert!(cfg.env.iter().any(|e| e.key == "PATH"));
        let app_env = cfg.env.iter().find(|e| e.key == "APP_ENV").unwrap();
        assert_eq!(app_env.value, "dev");
        assert_eq!(cfg.labels.get("app").map(String::as_str), Some("demo"));
    }

    #[test]
    fn stored_config_with_null_workdir_parses() {
        // Regression (2.8 finding 2): sandboxes created by the TUI store
        // `"workdir": null` (+ null hostname/user/log_level, `entrypoint: []`,
        // network tls/dns sub-objects the older fixture lacks). A strict
        // `workdir: String` made the WHOLE sandbox list fail with
        // "invalid type: null, expected a string at column 201".
        let raw = fixture("sandbox-stored-null-workdir");
        let cfg: SandboxConfig = serde_json::from_str(raw).expect("stored config must parse");
        assert_eq!(cfg.name, "Test2");
        assert_eq!(cfg.image.reference(), "debian");
        assert_eq!(cfg.runtime.workdir, None, "null workdir maps to None");
        assert_eq!(cfg.runtime.user, None);
        assert_eq!(cfg.runtime.hostname, None);
        assert_eq!(cfg.runtime.shell, "/bin/sh");
        assert_eq!(cfg.runtime.cmd, vec!["bash".to_string()]);
        assert_eq!(cfg.resources.memory_mib, 512);
    }

    #[test]
    fn stored_cli_config_with_null_cmd_and_entrypoint_parses() {
        // Regression (colleague home-lab report, 2026-10-01): a CLI-created
        // sandbox from an image whose CMD/ENTRYPOINT resolve to no workload
        // persists `runtime.cmd: null` / `entrypoint: null` (the 0.7.2
        // schema serializes unset `Option`s explicitly as null — same
        // mechanism as the null workdir above). The fleet-wide error was
        //  "sandbox list: … 'hermes': stored config for 'hermes' does not
        //   match the expected schema: invalid type: null, expected a
        //   sequence at line 1 column 344"
        let raw = fixture("sandbox-stored-cli-null-cmd");
        let cfg: SandboxConfig =
            serde_json::from_str(raw).expect("CLI-created config with null cmd must parse");
        assert_eq!(cfg.name, "hermes");
        assert_eq!(
            cfg.image.reference(),
            "nousresearch/hermes-agent:v2026.9.21"
        );
        assert!(
            cfg.runtime.cmd.is_empty(),
            "null cmd maps to empty (CLI semantics: no workload)"
        );
        assert_eq!(cfg.runtime.entrypoint, None, "null entrypoint maps to None");
        // Mounts survive: the sandbox carries two `--mount-dir` mounts.
        assert_eq!(cfg.mounts.len(), 2);
        // Networking enabled with no published ports.
        assert!(cfg.network.enabled);
        assert!(cfg.network.ports.is_empty());
    }

    #[test]
    fn stored_config_null_sequences_map_to_empty() {
        // Any sequence/map field the persisted schema can carry as an
        // explicit null must deserialize to its empty default — the SDK
        // writes nulls, not absent keys, and `#[serde(default)]` alone
        // only covers absent keys.
        let template: serde_json::Value =
            serde_json::from_str(fixture("sandbox-stored-null-workdir")).unwrap();
        for (label, field, is_empty) in [
            ("env", &["env"] as &[&str], true),
            ("mounts", &["mounts"], true),
            ("patches", &["patches"], true),
            ("rlimits", &["rlimits"], true),
            ("cmd", &["runtime", "cmd"], true),
            ("scripts", &["runtime", "scripts"], true),
            ("ports", &["network", "ports"], true),
        ] {
            let mut v = template.clone();
            let target = field.iter().fold(&mut v, |node, key| &mut node[*key]);
            *target = serde_json::Value::Null;
            let cfg: SandboxConfig = serde_json::from_value(v)
                .unwrap_or_else(|e| panic!("{label}: null {label} must parse: {e}"));
            let empty = match label {
                "env" => cfg.env.is_empty(),
                "mounts" => cfg.mounts.is_empty(),
                "patches" => cfg.patches.is_empty(),
                "rlimits" => cfg.rlimits.is_empty(),
                "cmd" => cfg.runtime.cmd.is_empty(),
                "scripts" => cfg.runtime.scripts.is_empty(),
                "ports" => cfg.network.ports.is_empty(),
                _ => unreachable!(),
            };
            assert!(is_empty == empty, "{label}: null maps to empty");
        }
        // And everything at once still parses to the defaults.
        let mut v = template.clone();
        let null = serde_json::Value::Null;
        v["env"] = null.clone();
        v["mounts"] = null.clone();
        v["patches"] = null.clone();
        v["rlimits"] = null.clone();
        v["runtime"]["cmd"] = null.clone();
        v["runtime"]["scripts"] = null.clone();
        v["network"]["ports"] = null;
        let cfg: SandboxConfig = serde_json::from_value(v).expect("all-null sequences parse");
        assert!(cfg.env.is_empty() && cfg.mounts.is_empty() && cfg.patches.is_empty());
        assert!(cfg.rlimits.is_empty() && cfg.runtime.cmd.is_empty());
        assert!(cfg.runtime.scripts.is_empty() && cfg.network.ports.is_empty());
    }

    #[test]
    fn stored_config_null_scalars_map_to_sdk_defaults() {
        // Scalar fields mirror the SDK spec mapper's unwrap-ors: a persisted
        // null maps to the same default the typed conversion produces
        // (shell "/bin/sh", sampling 1000 ms, absent digest "").
        let mut v: serde_json::Value =
            serde_json::from_str(fixture("sandbox-stored-null-workdir")).unwrap();
        v["runtime"]["shell"] = serde_json::Value::Null;
        v["runtime"]["metrics_sample_interval_ms"] = serde_json::Value::Null;
        v["manifest_digest"] = serde_json::Value::Null;
        let cfg: SandboxConfig = serde_json::from_value(v).expect("null scalars parse");
        assert_eq!(cfg.runtime.shell, "/bin/sh");
        assert_eq!(cfg.runtime.metrics_sample_interval_ms, 1000);
        assert_eq!(cfg.manifest_digest, "");
    }

    #[test]
    fn inspect_network_ports() {
        let insp: SandboxInspect = serde_json::from_str(fixture("sandbox-inspect")).unwrap();
        let net = &insp.active_config.network;
        assert!(net.enabled);
        assert!(!net.strict);
        assert!(!net.trust_host_cas);
        assert_eq!(net.max_connections, None);
        assert_eq!(net.ports.len(), 2);
        assert_eq!(
            net.ports[0],
            PublishedPort {
                host_bind: "127.0.0.1".into(),
                host_port: 8080,
                guest_port: 80,
                protocol: "tcp".into(),
            }
        );
        assert_eq!(
            net.ports[1],
            PublishedPort {
                host_bind: "0.0.0.0".into(),
                host_port: 9090,
                guest_port: 90,
                protocol: "udp".into(),
            }
        );
    }

    #[test]
    fn inspect_mounts() {
        let insp: SandboxInspect = serde_json::from_str(fixture("sandbox-inspect")).unwrap();
        let mounts = &insp.active_config.mounts;
        assert_eq!(mounts.len(), 2);

        let named = mounts.iter().find(|m| m.kind == "Named").unwrap();
        assert_eq!(named.guest, "/data");
        assert_eq!(named.name.as_deref(), Some("tui-data"));
        assert!(!named.is_readonly());
        assert_eq!(named.source(), "tui-data");

        let bind = mounts.iter().find(|m| m.kind == "Bind").unwrap();
        assert_eq!(bind.guest, "/mnt/src");
        assert_eq!(bind.host.as_deref(), Some("/tmp/opencode/probe-src"));
        assert!(bind.is_readonly());
        assert_eq!(bind.source(), "/tmp/opencode/probe-src");
    }

    #[test]
    fn inspect_resources_runtime_lifecycle() {
        let insp: SandboxInspect = serde_json::from_str(fixture("sandbox-inspect")).unwrap();
        let res = &insp.active_config.resources;
        assert_eq!(res.cpus, 2);
        assert_eq!(res.max_cpus, 2);
        assert_eq!(res.memory_mib, 768);
        assert_eq!(res.max_memory_mib, 768);

        let rt = &insp.active_config.runtime;
        assert_eq!(rt.cmd, vec!["/bin/sh"]);
        assert_eq!(rt.shell, "/bin/sh");
        assert_eq!(rt.workdir.as_deref(), Some("/"));
        assert_eq!(rt.user, None);

        let lc = &insp.active_config.lifecycle;
        assert!(!lc.ephemeral);
        assert_eq!(lc.idle_timeout_secs, None);
        assert_eq!(lc.max_duration_secs, None);

        assert_eq!(insp.active_config.pull_policy, "IfMissing");
        assert_eq!(insp.active_config.security_profile, "default");
    }

    // ---- fixtures: msb metrics ----

    #[test]
    fn parse_metrics_fixture() {
        let rows: Vec<Metrics> = serde_json::from_str(fixture("metrics")).unwrap();
        assert_eq!(rows.len(), 1);
        let m = &rows[0];
        assert_eq!(m.name, "tui-fixture");
        assert_eq!(m.state, SandboxState::Running);
        assert_eq!(m.cpus, 2);
        assert!(m.cpu_percent > 0.0);
        assert_eq!(m.memory_limit_bytes, 805306368);
        assert_eq!(m.memory_bytes, 59441152);
        assert_eq!(m.net_tx_bytes, 596);
        assert_eq!(m.net_rx_bytes, 0);
        assert!(m.uptime_secs > 0.0);
        assert_eq!(m.upper_used_bytes, Some(69632));

        let frac = m.memory_fraction().unwrap();
        assert!(frac > 0.0 && frac < 0.2, "frac={frac}");
    }

    #[test]
    fn metrics_memory_fraction_without_limit() {
        let mut m: Metrics = serde_json::from_str(fixture("metrics"))
            .map(|v: Vec<Metrics>| v.into_iter().next().unwrap())
            .unwrap();
        m.memory_limit_bytes = 0;
        assert_eq!(m.memory_fraction(), None);
    }

    // ---- fixtures: msb volumes ----

    #[test]
    fn parse_volumes_fixture() {
        let vols: Vec<Volume> = serde_json::from_str(fixture("volumes")).unwrap();
        assert_eq!(vols.len(), 1);
        let v = &vols[0];
        assert_eq!(v.name, "tui-data");
        assert_eq!(v.kind, "dir");
        assert_eq!(v.capacity_bytes, None);
        assert_eq!(v.quota_mib, None);
        assert_eq!(v.used_bytes, 0);
        assert_eq!(v.disk_format, None);
    }

    // ---- fixtures: msb images ----

    #[test]
    fn parse_images_fixture() {
        let imgs: Vec<Image> = serde_json::from_str(fixture("images")).unwrap();
        assert_eq!(imgs.len(), 1);
        let i = &imgs[0];
        assert_eq!(i.reference, "alpine");
        assert_eq!(i.architecture, "amd64");
        assert_eq!(i.os, "linux");
        assert_eq!(i.layer_count, 1);
        assert_eq!(i.size_bytes, 3849738);
        assert!(i.digest.starts_with("sha256:"));
    }

    // ---- image display names (digest-pinned snapshot artifacts) ----

    #[test]
    fn display_name_strips_registry_and_shortens_digest() {
        // Snapshot restores materialize the base image under its fully
        // qualified digest reference; the picker shows a readable form.
        let mut i = image(
            "docker.io/library/debian@sha256:d5ce19d4736f0ebbacd686d1040271a5aeb0cc920f5990c1bfae1717627f0674",
        );
        i.digest = "sha256:d5ce19d4736f0e".into();
        assert_eq!(i.display_name(), "debian@sha256:d5ce19d");
    }

    #[test]
    fn display_name_keeps_plain_references() {
        assert_eq!(image("debian").display_name(), "debian");
        assert_eq!(image("python:3.12").display_name(), "python:3.12");
        assert_eq!(
            image("my.registry.io/custom:1.0").display_name(),
            "my.registry.io/custom:1.0",
            "registry hosts other than docker.io are kept"
        );
    }

    fn image(reference: &str) -> Image {
        Image {
            architecture: "amd64".into(),
            created_at: DateTime::<Utc>::UNIX_EPOCH,
            digest: "sha256:deadbeef".into(),
            layer_count: 1,
            os: "linux".into(),
            reference: reference.into(),
            size_bytes: 1024,
        }
    }

    // ---- port spec parsing (CLI form, shared with templates) ----

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
}

impl SandboxConfig {
    /// Test helper: true when no unknown image variants were captured.
    pub(crate) fn other_config_is_empty(&self) -> bool {
        self.image.other.is_empty()
    }
}
