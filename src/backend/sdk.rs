//! SDK backend: talks to the local microsandbox runtime through the typed
//! `microsandbox` 0.7.2 SDK instead of shelling out to the `msb` CLI.
//!
//! Mapping contract: the SDK's rich types (`SandboxConfig`, `SandboxMetrics`,
//! `PublishedPort`, `LogEntry`, …) are converted at this boundary into the
//! TUI-internal DTOs in [`crate::models`]. Nothing SDK-specific leaks past this
//! module — the rest of the TUI depends only on [`MsbBackend`] + `models`.
//!
//! Persistence: SDK sandboxes default to *attached* mode (the VM dies with the
//! TUI process). Every create/start/restart here uses detached mode so
//! sandboxes outlive the dashboard, matching the CLI's behavior.

#![allow(dead_code)] // trait surface is broader than current callers

use std::net::IpAddr;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use microsandbox::sandbox::{SandboxBuilder, SandboxHandle, SandboxSpec, SandboxStatus};
use microsandbox_network::config::PortProtocol;
use microsandbox_types::{LogSource, NetworkSpec, RootfsSource, VolumeMount};
use tokio_stream::StreamExt as _;

use crate::models::{
    EnvVar, Image, Lifecycle, Metrics, Mount, MountOptions, NetworkConfig, OciImageSpec, PauseInfo,
    PublishedPort, Resources, RootDiskSpec, RuntimeConfig, SandboxConfig, SandboxInspect,
    SandboxState, SandboxStatusRow, SandboxSummary, Volume,
};

use super::{CreateSpec, ExecOutput, LogLine, LogStream, MsbBackend};

/// Backend backed by the microsandbox SDK against the local runtime.
///
/// Holds an explicit [`LocalBackend`] (the same lazy singleton the SDK would
/// resolve by default) so fleet metrics can be fetched via
/// `all_sandbox_metrics_reports_local`.
#[derive(Clone)]
pub struct SdkBackend {
    local: Arc<microsandbox::LocalBackend>,
}

impl SdkBackend {
    /// Construct from the ambient default local configuration
    /// (`~/.microsandbox/config.json` or defaults, honoring `MSB_CONFIG_PATH`).
    pub fn new() -> Result<Self> {
        let local = microsandbox::LocalBackend::lazy();
        Ok(Self {
            local: Arc::new(local),
        })
    }

    /// The wrapped local backend (used by runtime-management code).
    pub fn local(&self) -> &Arc<microsandbox::LocalBackend> {
        &self.local
    }

    // ---- shared SDK calls ----

    async fn handle(&self, name: &str) -> Result<SandboxHandle> {
        microsandbox::Sandbox::get(name)
            .await
            .with_context(|| format!("sandbox '{name}' not found"))
    }

    /// Full lifecycle info for a sandbox: the SDK handle carries the stored
    /// config plus local state (status, timestamps).
    async fn inspect_from_local_state(&self, handle: &SandboxHandle) -> Result<SandboxInspect> {
        let Some(local) = handle.local() else {
            return Err(anyhow!(
                "sandbox '{}' is not a local sandbox",
                handle.name()
            ));
        };
        let cfg = self.sandbox_config_from_json(handle)?;
        let status = map_status(local.status);
        Ok(SandboxInspect {
            active_config: cfg.clone(),
            config: cfg,
            created_at: local.created_at.unwrap_or_else(Utc::now),
            name: handle.name().to_string(),
            pause: PauseInfo {
                capture_unavailable: None,
                paused: matches!(local.status, SandboxStatus::Paused),
                recovery_required: false,
            },
            pending_changes: Vec::new(),
            status,
            updated_at: local.updated_at.unwrap_or_else(Utc::now),
        })
    }

    /// Deserialize the stored config JSON into our DTO layer.
    ///
    /// The stored JSON is the SDK's `SandboxConfig` (spec-flattened). We parse
    /// it into our own `SandboxConfig` DTO, which mirrors the persisted schema
    /// (same field names as `msb inspect` output).
    fn sandbox_config_from_json(&self, handle: &SandboxHandle) -> Result<SandboxConfig> {
        let json = handle.config_json();
        serde_json::from_str(json).with_context(|| {
            format!(
                "stored config for '{}' does not match the expected schema",
                handle.name()
            )
        })
    }
}

// ---------------------------------------------------------------------------
// DTO conversions (SDK -> models.rs)
// ---------------------------------------------------------------------------

/// Map an SDK [`SandboxStatus`] to our normalized state.
pub(crate) fn map_status(status: SandboxStatus) -> SandboxState {
    use SandboxStatus as S;
    match status {
        S::Created => SandboxState::Created,
        S::Starting => SandboxState::Running,
        S::Running => SandboxState::Running,
        S::Draining => SandboxState::Running,
        S::Paused => SandboxState::Paused,
        S::Stopped => SandboxState::Stopped,
        S::Crashed => SandboxState::Crashed,
    }
}

/// Map the metrics-report row state (Running/Stalled/Exited).
pub(crate) fn map_metrics_state(state: microsandbox::SandboxMetricsState) -> SandboxState {
    use microsandbox::SandboxMetricsState as M;
    match state {
        M::Running => SandboxState::Running,
        M::Stalled => SandboxState::Stalled,
        M::Exited => SandboxState::Exited,
    }
}

/// Convert SDK published-port data (IpAddr bind) into our string-bind DTO.
pub(crate) fn map_port(p: &microsandbox_network::config::PublishedPort) -> PublishedPort {
    PublishedPort {
        host_bind: p.host_bind.to_string(),
        host_port: p.host_port,
        guest_port: p.guest_port,
        protocol: match p.protocol {
            PortProtocol::Udp => "udp".to_string(),
            PortProtocol::Tcp => "tcp".to_string(),
        },
    }
}

/// Convert our DTO back into the SDK's typed port (used by create + recreate).
pub(crate) fn sdk_port(p: &PublishedPort) -> Result<microsandbox_network::config::PublishedPort> {
    Ok(microsandbox_network::config::PublishedPort {
        host_bind: p.host_bind.parse::<IpAddr>().map_err(|_| {
            anyhow!(
                "invalid host bind address '{}' for port {}",
                p.host_bind,
                p.host_port
            )
        })?,
        host_port: p.host_port,
        guest_port: p.guest_port,
        protocol: match p.protocol.as_str() {
            "udp" => PortProtocol::Udp,
            _ => PortProtocol::Tcp,
        },
    })
}

/// Convert an SDK `SandboxMetrics` sample into our DTO.
pub(crate) fn map_metrics(
    name: &str,
    state: SandboxState,
    m: &microsandbox::SandboxMetrics,
) -> Metrics {
    Metrics {
        cpu_percent: m.cpu_percent as f64,
        cpus: 0, // resolved from the report below when available
        disk_read_bytes: m.disk_read_bytes,
        disk_write_bytes: m.disk_write_bytes,
        memory_available_bytes: m.memory_available_bytes.unwrap_or(0),
        memory_bytes: m.memory_bytes,
        memory_host_resident_bytes: m.memory_host_resident_bytes.unwrap_or(0),
        memory_limit_bytes: m.memory_limit_bytes,
        name: name.to_string(),
        net_rx_bytes: m.net_rx_bytes,
        net_tx_bytes: m.net_tx_bytes,
        state,
        timestamp: m.timestamp,
        upper_free_bytes: m.upper_free_bytes,
        upper_host_allocated_bytes: m.upper_host_allocated_bytes,
        upper_used_bytes: m.upper_used_bytes,
        uptime_secs: m.uptime.as_secs_f64(),
        vcpu_time_ns: m.vcpu_time_ns,
    }
}

/// Convert an SDK metrics report (sample + catalog context) into our DTO.
pub(crate) fn map_report(report: &microsandbox::SandboxMetricsReport) -> Metrics {
    let mut m = map_metrics(
        &report.name,
        map_metrics_state(report.state),
        &report.metrics,
    );
    if let Some(cpus) = report.cpus {
        m.cpus = cpus;
    }
    m
}

/// Derive the image reference from the persisted rootfs source.
fn image_reference(spec: &RootfsSource) -> String {
    match spec {
        RootfsSource::Oci(oci) => oci.reference.clone(),
        RootfsSource::Bind { path, .. } => format!("bind:{}", path.display()),
        RootfsSource::DiskImage { path, .. } => format!("disk:{}", path.display()),
    }
}

/// Convert a persisted SDK mount into our display DTO.
fn map_mount(m: &VolumeMount) -> Mount {
    // Every variant carries `options` inline; extract via the shared shape.
    let sdk_options = match m {
        VolumeMount::Bind { options, .. }
        | VolumeMount::Named { options, .. }
        | VolumeMount::Owned { options, .. }
        | VolumeMount::Tmpfs { options, .. }
        | VolumeMount::DiskImage { options, .. } => *options,
    };
    let options = MountOptions {
        readonly: sdk_options.readonly,
        noexec: sdk_options.noexec,
        nosuid: sdk_options.nosuid,
        nodev: sdk_options.nodev,
    };

    let (kind, name, host, quota_mib, follow_root_symlinks) = match m {
        VolumeMount::Named { name, .. } => {
            ("Named".to_string(), Some(name.clone()), None, None, None)
        }
        VolumeMount::Bind {
            host,
            quota_mib,
            follow_root_symlinks,
            ..
        } => (
            "Bind".to_string(),
            None,
            Some(host.display().to_string()),
            quota_mib.map(|q| q as u64),
            Some(*follow_root_symlinks),
        ),
        VolumeMount::Owned { .. } => ("Owned".to_string(), None, None, None, None),
        VolumeMount::Tmpfs { .. } => ("Tmpfs".to_string(), None, None, None, None),
        VolumeMount::DiskImage { .. } => ("DiskImage".to_string(), None, None, None, None),
    };

    Mount {
        kind,
        guest: m.guest().to_string(),
        name,
        host,
        options,
        quota_mib,
        follow_root_symlinks,
        host_permissions: None,
        stat_virtualization: None,
        extra: Default::default(),
    }
}

/// Compact `SOURCE:GUEST[:ro]`-style display strings for a config's mounts,
/// rendered on dashboard cards (`mount_display` gives the exact token shape).
pub(crate) fn map_mounts(cfg: &SandboxConfig) -> Vec<String> {
    cfg.mounts
        .iter()
        .map(|m| crate::models::mount_display(&m.kind, m.source(), &m.guest, m.is_readonly()))
        .collect()
}

/// Convert the network portion of a stored spec into our DTO.
///
/// Note: `NetworkSpec.ports` are `PublishedPortSpec` (types crate, `String`
/// bind), while `NetworkBuilder` produces network-crate `PublishedPort`
/// (`IpAddr` bind). This maps the persisted shape.
fn map_network(spec: &NetworkSpec) -> NetworkConfig {
    NetworkConfig {
        enabled: spec.enabled,
        max_connections: spec.max_tcp_connections.map(|c| c as u32),
        ports: spec.ports.iter().map(map_port_spec).collect(),
        strict: spec.strict,
        trust_host_cas: spec.trust_host_cas,
        extra: Default::default(),
    }
}

/// Map a persisted `PublishedPortSpec` (String bind) into our DTO.
fn map_port_spec(p: &microsandbox_types::PublishedPortSpec) -> PublishedPort {
    PublishedPort {
        host_bind: p.host_bind.clone(),
        host_port: p.host_port,
        guest_port: p.guest_port,
        protocol: match p.protocol {
            microsandbox_types::PortProtocol::Udp => "udp".to_string(),
            microsandbox_types::PortProtocol::Tcp => "tcp".to_string(),
        },
    }
}

/// Convert a full SDK spec (the `SandboxSpec` half of a stored config) into our
/// display-oriented `SandboxConfig` DTO. Reads only the fields the TUI shows;
/// SDK-only operation state is intentionally dropped.
fn map_config(spec: &SandboxSpec) -> SandboxConfig {
    SandboxConfig {
        deployment_profile: format!("{:?}", spec.deployment_profile),
        env: spec
            .env
            .iter()
            .map(|e| EnvVar {
                key: e.key.clone(),
                value: e.value.clone(),
            })
            .collect(),
        external_mount_policy: "strict".to_string(),
        image: crate::models::ImageSpec {
            oci: Some(OciImageSpec {
                reference: image_reference(&spec.image),
                root_disk: RootDiskSpec {
                    kind: "managed".to_string(),
                    size_mib: None,
                    extra: Default::default(),
                },
                extra: Default::default(),
            }),
            other: Default::default(),
        },
        init: None,
        labels: spec.labels.clone().into_iter().collect(),
        lifecycle: Lifecycle {
            ephemeral: spec.lifecycle.ephemeral,
            idle_timeout_secs: spec.lifecycle.idle_timeout_secs,
            max_duration_secs: spec.lifecycle.max_duration_secs,
        },
        manifest_digest: String::new(),
        mounts: spec.mounts.iter().map(map_mount).collect(),
        name: spec.name.clone(),
        network: map_network(&spec.network),
        patches: Vec::new(),
        pull_policy: format!("{:?}", spec.pull_policy),
        resources: Resources {
            cpus: spec.resources.cpus as u32,
            max_cpus: spec.resources.max_cpus as u32,
            max_memory_mib: spec.resources.max_memory_mib as u64,
            memory_mib: spec.resources.memory_mib as u64,
        },
        rlimits: Vec::new(),
        runtime: RuntimeConfig {
            cmd: spec.runtime.cmd.clone().unwrap_or_default(),
            disable_metrics_sample: spec.runtime.disable_metrics_sample,
            entrypoint: spec
                .runtime
                .entrypoint
                .clone()
                .map(|e| serde_json::json!(e)),
            hostname: spec.runtime.hostname.clone(),
            log_level: spec
                .runtime
                .log_level
                .map(|l| format!("{l:?}").to_lowercase()),
            metrics_sample_interval_ms: spec.runtime.metrics_sample_interval_ms.unwrap_or(1000),
            scripts: spec
                .runtime
                .scripts
                .iter()
                .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                .collect(),
            shell: spec
                .runtime
                .shell
                .clone()
                .unwrap_or_else(|| "/bin/sh".to_string()),
            user: spec.runtime.user.clone(),
            workdir: spec.runtime.workdir.clone(),
        },
        security_profile: format!("{:?}", spec.security_profile),
    }
}

/// Convert one SDK log entry into the `LogLine` the UI consumes.
fn map_log_line(e: microsandbox::logs::LogEntry) -> Option<LogLine> {
    let data = String::from_utf8_lossy(&e.data).into_owned();
    let id = e.session_id.unwrap_or(0);
    let source = match e.source {
        LogSource::Stdout => "stdout",
        LogSource::Stderr => "stderr",
        LogSource::Output => "output",
        LogSource::System => "system",
    };
    Some(LogLine {
        id,
        source: source.to_string(),
        data,
        timestamp: e.timestamp,
    })
}

/// Map a `CreateSpec`'s network profile name to a policy.
///
/// Phase-1 form profiles: public / private / host / none. SDK policies expand
/// into first-match-wins rules via `NetworkPolicy::from_profiles`.
fn network_policy_for(profile: &str) -> Option<microsandbox::NetworkPolicy> {
    match profile {
        "public" => Some(microsandbox::NetworkPolicy::from_profiles([
            microsandbox::NetworkProfile::Public,
        ])),
        "private" => Some(microsandbox::NetworkPolicy::from_profiles([
            microsandbox::NetworkProfile::Private,
        ])),
        "host" => Some(microsandbox::NetworkPolicy::from_profiles([
            microsandbox::NetworkProfile::Host,
        ])),
        _ => None,
    }
}

/// Convert our `CreateSpec` into an SDK `SandboxBuilder`.
///
/// Exposed for unit tests (the builder itself requires a live runtime to
/// resolve, so we test the pieces instead of the final create call).
pub(crate) fn apply_spec_to_builder(builder: SandboxBuilder, spec: &CreateSpec) -> SandboxBuilder {
    // The image is mandatory — the SDK validates it as "image source is
    // required" otherwise (regression found in the phase2.5 smoke test).
    let mut builder = builder.detached(true).image(spec.image.as_str());

    if let Some(cpus) = spec.cpus {
        builder = builder.cpus(cpus as u8);
    }
    if let Some(mib) = spec.memory.as_deref().and_then(parse_memory_mib) {
        builder = builder.memory(mib);
    }
    if let Some(wd) = &spec.workdir {
        builder = builder.workdir(wd);
    }
    for e in &spec.env {
        if let Some((k, v)) = e.split_once('=') {
            builder = builder.env(k, v);
        }
    }
    for l in &spec.labels {
        if let Some((k, v)) = l.split_once('=') {
            builder = builder.label(k, v);
        }
    }

    // Network: profile preset + published ports via the typed builder.
    builder = builder.network(|n| {
        let mut n = match &spec.net_profile {
            Some(profile) => match network_policy_for(profile) {
                Some(policy) => n.enabled(true).policy(policy),
                None => n, // "none" or unknown → leave defaults; caller disables below
            },
            None => n,
        };
        for p in &spec.ports {
            if let Ok(sp) = sdk_port(p) {
                n = match sp.protocol {
                    PortProtocol::Udp => n.port_udp_bind(sp.host_bind, sp.host_port, sp.guest_port),
                    PortProtocol::Tcp => n.port_bind(sp.host_bind, sp.host_port, sp.guest_port),
                };
            }
        }
        n
    });

    // Volume mounts: parse "SOURCE:GUEST" strings into typed mounts.
    for v in &spec.volumes {
        if let Some(mount) = parse_volume_string(v) {
            let ParsedMount { guest, kind } = mount;
            builder = builder.volume(guest, move |m| match kind {
                ParsedMountKind::Bind(host) => m.bind(host),
                ParsedMountKind::Named(name) => m.named(name),
                ParsedMountKind::Tmpfs => m.tmpfs(),
            });
        }
    }

    builder
}

/// Parse a memory string like `512M` / `1G` / `1024` (MiB) into MiB.
pub(crate) fn parse_memory_mib(s: &str) -> Option<u32> {
    let s = s.trim();
    let (num, mult) = match s.chars().last()? {
        // K is below MiB precision — rejected below via mult == 0.
        'K' | 'k' => (s.get(..s.len() - 1)?, 0),
        'M' | 'm' => (s.get(..s.len() - 1)?, 1),
        'G' | 'g' => (s.get(..s.len() - 1)?, 1024),
        'T' | 't' => (s.get(..s.len() - 1)?, 1024 * 1024),
        _ => (s, 1),
    };
    if mult == 0 {
        return None; // K is below MiB precision — reject.
    }
    num.parse::<u64>().ok()?.checked_mul(mult)?.try_into().ok()
}

/// One parsed volume-mount string, ready to apply to a `MountBuilder`.
struct ParsedMount {
    guest: String,
    kind: ParsedMountKind,
}

enum ParsedMountKind {
    /// Host directory bind mount (source contains `/` or starts with `.`).
    Bind(String),
    /// Named volume.
    Named(String),
    /// Tmpfs (source is `tmpfs`).
    Tmpfs,
}

/// Parse `SOURCE:GUEST` (bind or named volume) or `tmpfs:GUEST` mount strings.
fn parse_volume_string(s: &str) -> Option<ParsedMount> {
    let (source, guest) = s.split_once(':')?;
    let kind = if source == "tmpfs" {
        ParsedMountKind::Tmpfs
    } else if source.contains('/') || source.starts_with('.') {
        ParsedMountKind::Bind(source.to_string())
    } else {
        ParsedMountKind::Named(source.to_string())
    };
    Some(ParsedMount {
        guest: guest.to_string(),
        kind,
    })
}

#[async_trait]
impl MsbBackend for SdkBackend {
    async fn list_sandboxes(&self) -> Result<Vec<SandboxSummary>> {
        let page = microsandbox::Sandbox::list().await?;
        let mut out = Vec::with_capacity(page.sandboxes.len());
        let mut degraded = Vec::new();
        for handle in &page.sandboxes {
            let Some(local) = handle.local() else {
                continue;
            };
            // A sandbox whose stored config can't be parsed still shows up
            // (name, state, created_at; image "?") instead of failing the
            // whole list — smoke-test finding 2.8.
            let parsed = self.sandbox_config_from_json(handle);
            let (image, workdir) = match &parsed {
                Ok(cfg) => (cfg.image.reference(), cfg.runtime.workdir.clone()),
                Err(e) => {
                    degraded.push(format!("{}: {e:#}", handle.name()));
                    ("?".to_string(), None)
                }
            };
            out.push(SandboxSummary {
                created_at: local.created_at.unwrap_or_else(Utc::now),
                image,
                name: handle.name().to_string(),
                status: map_status(local.status),
                workdir,
                mounts: parsed.as_ref().ok().map(map_mounts).unwrap_or_default(),
            });
        }
        if !degraded.is_empty() {
            // Surface the first parse failure so the user knows why a card
            // shows incomplete data.
            return Err(anyhow!(
                "some sandboxes have unreadable configs: {}",
                degraded.join("; ")
            ));
        }
        // SDK lists newest first; the dashboard is not order-sensitive but
        // keep a stable, human-friendly order.
        out.sort_by_key(|s| std::cmp::Reverse(s.created_at));
        Ok(out)
    }

    async fn status(&self, name: &str) -> Result<SandboxStatusRow> {
        let handle = self.handle(name).await?;
        let local = handle
            .local()
            .ok_or_else(|| anyhow!("sandbox '{name}' is not a local sandbox"))?;
        let cfg = self.sandbox_config_from_json(&handle)?;
        Ok(SandboxStatusRow {
            command: cfg
                .runtime
                .cmd
                .iter()
                .map(|c| format!("\"{c}\""))
                .collect::<Vec<_>>()
                .join(" "),
            cpus: cfg.resources.cpus,
            image: cfg.image.reference(),
            max_cpus: cfg.resources.max_cpus,
            max_memory_mib: cfg.resources.max_memory_mib,
            memory_mib: cfg.resources.memory_mib,
            name: handle.name().to_string(),
            ports: cfg
                .network
                .ports
                .iter()
                .map(|p| {
                    let proto = if p.protocol == "tcp" {
                        ""
                    } else {
                        p.protocol.as_str()
                    };
                    format!("{}:{}->{}{}", p.host_bind, p.host_port, p.guest_port, proto)
                })
                .collect(),
            status: map_status(local.status),
            thp: "madvise".to_string(),
        })
    }

    async fn inspect(&self, name: &str) -> Result<SandboxInspect> {
        let handle = self.handle(name).await?;
        self.inspect_from_local_state(&handle).await
    }

    async fn metrics(&self) -> Result<Vec<Metrics>> {
        let reports = microsandbox::all_sandbox_metrics_reports_local(&self.local, true).await?;
        Ok(reports.iter().map(map_report).collect())
    }

    async fn list_volumes(&self) -> Result<Vec<Volume>> {
        let vols = microsandbox::Volume::list().await?;
        let mut out = Vec::with_capacity(vols.len());
        for v in &vols {
            out.push(Volume {
                capacity_bytes: v.capacity_bytes(),
                created_at: v.created_at().unwrap_or_else(Utc::now),
                disk_format: v.disk_format().map(str::to_string),
                disk_fstype: v.disk_fstype().map(str::to_string),
                kind: match v.kind() {
                    microsandbox::VolumeKind::Directory => "dir".to_string(),
                    microsandbox::VolumeKind::Disk => "disk".to_string(),
                },
                name: v.name().to_string(),
                quota_mib: v.quota_mib().map(|q| q as u64),
                used_bytes: v.used_bytes(),
            });
        }
        Ok(out)
    }

    async fn list_images(&self) -> Result<Vec<Image>> {
        let imgs = microsandbox::Image::list().await?;
        let mut out = Vec::with_capacity(imgs.len());
        for i in &imgs {
            out.push(Image {
                architecture: i.architecture().unwrap_or("unknown").to_string(),
                created_at: i.created_at().unwrap_or_else(Utc::now),
                digest: i.manifest_digest().unwrap_or("").to_string(),
                layer_count: i.layer_count() as u32,
                os: i.os().unwrap_or("unknown").to_string(),
                reference: i.reference().to_string(),
                size_bytes: i.size_bytes().unwrap_or(0).max(0) as u64,
            });
        }
        Ok(out)
    }

    async fn start(&self, name: &str) -> Result<()> {
        // Detached: the sandbox must survive TUI exit.
        microsandbox::Sandbox::get(name)
            .await?
            .start_detached()
            .await
            .map(|_| ())
            .with_context(|| format!("start {name}"))
    }

    async fn stop(&self, name: &str) -> Result<()> {
        let handle = microsandbox::Sandbox::get(name).await?;
        if handle
            .local()
            .is_some_and(|l| matches!(l.status, SandboxStatus::Stopped | SandboxStatus::Crashed))
        {
            return Ok(()); // already stopped
        }
        handle
            .stop_with_timeout(std::time::Duration::from_secs(10))
            .await
            .with_context(|| format!("stop {name}"))
    }

    async fn restart(&self, name: &str) -> Result<()> {
        let handle = microsandbox::Sandbox::get(name).await?;
        handle
            .restart_with(microsandbox::sandbox::RestartOptions {
                detached: true, // keep persistence across the restart
                ..Default::default()
            })
            .await
            .map(|_| ())
            .with_context(|| format!("restart {name}"))
    }

    async fn remove(&self, name: &str) -> Result<()> {
        microsandbox::Sandbox::remove(name)
            .await
            .with_context(|| format!("remove {name}"))
    }

    async fn create(&self, spec: &CreateSpec) -> Result<String> {
        let name = spec
            .name
            .clone()
            .unwrap_or_else(|| format!("tui-{}", chrono::Utc::now().timestamp_millis()));
        let builder = apply_spec_to_builder(microsandbox::Sandbox::builder(&name), spec);
        // Detached create: persistent sandbox, VM survives TUI exit.
        builder
            .create_detached()
            .await
            .with_context(|| format!("create {name}"))?;
        Ok(name)
    }

    async fn logs_follow(&self, name: &str) -> Result<LogStream> {
        let handle = microsandbox::Sandbox::get(name).await?;
        let opts = microsandbox::logs::LogOptions {
            tail: Some(1000),
            ..Default::default()
        };
        let stream = handle.follow_logs(&opts).await?;
        // tokio-stream's filter_map takes a sync closure (Option in, Option out).
        let mapped = stream.filter_map(|item| match item {
            Ok(entry) => map_log_line(entry),
            Err(_) => None,
        });
        Ok(Box::pin(mapped) as LogStream)
    }

    async fn exec(&self, name: &str, cmd: &[String]) -> Result<ExecOutput> {
        let handle = microsandbox::Sandbox::get(name).await?;
        let sbx = handle
            .connect()
            .await
            .with_context(|| format!("connect to '{name}' for exec (is it running?)"))?;
        let mut parts = cmd.iter();
        let program = parts.next().ok_or_else(|| anyhow!("exec: empty command"))?;
        let output = sbx
            .exec(program, parts)
            .await
            .with_context(|| format!("exec in {name}"))?;
        Ok(ExecOutput {
            stdout: output.stdout().unwrap_or_default(),
            stderr: output.stderr().unwrap_or_default(),
            exit_code: output.status().code,
        })
    }
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- memory parsing ----

    #[test]
    fn parse_memory_mib_units() {
        assert_eq!(parse_memory_mib("512M"), Some(512));
        assert_eq!(parse_memory_mib("1G"), Some(1024));
        assert_eq!(parse_memory_mib("2g"), Some(2048));
        assert_eq!(parse_memory_mib("1024"), Some(1024));
        assert_eq!(parse_memory_mib("512m"), Some(512));
        assert_eq!(parse_memory_mib("0M"), Some(0));
    }

    #[test]
    fn parse_memory_mib_rejects_bad_input() {
        assert_eq!(parse_memory_mib(""), None);
        assert_eq!(parse_memory_mib("abc"), None);
        assert_eq!(parse_memory_mib("1K"), None, "K is below MiB precision");
        assert_eq!(parse_memory_mib("999999999999G"), None, "overflow rejected");
    }

    // ---- ports ----

    #[test]
    fn sdk_port_roundtrip_tcp_udp() {
        let ours = PublishedPort {
            host_bind: "0.0.0.0".into(),
            host_port: 9090,
            guest_port: 90,
            protocol: "udp".into(),
        };
        let sdk = sdk_port(&ours).unwrap();
        assert_eq!(sdk.host_port, 9090);
        assert_eq!(sdk.guest_port, 90);
        assert!(matches!(sdk.protocol, PortProtocol::Udp));
        assert_eq!(sdk.host_bind, "0.0.0.0".parse::<IpAddr>().unwrap());

        let back = map_port(&sdk);
        assert_eq!(back, ours);
    }

    #[test]
    fn sdk_port_rejects_non_ip_bind() {
        let ours = PublishedPort {
            host_bind: "localhost".into(),
            host_port: 1,
            guest_port: 2,
            protocol: "tcp".into(),
        };
        assert!(sdk_port(&ours).is_err());
    }

    // ---- status mapping ----

    #[test]
    fn map_status_covers_all_variants() {
        use SandboxStatus as S;
        assert_eq!(map_status(S::Created), SandboxState::Created);
        assert_eq!(map_status(S::Starting), SandboxState::Running);
        assert_eq!(map_status(S::Running), SandboxState::Running);
        assert_eq!(map_status(S::Draining), SandboxState::Running);
        assert_eq!(map_status(S::Paused), SandboxState::Paused);
        assert_eq!(map_status(S::Stopped), SandboxState::Stopped);
        assert_eq!(map_status(S::Crashed), SandboxState::Crashed);
    }

    #[test]
    fn map_metrics_state_covers_all_variants() {
        use microsandbox::SandboxMetricsState as M;
        assert_eq!(map_metrics_state(M::Running), SandboxState::Running);
        assert_eq!(map_metrics_state(M::Stalled), SandboxState::Stalled);
        assert_eq!(map_metrics_state(M::Exited), SandboxState::Exited);
    }

    // ---- metrics mapping ----

    #[test]
    fn map_report_carries_name_state_cpus() {
        use std::time::Duration;
        let sdk_metrics = microsandbox::SandboxMetrics {
            cpu_percent: 42.5,
            vcpu_time_ns: 1_000,
            memory_bytes: 1024,
            memory_available_bytes: Some(512),
            memory_host_resident_bytes: Some(2048),
            memory_limit_bytes: 4096,
            disk_read_bytes: 10,
            disk_write_bytes: 20,
            net_rx_bytes: 30,
            net_tx_bytes: 40,
            upper_used_bytes: Some(50),
            upper_free_bytes: Some(60),
            upper_host_allocated_bytes: Some(70),
            uptime: Duration::from_secs(90),
            timestamp: chrono::DateTime::<Utc>::UNIX_EPOCH,
        };
        let report = microsandbox::SandboxMetricsReport {
            name: "web".to_string(),
            sandbox_id: 1,
            run_id: 1,
            state: microsandbox::SandboxMetricsState::Running,
            cpus: Some(2),
            metrics: sdk_metrics,
        };
        let m = map_report(&report);
        assert_eq!(m.name, "web");
        assert_eq!(m.state, SandboxState::Running);
        assert_eq!(m.cpus, 2);
        assert_eq!(m.cpu_percent, 42.5);
        assert_eq!(m.memory_limit_bytes, 4096);
        assert_eq!(m.uptime_secs, 90.0);
        assert_eq!(m.upper_used_bytes, Some(50));
    }

    // ---- volume strings ----

    #[test]
    fn parse_volume_strings() {
        let bind = parse_volume_string("./src:/app").unwrap();
        assert!(matches!(bind.kind, ParsedMountKind::Bind(h) if h == "./src"));
        assert_eq!(bind.guest, "/app");

        let named = parse_volume_string("mydata:/data").unwrap();
        assert!(matches!(named.kind, ParsedMountKind::Named(n) if n == "mydata"));

        let tmpfs = parse_volume_string("tmpfs:/cache").unwrap();
        assert!(matches!(tmpfs.kind, ParsedMountKind::Tmpfs));

        assert!(parse_volume_string("no-colon").is_none());
    }

    // ---- mounts → card display ----

    #[test]
    fn map_mounts_renders_fixture_shapes() {
        let insp: SandboxInspect =
            serde_json::from_str(crate::models::test_fixture("sandbox-inspect")).unwrap();
        let mounts = map_mounts(&insp.active_config);
        // Bind (read-only in the fixture) + Named.
        assert_eq!(
            mounts,
            vec![
                "tui-data:/data".to_string(),
                "/tmp/opencode/probe-src⇢/mnt/src".to_string(),
            ]
        );
    }

    #[test]
    fn map_mounts_empty_for_mountless() {
        let cfg: SandboxConfig =
            serde_json::from_str(crate::models::test_fixture("sandbox-stored-null-workdir"))
                .unwrap();
        assert!(map_mounts(&cfg).is_empty());
    }

    // ---- config mapping (uses the captured inspect fixture's spec shape) ----

    #[test]
    fn map_config_roundtrips_core_fields() {
        // Build a minimal SDK spec, serialize, re-deserialize into ours.
        let spec = SandboxSpec {
            name: "cfg-test".into(),
            image: RootfsSource::Oci(microsandbox_types::OciRootfsSource {
                reference: "alpine".into(),
                root_disk: None,
            }),
            ..Default::default()
        };
        let json = serde_json::to_string(&spec).unwrap();
        let parsed: SandboxSpec = serde_json::from_str(&json).unwrap();
        let cfg = map_config(&parsed);
        assert_eq!(cfg.name, "cfg-test");
        assert_eq!(cfg.image.reference(), "alpine");
    }
}
