//! In-memory `MsbBackend` for unit tests.
//!
//! Stores sandboxes, volumes, and images in `HashMap`s and records every
//! method call so action-layer tests can assert call sequences (e.g. the
//! recreate flow: inspect → stop → create → start).

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::Mutex;

use crate::models::{
    EnvVar, Image, ImageSpec, Lifecycle, Metrics, Mount, NetworkConfig, OciImageSpec, PauseInfo,
    PublishedPort, Resources, RootDiskSpec, RuntimeConfig, SandboxConfig, SandboxInspect,
    SandboxState, SandboxStatusRow, SandboxSummary, Volume,
};

use super::{CreateSpec, ExecOutput, LogStream, MsbBackend};

/// One fake sandbox entry.
#[derive(Debug, Clone)]
pub struct FakeSandbox {
    pub name: String,
    pub state: SandboxState,
    pub config: SandboxConfig,
    pub metrics: Metrics,
}

/// Record of a single backend method call, in invocation order.
#[derive(Debug, Clone, PartialEq)]
pub enum Call {
    ListSandboxes,
    Status(String),
    Inspect(String),
    Metrics,
    ListVolumes,
    ListImages,
    Start(String),
    Stop(String),
    Restart(String),
    Remove(String),
    Create(Box<CreateSpec>),
    LogsFollow(String),
    Exec {
        name: String,
        cmd: Vec<String>,
    },
    /// Disk snapshot of a sandbox; the string is the returned reference.
    SnapshotDisk(String),
    /// Restore a snapshot into a sandbox with the given spec + ports.
    Restore {
        name: String,
        spec: Box<CreateSpec>,
        ports: Vec<PublishedPort>,
    },
    /// Best-effort snapshot cleanup by reference.
    RemoveSnapshot(String),
    /// Reapply config (env/workdir/labels) after a restore, persisted for
    /// the next start (no restart).
    Modify {
        name: String,
        spec: Box<CreateSpec>,
    },
}

/// In-memory backend for unit tests.
///
/// All state is behind a `Mutex` so the same instance can be shared across
/// async tasks (the event loop will do this in Task 4).
#[derive(Debug, Clone, Default)]
pub struct FakeBackend {
    inner: Arc<Mutex<FakeBackendInner>>,
}

#[derive(Debug, Default)]
struct FakeBackendInner {
    sandboxes: HashMap<String, FakeSandbox>,
    volumes: Vec<Volume>,
    images: Vec<Image>,
    calls: Vec<Call>,
    /// Names returned by `create` when `spec.name` is `None`.
    auto_name_counter: u64,
    /// Snapshots by reference: the captured sandbox state (a clone taken at
    /// snapshot time — like a real snapshot, it survives the source's removal).
    snapshots: HashMap<String, FakeSandbox>,
    /// When set, the next `restore_with_ports` fails (error injection).
    fail_next_restore: bool,
}

impl FakeBackend {
    /// Build a `FakeBackend` seeded with one running sandbox from the
    /// `sandbox-inspect` fixture, plus the volumes/images fixtures.
    pub fn with_fixture_sandbox() -> Self {
        let insp: SandboxInspect =
            serde_json::from_str(include_str!("../fixtures/sandbox-inspect.json"))
                .expect("fixture parses");
        let metrics: Vec<Metrics> = serde_json::from_str(include_str!("../fixtures/metrics.json"))
            .expect("metrics fixture parses");
        let volumes: Vec<Volume> = serde_json::from_str(include_str!("../fixtures/volumes.json"))
            .expect("volumes fixture parses");
        let images: Vec<Image> = serde_json::from_str(include_str!("../fixtures/images.json"))
            .expect("images fixture parses");

        let name = insp.name.clone();
        let mut metrics = metrics.into_iter().next().unwrap_or_else(|| Metrics {
            cpu_percent: 0.0,
            cpus: insp.active_config.resources.cpus,
            disk_read_bytes: 0,
            disk_write_bytes: 0,
            memory_available_bytes: 0,
            memory_bytes: 0,
            memory_host_resident_bytes: 0,
            memory_limit_bytes: 0,
            name: name.clone(),
            net_rx_bytes: 0,
            net_tx_bytes: 0,
            state: SandboxState::Running,
            timestamp: Utc::now(),
            upper_free_bytes: None,
            upper_host_allocated_bytes: None,
            upper_used_bytes: None,
            uptime_secs: 0.0,
            vcpu_time_ns: 0,
        });
        metrics.name = name.clone();
        metrics.state = SandboxState::Running;

        let sbx = FakeSandbox {
            name: name.clone(),
            state: SandboxState::Running,
            config: insp.active_config.clone(),
            metrics,
        };

        let mut sandboxes = HashMap::new();
        sandboxes.insert(name, sbx);

        Self {
            inner: Arc::new(Mutex::new(FakeBackendInner {
                sandboxes,
                volumes,
                images,
                calls: Vec::new(),
                auto_name_counter: 0,
                snapshots: HashMap::new(),
                fail_next_restore: false,
            })),
        }
    }

    /// Build an empty `FakeBackend`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot of recorded calls, in invocation order.
    pub async fn calls(&self) -> Vec<Call> {
        self.inner.lock().await.calls.clone()
    }

    /// Make the next `restore_with_ports` fail (error injection).
    pub async fn fail_next_restore(&self) {
        self.inner.lock().await.fail_next_restore = true;
    }

    /// Insert a sandbox directly (bypassing `create`).
    pub async fn insert_sandbox(&self, sbx: FakeSandbox) {
        self.inner
            .lock()
            .await
            .sandboxes
            .insert(sbx.name.clone(), sbx);
    }

    /// Helper: build a minimal `FakeSandbox` with the given name, image, and
    /// ports. Useful for ad-hoc test setup.
    pub fn make_sandbox(name: &str, image: &str, ports: Vec<PublishedPort>) -> FakeSandbox {
        let config = SandboxConfig {
            deployment_profile: "single_tenant".into(),
            env: vec![EnvVar {
                key: "PATH".into(),
                value: "/usr/bin".into(),
            }],
            external_mount_policy: "strict".into(),
            image: ImageSpec {
                oci: Some(OciImageSpec {
                    reference: image.into(),
                    root_disk: RootDiskSpec {
                        kind: "managed".into(),
                        size_mib: Some(4096),
                        extra: HashMap::new(),
                    },
                    extra: HashMap::new(),
                }),
                other: HashMap::new(),
            },
            init: None,
            labels: HashMap::new(),
            lifecycle: Lifecycle {
                ephemeral: false,
                idle_timeout_secs: None,
                max_duration_secs: None,
            },
            manifest_digest: "sha256:fake".into(),
            mounts: Vec::new(),
            name: name.into(),
            network: NetworkConfig {
                enabled: true,
                max_connections: None,
                ports,
                strict: false,
                trust_host_cas: false,
                extra: HashMap::new(),
            },
            patches: Vec::new(),
            pull_policy: "IfMissing".into(),
            resources: Resources {
                cpus: 1,
                max_cpus: 1,
                max_memory_mib: 512,
                memory_mib: 512,
            },
            rlimits: Vec::new(),
            runtime: RuntimeConfig {
                cmd: vec!["/bin/sh".into()],
                disable_metrics_sample: false,
                entrypoint: None,
                hostname: None,
                log_level: None,
                metrics_sample_interval_ms: 1000,
                scripts: HashMap::new(),
                shell: "/bin/sh".into(),
                user: None,
                workdir: Some("/".into()),
            },
            security_profile: "default".into(),
        };
        let metrics = Metrics {
            cpu_percent: 0.0,
            cpus: 1,
            disk_read_bytes: 0,
            disk_write_bytes: 0,
            memory_available_bytes: 0,
            memory_bytes: 0,
            memory_host_resident_bytes: 0,
            memory_limit_bytes: 512 * 1024 * 1024,
            name: name.into(),
            net_rx_bytes: 0,
            net_tx_bytes: 0,
            state: SandboxState::Running,
            timestamp: Utc::now(),
            upper_free_bytes: None,
            upper_host_allocated_bytes: None,
            upper_used_bytes: None,
            uptime_secs: 0.0,
            vcpu_time_ns: 0,
        };
        FakeSandbox {
            name: name.into(),
            state: SandboxState::Running,
            config,
            metrics,
        }
    }
}

/// Build a `SandboxInspect` from a `FakeSandbox` (config is cloned for both
/// `config` and `active_config`).
fn inspect_from(sbx: &FakeSandbox) -> SandboxInspect {
    SandboxInspect {
        active_config: sbx.config.clone(),
        config: sbx.config.clone(),
        created_at: Utc::now(),
        name: sbx.name.clone(),
        pause: PauseInfo {
            capture_unavailable: None,
            paused: matches!(sbx.state, SandboxState::Paused),
            recovery_required: false,
        },
        pending_changes: Vec::new(),
        status: sbx.state.clone(),
        updated_at: Utc::now(),
    }
}

/// Convert a `SandboxConfig` + current state into a `SandboxSummary`.
fn summary_from(sbx: &FakeSandbox) -> SandboxSummary {
    SandboxSummary {
        created_at: Utc::now(),
        image: sbx.config.image.reference(),
        name: sbx.name.clone(),
        status: sbx.state.clone(),
        workdir: sbx.config.runtime.workdir.clone(),
        mounts: crate::backend::sdk::map_mounts(&sbx.config),
    }
}

/// Convert a `SandboxConfig` + current state into a `SandboxStatusRow`.
fn status_row_from(sbx: &FakeSandbox) -> SandboxStatusRow {
    let ports: Vec<String> = sbx
        .config
        .network
        .ports
        .iter()
        .map(|p| {
            // Reverse of parse_ps: BIND:HOST->GUEST/proto or HOST->GUEST/proto
            let proto = if p.protocol == "tcp" {
                String::new()
            } else {
                format!("/{}", p.protocol)
            };
            match p.host_bind.as_str() {
                "" | "127.0.0.1" => format!("{}->{}{}", p.host_port, p.guest_port, proto),
                bind => format!("{}:{}->{}{}", bind, p.host_port, p.guest_port, proto),
            }
        })
        .collect();
    SandboxStatusRow {
        command: sbx
            .config
            .runtime
            .cmd
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(" "),
        cpus: sbx.config.resources.cpus,
        image: sbx.config.image.reference(),
        max_cpus: sbx.config.resources.max_cpus,
        max_memory_mib: sbx.config.resources.max_memory_mib,
        memory_mib: sbx.config.resources.memory_mib,
        name: sbx.name.clone(),
        ports,
        status: sbx.state.clone(),
        thp: "madvise".into(),
    }
}

/// Build a `CreateSpec` from an existing `SandboxConfig` — used by the recreate
/// flow to preserve all settings when adding/removing a port.
pub fn spec_from_config(cfg: &SandboxConfig, ports: Vec<PublishedPort>) -> CreateSpec {
    CreateSpec {
        image: cfg.image.reference(),
        name: Some(cfg.name.clone()),
        cpus: Some(cfg.resources.cpus),
        memory: Some(format!("{}M", cfg.resources.memory_mib)),
        workdir: cfg.runtime.workdir.clone().filter(|w| !w.is_empty()),
        ports,
        volumes: cfg
            .mounts
            .iter()
            .map(|m| match m.source() {
                "" => m.guest.clone(),
                src => format!("{src}:{}", m.guest),
            })
            .collect(),
        env: cfg
            .env
            .iter()
            .map(|e| format!("{}={}", e.key, e.value))
            .collect(),
        labels: cfg.labels.iter().map(|(k, v)| format!("{k}={v}")).collect(),
        net_profile: if cfg.network.enabled {
            Some("public".into())
        } else {
            None
        },
        net_rules: Vec::new(),
    }
}

#[async_trait]
impl MsbBackend for FakeBackend {
    async fn list_sandboxes(&self) -> Result<(Vec<SandboxSummary>, Vec<String>)> {
        let mut g = self.inner.lock().await;
        g.calls.push(Call::ListSandboxes);
        let mut rows: Vec<SandboxSummary> = g.sandboxes.values().map(summary_from).collect();
        rows.sort_by(|a, b| a.name.cmp(&b.name));
        Ok((rows, Vec::new()))
    }

    async fn status(&self, name: &str) -> Result<SandboxStatusRow> {
        let mut g = self.inner.lock().await;
        g.calls.push(Call::Status(name.to_string()));
        let sbx = g
            .sandboxes
            .get(name)
            .ok_or_else(|| anyhow!("FakeBackend: no sandbox named {name}"))?;
        Ok(status_row_from(sbx))
    }

    async fn inspect(&self, name: &str) -> Result<SandboxInspect> {
        let mut g = self.inner.lock().await;
        g.calls.push(Call::Inspect(name.to_string()));
        let sbx = g
            .sandboxes
            .get(name)
            .ok_or_else(|| anyhow!("FakeBackend: no sandbox named {name}"))?;
        Ok(inspect_from(sbx))
    }

    async fn metrics(&self) -> Result<Vec<Metrics>> {
        let mut g = self.inner.lock().await;
        g.calls.push(Call::Metrics);
        let mut rows: Vec<Metrics> = g
            .sandboxes
            .values()
            .map(|s| {
                let mut m = s.metrics.clone();
                m.state = s.state.clone();
                m
            })
            .collect();
        rows.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(rows)
    }

    async fn list_volumes(&self) -> Result<Vec<Volume>> {
        let mut g = self.inner.lock().await;
        g.calls.push(Call::ListVolumes);
        Ok(g.volumes.clone())
    }

    async fn list_images(&self) -> Result<Vec<Image>> {
        let mut g = self.inner.lock().await;
        g.calls.push(Call::ListImages);
        Ok(g.images.clone())
    }

    async fn start(&self, name: &str) -> Result<()> {
        let mut g = self.inner.lock().await;
        g.calls.push(Call::Start(name.to_string()));
        let sbx = g
            .sandboxes
            .get_mut(name)
            .ok_or_else(|| anyhow!("FakeBackend: no sandbox named {name}"))?;
        // SDK invariant (0.7.2): starting a Running sandbox fails with
        // SandboxStillRunning ("cannot start sandbox: already running").
        if sbx.state.is_running() {
            return Err(anyhow!("cannot start sandbox '{name}': already running"));
        }
        sbx.state = SandboxState::Running;
        Ok(())
    }

    async fn stop(&self, name: &str) -> Result<()> {
        let mut g = self.inner.lock().await;
        g.calls.push(Call::Stop(name.to_string()));
        let sbx = g
            .sandboxes
            .get_mut(name)
            .ok_or_else(|| anyhow!("FakeBackend: no sandbox named {name}"))?;
        sbx.state = SandboxState::Stopped;
        Ok(())
    }

    async fn restart(&self, name: &str) -> Result<()> {
        let mut g = self.inner.lock().await;
        g.calls.push(Call::Restart(name.to_string()));
        let sbx = g
            .sandboxes
            .get_mut(name)
            .ok_or_else(|| anyhow!("FakeBackend: no sandbox named {name}"))?;
        sbx.state = SandboxState::Running;
        Ok(())
    }

    async fn remove(&self, name: &str) -> Result<()> {
        let mut g = self.inner.lock().await;
        g.calls.push(Call::Remove(name.to_string()));
        g.sandboxes
            .remove(name)
            .ok_or_else(|| anyhow!("FakeBackend: no sandbox named {name}"))?;
        Ok(())
    }

    async fn snapshot_disk(&self, name: &str) -> Result<String> {
        let mut g = self.inner.lock().await;
        g.calls.push(Call::SnapshotDisk(name.to_string()));
        let sbx = g
            .sandboxes
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow!("FakeBackend: no sandbox named {name}"))?;
        // Like a real snapshot: clone the captured state; the copy survives
        // the source's removal.
        let reference = format!("snap-{name}");
        g.snapshots.insert(reference.clone(), sbx);
        Ok(reference)
    }

    async fn restore_with_ports(
        &self,
        snapshot_ref: &str,
        spec: &CreateSpec,
        ports: Vec<PublishedPort>,
    ) -> Result<()> {
        let mut g = self.inner.lock().await;
        g.calls.push(Call::Restore {
            name: spec.name.clone().unwrap_or_else(|| "restored".to_string()),
            spec: Box::new(spec.clone()),
            ports: ports.clone(),
        });
        if g.fail_next_restore {
            g.fail_next_restore = false;
            return Err(anyhow!("FakeBackend: injected restore failure"));
        }
        // The restore cold-boots the captured disk content: the sandbox
        // comes back with the snapshot's config, the NEW port set, running.
        let mut sbx = g
            .snapshots
            .get(snapshot_ref)
            .cloned()
            .ok_or_else(|| anyhow!("FakeBackend: no snapshot {snapshot_ref}"))?;
        sbx.name = spec.name.clone().unwrap_or(sbx.name);
        sbx.state = SandboxState::Running;
        sbx.config.network.ports = ports;
        g.sandboxes.insert(sbx.name.clone(), sbx);
        Ok(())
    }

    async fn remove_snapshot(&self, snapshot_ref: &str) -> Result<()> {
        let mut g = self.inner.lock().await;
        g.calls.push(Call::RemoveSnapshot(snapshot_ref.to_string()));
        g.snapshots
            .remove(snapshot_ref)
            .ok_or_else(|| anyhow!("FakeBackend: no snapshot {snapshot_ref}"))?;
        Ok(())
    }

    async fn reapply_config(&self, name: &str, spec: &CreateSpec) -> Result<()> {
        let mut g = self.inner.lock().await;
        g.calls.push(Call::Modify {
            name: name.to_string(),
            spec: Box::new(spec.clone()),
        });
        let sbx = g
            .sandboxes
            .get_mut(name)
            .ok_or_else(|| anyhow!("FakeBackend: no sandbox named {name}"))?;
        // Persist env/workdir/labels like the modification API would.
        sbx.config.env = spec
            .env
            .iter()
            .filter_map(|e| {
                e.split_once('=').map(|(k, v)| EnvVar {
                    key: k.into(),
                    value: v.into(),
                })
            })
            .collect();
        sbx.config.runtime.workdir = spec.workdir.clone().filter(|w| !w.is_empty());
        Ok(())
    }

    async fn create(&self, spec: &CreateSpec) -> Result<String> {
        let mut g = self.inner.lock().await;
        g.calls.push(Call::Create(Box::new(spec.clone())));
        let name = spec.name.clone().unwrap_or_else(|| {
            g.auto_name_counter += 1;
            format!("auto-{}", g.auto_name_counter)
        });
        let mut sbx = Self::make_sandbox(&name, &spec.image, spec.ports.clone());

        // Reconstruct mounts from volume strings ("src:dest" or "name:dest").
        sbx.config.mounts = spec
            .volumes
            .iter()
            .map(|v| {
                let (source, guest) = match v.split_once(':') {
                    Some((s, g)) => (s.to_string(), g.to_string()),
                    None => (v.clone(), v.clone()),
                };
                // Heuristic: if source contains '/' it's a bind mount, else named.
                let is_bind = source.contains('/') || source == ".";
                let m = Mount {
                    kind: if is_bind { "Bind" } else { "Named" }.into(),
                    guest,
                    name: if is_bind { None } else { Some(source.clone()) },
                    host: if is_bind { Some(source.clone()) } else { None },
                    options: Default::default(),
                    quota_mib: None,
                    follow_root_symlinks: None,
                    host_permissions: None,
                    stat_virtualization: None,
                    extra: HashMap::new(),
                };
                // Use source to avoid unused warning
                let _ = &source;
                m
            })
            .collect();

        // Reconstruct env from "KEY=VALUE" strings.
        sbx.config.env = spec
            .env
            .iter()
            .filter_map(|e| {
                e.split_once('=').map(|(k, v)| EnvVar {
                    key: k.to_string(),
                    value: v.to_string(),
                })
            })
            .collect();

        // Reconstruct labels from "KEY=VALUE" strings.
        sbx.config.labels = spec
            .labels
            .iter()
            .filter_map(|l| {
                l.split_once('=')
                    .map(|(k, v)| (k.to_string(), v.to_string()))
            })
            .collect();

        // Apply resource spec if provided.
        if let Some(cpus) = spec.cpus {
            sbx.config.resources.cpus = cpus;
            sbx.config.resources.max_cpus = cpus;
        }
        if let Some(mem) = &spec.memory {
            // Parse "512M" or "1G" into mib.
            let mib = if let Some(num) = mem.strip_suffix('M') {
                num.parse::<u64>().unwrap_or(512)
            } else if let Some(num) = mem.strip_suffix('G') {
                num.parse::<u64>().unwrap_or(1) * 1024
            } else {
                mem.parse::<u64>().unwrap_or(512)
            };
            sbx.config.resources.memory_mib = mib;
            sbx.config.resources.max_memory_mib = mib;
        }

        // Apply workdir.
        if let Some(wd) = &spec.workdir {
            sbx.config.runtime.workdir = Some(wd.clone());
        }

        sbx.metrics.name = name.clone();
        g.sandboxes.insert(name.clone(), sbx);
        Ok(name)
    }

    async fn logs_follow(&self, name: &str) -> Result<LogStream> {
        let mut g = self.inner.lock().await;
        g.calls.push(Call::LogsFollow(name.to_string()));
        if !g.sandboxes.contains_key(name) {
            return Err(anyhow!("FakeBackend: no sandbox named {name}"));
        }
        // Empty stream — tests don't consume log lines, they just assert the call.
        let stream = tokio_stream::empty();
        Ok(Box::pin(stream))
    }

    async fn exec(&self, name: &str, cmd: &[String]) -> Result<ExecOutput> {
        let mut g = self.inner.lock().await;
        g.calls.push(Call::Exec {
            name: name.to_string(),
            cmd: cmd.to_vec(),
        });
        let sbx = g
            .sandboxes
            .get(name)
            .ok_or_else(|| anyhow!("FakeBackend: no sandbox named {name}"))?;
        let _ = sbx;
        Ok(ExecOutput {
            stdout: format!("[fake exec] {cmd:?} in {name}\n"),
            stderr: String::new(),
            exit_code: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_backend_lifecycle() {
        let b = FakeBackend::with_fixture_sandbox();
        // inspect
        let insp = b.inspect("tui-fixture").await.unwrap();
        assert_eq!(insp.name, "tui-fixture");
        // stop
        b.stop("tui-fixture").await.unwrap();
        let insp = b.inspect("tui-fixture").await.unwrap();
        assert_eq!(insp.status, SandboxState::Stopped);
        // start
        b.start("tui-fixture").await.unwrap();
        let insp = b.inspect("tui-fixture").await.unwrap();
        assert_eq!(insp.status, SandboxState::Running);
    }

    #[tokio::test]
    async fn fake_backend_create_records_spec() {
        let b = FakeBackend::new();
        let spec = CreateSpec {
            image: "alpine".into(),
            name: Some("test-sbx".into()),
            cpus: Some(2),
            ..Default::default()
        };
        let name = b.create(&spec).await.unwrap();
        assert_eq!(name, "test-sbx");
        let calls = b.calls().await;
        assert_eq!(calls.len(), 1);
        assert!(matches!(&calls[0], Call::Create(s) if s.name == Some("test-sbx".into())));
    }

    #[tokio::test]
    async fn fake_backend_create_applies_workdir_and_mounts() {
        let b = FakeBackend::new();
        // sbx-style spec: same-path workspace bind + workdir.
        let spec = CreateSpec {
            image: "alpine".into(),
            workdir: Some("/home/me/proj".into()),
            volumes: vec!["/home/me/proj:/home/me/proj".into(), "data:/data".into()],
            ..Default::default()
        };
        let name = b.create(&spec).await.unwrap();
        let insp = b.inspect(&name).await.unwrap();
        // Workdir applied verbatim.
        assert_eq!(
            insp.active_config.runtime.workdir.as_deref(),
            Some("/home/me/proj")
        );
        // Both mounts reconstructed: the workspace bind + the named volume.
        let mounts = &insp.active_config.mounts;
        assert_eq!(mounts.len(), 2);
        let bind = mounts.iter().find(|m| m.kind == "Bind").unwrap();
        assert_eq!(bind.host.as_deref(), Some("/home/me/proj"));
        assert_eq!(bind.guest, "/home/me/proj");
        let named = mounts.iter().find(|m| m.kind == "Named").unwrap();
        assert_eq!(named.name.as_deref(), Some("data"));
        assert_eq!(named.guest, "/data");
        // The card display shows the mounts (map_mounts via summary).
        let (rows, warnings) = b.list_sandboxes().await.unwrap();
        assert!(warnings.is_empty());
        let row = rows.iter().find(|r| r.name == name).unwrap();
        assert_eq!(
            row.mounts,
            vec!["/home/me/proj⇄/home/me/proj", "data:/data"]
        );
    }

    #[tokio::test]
    async fn fake_backend_remove() {
        let b = FakeBackend::with_fixture_sandbox();
        b.remove("tui-fixture").await.unwrap();
        assert!(b.inspect("tui-fixture").await.is_err());
    }

    #[tokio::test]
    async fn spec_from_config_preserves_ports() {
        let b = FakeBackend::with_fixture_sandbox();
        let insp = b.inspect("tui-fixture").await.unwrap();
        let ports = insp.active_config.network.ports.clone();
        let spec = spec_from_config(&insp.active_config, ports);
        assert_eq!(spec.name, Some("tui-fixture".into()));
        assert_eq!(spec.image, "alpine");
        assert_eq!(spec.ports.len(), 2);
        assert_eq!(spec.ports[0].host_port, 8080);
    }

    #[tokio::test]
    async fn exec_records_call_and_returns_output() {
        let b = FakeBackend::with_fixture_sandbox();
        let out = b
            .exec("tui-fixture", &["echo".to_string(), "hi".to_string()])
            .await
            .unwrap();
        assert_eq!(out.exit_code, 0);
        assert!(calls_contains_exec(&b, "tui-fixture", &["echo", "hi"]).await);

        // Unknown sandbox errors.
        assert!(b.exec("ghost", &["true".to_string()]).await.is_err());
    }

    async fn calls_contains_exec(b: &FakeBackend, name: &str, cmd: &[&str]) -> bool {
        b.calls().await.iter().any(|c| {
            matches!(c, Call::Exec { name: n, cmd: v }
                if n == name && v.iter().map(String::as_str).collect::<Vec<_>>() == cmd)
        })
    }
}
