//! High-level user-facing operations.
//!
//! These wrap the [`MsbBackend`] trait and add the recreate flow for port
//! changes (microsandbox 0.7.2 binds ports at boot time, so publish/unpublish
//! requires stop → recreate → start).
//!
//! NOTE(dead_code): callers arrive in Task 4 (app). Remove this allow once the
//! event loop wires these up.

#![allow(dead_code)]

use anyhow::{Context, Result};

use crate::backend::fake::spec_from_config;
use crate::backend::{CreateSpec, LogStream, MsbBackend};
use crate::models::PublishedPort;

/// Outcome of [`publish_port`] / [`unpublish_port`]: whether the sandbox was
/// recreated or the operation was a no-op.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishResult {
    /// The port was already in the desired state; no recreation happened.
    AlreadyExists,
    /// The sandbox was stopped, recreated with the new port set, and started.
    Recreated,
}

/// Create a new sandbox. Returns the sandbox name.
pub async fn create_sandbox(backend: &dyn MsbBackend, spec: &CreateSpec) -> Result<String> {
    backend.create(spec).await
}

/// Stop a running sandbox.
pub async fn stop_sandbox(backend: &dyn MsbBackend, name: &str) -> Result<()> {
    backend.stop(name).await
}

/// Start a stopped sandbox.
pub async fn start_sandbox(backend: &dyn MsbBackend, name: &str) -> Result<()> {
    backend.start(name).await
}

/// Restart a sandbox.
pub async fn restart_sandbox(backend: &dyn MsbBackend, name: &str) -> Result<()> {
    backend.restart(name).await
}

/// Remove a sandbox: stop it first (ignoring stop errors if already stopped),
/// then remove.
pub async fn remove_sandbox(backend: &dyn MsbBackend, name: &str) -> Result<()> {
    // Best-effort stop — a sandbox that's already stopped may error; we don't care.
    let _ = backend.stop(name).await;
    backend.remove(name).await
}

/// Publish a port on a sandbox via the recreate flow (msb 0.7.2 binds ports at
/// boot time).
///
/// 1. `inspect` to read current config.
/// 2. If the port already exists (same host_port, guest_port, protocol), return
///    `AlreadyExists` (idempotent).
/// 3. `stop` the sandbox.
/// 4. Build a `CreateSpec` from the current config + the new port set.
/// 5. `create` with `--replace` semantics (the backend's `create` handles
///    replacement).
/// 6. `start` the sandbox.
///
/// Returns `Recreated` on success. The caller should warn the user that the
/// rootfs resets on recreate (volume data persists).
pub async fn publish_port(
    backend: &dyn MsbBackend,
    name: &str,
    port: PublishedPort,
) -> Result<PublishResult> {
    let insp = backend
        .inspect(name)
        .await
        .with_context(|| format!("inspect {name} for publish_port"))?;

    // Idempotent: port already exists?
    let exists = insp.active_config.network.ports.iter().any(|p| {
        p.host_port == port.host_port
            && p.guest_port == port.guest_port
            && p.protocol == port.protocol
            && p.host_bind == port.host_bind
    });
    if exists {
        return Ok(PublishResult::AlreadyExists);
    }

    // Build new port list: existing + new
    let mut ports = insp.active_config.network.ports.clone();
    ports.push(port);

    recreate_with_ports(backend, name, &insp.active_config, ports).await?;
    Ok(PublishResult::Recreated)
}

/// Unpublish a port from a sandbox via the recreate flow.
///
/// `port_spec` is matched by host_port + guest_port + protocol (host_bind if
/// provided). If the port isn't present, returns `AlreadyExists` (idempotent
/// no-op).
pub async fn unpublish_port(
    backend: &dyn MsbBackend,
    name: &str,
    port_spec: &PublishedPort,
) -> Result<PublishResult> {
    let insp = backend
        .inspect(name)
        .await
        .with_context(|| format!("inspect {name} for unpublish_port"))?;

    // Filter out the matching port
    let before = insp.active_config.network.ports.len();
    let ports: Vec<PublishedPort> = insp
        .active_config
        .network
        .ports
        .iter()
        .filter(|p| {
            !(p.host_port == port_spec.host_port
                && p.guest_port == port_spec.guest_port
                && p.protocol == port_spec.protocol
                && p.host_bind == port_spec.host_bind)
        })
        .cloned()
        .collect();

    if ports.len() == before {
        // Port wasn't there — no-op.
        return Ok(PublishResult::AlreadyExists);
    }

    recreate_with_ports(backend, name, &insp.active_config, ports).await?;
    Ok(PublishResult::Recreated)
}

/// Begin streaming logs for a sandbox.
pub async fn stream_logs(backend: &dyn MsbBackend, name: &str) -> Result<LogStream> {
    backend.logs_follow(name).await
}

// ---- internal helpers ----

/// Shared recreate logic: stop → create(replace) → start.
async fn recreate_with_ports(
    backend: &dyn MsbBackend,
    name: &str,
    cfg: &crate::models::SandboxConfig,
    ports: Vec<PublishedPort>,
) -> Result<()> {
    backend
        .stop(name)
        .await
        .with_context(|| format!("stop {name} for recreate"))?;

    let spec = spec_from_config(cfg, ports);
    backend
        .create(&spec)
        .await
        .with_context(|| format!("recreate {name}"))?;

    backend
        .start(name)
        .await
        .with_context(|| format!("start {name} after recreate"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::fake::{Call, FakeBackend};
    use crate::models::SandboxState;

    fn port(host: u16, guest: u16, proto: &str) -> PublishedPort {
        PublishedPort {
            host_bind: "127.0.0.1".into(),
            host_port: host,
            guest_port: guest,
            protocol: proto.into(),
        }
    }

    // ---- create_sandbox ----

    #[tokio::test]
    async fn create_sandbox_calls_backend_create() {
        let b = FakeBackend::new();
        let spec = CreateSpec {
            image: "alpine".into(),
            name: Some("my-sbx".into()),
            ..Default::default()
        };
        let name = create_sandbox(&b, &spec).await.unwrap();
        assert_eq!(name, "my-sbx");
        let calls = b.calls().await;
        assert_eq!(calls.len(), 1);
        assert!(matches!(&calls[0], Call::Create(s) if s.name == Some("my-sbx".into())));
    }

    // ---- remove_sandbox ----

    #[tokio::test]
    async fn remove_sandbox_stops_then_removes() {
        let b = FakeBackend::with_fixture_sandbox();
        remove_sandbox(&b, "tui-fixture").await.unwrap();
        let calls = b.calls().await;
        // Should have stop then remove
        assert!(
            calls
                .iter()
                .any(|c| matches!(c, Call::Stop(n) if n == "tui-fixture"))
        );
        assert!(
            calls
                .iter()
                .any(|c| matches!(c, Call::Remove(n) if n == "tui-fixture"))
        );
        // stop must come before remove
        let stop_idx = calls
            .iter()
            .position(|c| matches!(c, Call::Stop(n) if n == "tui-fixture"))
            .unwrap();
        let rm_idx = calls
            .iter()
            .position(|c| matches!(c, Call::Remove(n) if n == "tui-fixture"))
            .unwrap();
        assert!(stop_idx < rm_idx);
    }

    #[tokio::test]
    async fn remove_sandbox_ignores_stop_errors() {
        let b = FakeBackend::new();
        // No sandbox exists — stop will fail, but remove should also fail with a clear error,
        // not the stop error.
        let err = remove_sandbox(&b, "ghost").await.unwrap_err();
        assert!(err.to_string().contains("ghost"));
    }

    // ---- publish_port: full recreate flow ----

    #[tokio::test]
    async fn publish_port_recreate_sequence() {
        let b = FakeBackend::with_fixture_sandbox();
        // Fixture has ports 8080→80/tcp and 9090→90/udp. Add 7070→70/tcp.
        let new_port = port(7070, 70, "tcp");
        let result = publish_port(&b, "tui-fixture", new_port).await.unwrap();
        assert_eq!(result, PublishResult::Recreated);

        let calls = b.calls().await;
        // Expected: inspect, stop, create, start
        let seq: Vec<&Call> = calls
            .iter()
            .filter(|c| {
                matches!(
                    c,
                    Call::Inspect(_) | Call::Stop(_) | Call::Create(_) | Call::Start(_)
                )
            })
            .collect();
        assert!(seq.len() >= 4, "expected at least 4 calls, got {seq:?}");
        assert!(matches!(seq[0], Call::Inspect(n) if n == "tui-fixture"));
        assert!(matches!(seq[1], Call::Stop(n) if n == "tui-fixture"));
        assert!(matches!(seq[2], Call::Create(_)));
        assert!(matches!(seq[3], Call::Start(n) if n == "tui-fixture"));
    }

    #[tokio::test]
    async fn publish_port_idempotent_when_already_exists() {
        let b = FakeBackend::with_fixture_sandbox();
        // 8080→80/tcp already exists in the fixture.
        let existing = port(8080, 80, "tcp");
        let result = publish_port(&b, "tui-fixture", existing).await.unwrap();
        assert_eq!(result, PublishResult::AlreadyExists);
        let calls = b.calls().await;
        // Only inspect should have been called — no stop/create/start.
        assert_eq!(calls.len(), 1);
        assert!(matches!(&calls[0], Call::Inspect(n) if n == "tui-fixture"));
    }

    #[tokio::test]
    async fn publish_port_preserves_existing_and_adds_new() {
        let b = FakeBackend::with_fixture_sandbox();
        let new_port = port(7070, 70, "tcp");
        publish_port(&b, "tui-fixture", new_port).await.unwrap();

        // After recreate, inspect should show 3 ports: original 2 + new 1.
        let insp = b.inspect("tui-fixture").await.unwrap();
        let ports = &insp.active_config.network.ports;
        assert_eq!(ports.len(), 3);
        assert!(
            ports
                .iter()
                .any(|p| p.host_port == 8080 && p.guest_port == 80)
        );
        assert!(
            ports
                .iter()
                .any(|p| p.host_port == 9090 && p.guest_port == 90)
        );
        assert!(
            ports
                .iter()
                .any(|p| p.host_port == 7070 && p.guest_port == 70)
        );
    }

    #[tokio::test]
    async fn publish_port_preserves_config_fields() {
        let b = FakeBackend::with_fixture_sandbox();
        let new_port = port(7070, 70, "tcp");
        publish_port(&b, "tui-fixture", new_port).await.unwrap();

        let insp = b.inspect("tui-fixture").await.unwrap();
        let cfg = &insp.active_config;
        // Image preserved
        assert_eq!(cfg.image.reference(), "alpine");
        // Resources preserved
        assert_eq!(cfg.resources.cpus, 2);
        assert_eq!(cfg.resources.memory_mib, 768);
        // Env preserved
        assert!(
            cfg.env
                .iter()
                .any(|e| e.key == "APP_ENV" && e.value == "dev")
        );
        // Mounts preserved
        assert_eq!(cfg.mounts.len(), 2);
    }

    // ---- unpublish_port ----

    #[tokio::test]
    async fn unpublish_port_removes_correct_port() {
        let b = FakeBackend::with_fixture_sandbox();
        // Remove 8080→80/tcp (the first port in the fixture).
        let to_remove = port(8080, 80, "tcp");
        let result = unpublish_port(&b, "tui-fixture", &to_remove).await.unwrap();
        assert_eq!(result, PublishResult::Recreated);

        let insp = b.inspect("tui-fixture").await.unwrap();
        let ports = &insp.active_config.network.ports;
        assert_eq!(ports.len(), 1);
        // The remaining port should be 9090→90/udp.
        assert!(ports.iter().all(|p| p.host_port == 9090));
    }

    #[tokio::test]
    async fn unpublish_port_idempotent_when_not_found() {
        let b = FakeBackend::with_fixture_sandbox();
        let ghost = port(1234, 56, "tcp");
        let result = unpublish_port(&b, "tui-fixture", &ghost).await.unwrap();
        assert_eq!(result, PublishResult::AlreadyExists);
        let calls = b.calls().await;
        // Only inspect.
        assert_eq!(calls.len(), 1);
        assert!(matches!(&calls[0], Call::Inspect(n) if n == "tui-fixture"));
    }

    // ---- stream_logs ----

    #[tokio::test]
    async fn stream_logs_calls_backend() {
        let b = FakeBackend::with_fixture_sandbox();
        let _stream = stream_logs(&b, "tui-fixture").await.unwrap();
        let calls = b.calls().await;
        assert!(
            calls
                .iter()
                .any(|c| matches!(c, Call::LogsFollow(n) if n == "tui-fixture"))
        );
    }

    // ---- start/stop/restart ----

    #[tokio::test]
    async fn stop_sandbox_calls_backend_stop() {
        let b = FakeBackend::with_fixture_sandbox();
        stop_sandbox(&b, "tui-fixture").await.unwrap();
        let calls = b.calls().await;
        assert!(
            calls
                .iter()
                .any(|c| matches!(c, Call::Stop(n) if n == "tui-fixture"))
        );
        let insp = b.inspect("tui-fixture").await.unwrap();
        assert_eq!(insp.status, SandboxState::Stopped);
    }

    #[tokio::test]
    async fn start_sandbox_calls_backend_start() {
        let b = FakeBackend::with_fixture_sandbox();
        b.stop("tui-fixture").await.unwrap();
        start_sandbox(&b, "tui-fixture").await.unwrap();
        let insp = b.inspect("tui-fixture").await.unwrap();
        assert_eq!(insp.status, SandboxState::Running);
    }

    #[tokio::test]
    async fn restart_sandbox_calls_backend_restart() {
        let b = FakeBackend::with_fixture_sandbox();
        restart_sandbox(&b, "tui-fixture").await.unwrap();
        let calls = b.calls().await;
        assert!(
            calls
                .iter()
                .any(|c| matches!(c, Call::Restart(n) if n == "tui-fixture"))
        );
    }
}
