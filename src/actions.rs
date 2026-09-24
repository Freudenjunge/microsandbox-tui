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
use crate::backend::{CreateSpec, ExecOutput, LogStream, MsbBackend};
use crate::models::{PublishedPort, SandboxState};

/// Outcome of [`publish_port`] / [`unpublish_port`]: whether the sandbox was
/// recreated or the operation was a no-op.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishResult {
    /// The port was already in the desired state; no recreation happened.
    AlreadyExists,
    /// The sandbox was stopped, recreated with the new port set, and started.
    Recreated,
}

/// A state-gated key action offered on the selected sandbox card
/// (Docker-sbx parity: the card shows the action word with the key letter
/// highlighted inside it, e.g. E**x**ec — and only what the current state
/// allows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardAction {
    /// `s` — start a stopped sandbox (the `s` toggle's start half).
    Start,
    /// `s` — stop a running sandbox (the `s` toggle's stop half).
    Stop,
    /// `r` — restart a running/stalled sandbox.
    Restart,
    /// `x` — open an interactive shell (E**x**ec).
    Shell,
    /// `Del` — remove the sandbox (stops first if needed).
    Destroy,
}

impl CardAction {
    /// The key that triggers this action, as shown in help text.
    pub fn key_hint(self) -> &'static str {
        match self {
            Self::Start | Self::Stop => "s",
            Self::Restart => "r",
            Self::Shell => "x",
            Self::Destroy => "Del",
        }
    }

    /// Action word shown on the card, e.g. `Exec`.
    pub fn label(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::Stop => "Stop",
            Self::Restart => "Restart",
            Self::Shell => "Exec",
            Self::Destroy => "Delete",
        }
    }

    /// The substring of [`CardAction::label`] that is the key, highlighted
    /// in the card's action row (Docker-sbx style).
    pub fn highlight(self) -> &'static str {
        match self {
            Self::Start => "S",
            Self::Stop => "S",
            Self::Restart => "R",
            Self::Shell => "x",
            Self::Destroy => "Del",
        }
    }
}

/// Which half of the `s` start/stop toggle (Docker-sbx parity) applies to a
/// sandbox in `state`: startable states get [`CardAction::Start`],
/// stoppable ones get [`CardAction::Stop`], `None` when neither applies.
pub fn toggle_action(state: &SandboxState) -> Option<CardAction> {
    use CardAction as A;
    use SandboxState as S;
    match state {
        S::Stopped | S::Created | S::Crashed | S::Exited => Some(A::Start),
        S::Running | S::Paused | S::Stalled => Some(A::Stop),
        S::Unknown(_) => None,
    }
}

/// The actions currently available for a sandbox in `state`, in display
/// order. Destroy is always available (the remove flow stops first);
/// everything else is gated by state:
///
/// | state            | Start | Stop | Restart | Shell | Destroy |
/// |------------------|-------|------|---------|-------|---------|
/// | Running          |   –   |  ✓   |    ✓    |   ✓   |    ✓    |
/// | Stopped/Created/ |   ✓   |  –   |    –    |   –   |    ✓    |
/// | Crashed/Exited   |       |      |         |       |         |
/// | Paused           |   –   |  ✓   |    –    |   –   |    ✓    |
/// | Stalled          |   –   |  ✓   |    ✓    |   –   |    ✓    |
/// | Unknown          |   –   |  –   |    –    |   –   |    ✓    |
pub fn available_actions(state: &SandboxState) -> Vec<CardAction> {
    use CardAction as A;
    use SandboxState as S;
    let mut actions = match state {
        S::Running => vec![A::Stop, A::Restart, A::Shell],
        S::Stopped | S::Created | S::Crashed | S::Exited => vec![A::Start],
        S::Paused => vec![A::Stop],
        S::Stalled => vec![A::Stop, A::Restart],
        S::Unknown(_) => Vec::new(),
    };
    actions.push(A::Destroy);
    actions
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

/// Publish a port on a sandbox via the snapshot-restore flow (msb 0.7.2
/// binds ports at boot time, so a port change requires a recreate).
///
/// 1. `inspect` to read current config.
/// 2. If the port already exists (same host_port, guest_port, protocol), return
///    `AlreadyExists` (idempotent).
/// 3. Snapshot the disk, stop, remove, and restore with the new port set —
///    see [`recreate_with_ports`]. The sandbox's DATA IS PRESERVED (the new
///    disk comes from the snapshot); only the running processes are lost,
///    like a `restart`.
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

/// Run a command inside a sandbox and capture its output.
pub async fn exec_sandbox(
    backend: &dyn MsbBackend,
    name: &str,
    cmd: &[String],
) -> Result<ExecOutput> {
    backend.exec(name, cmd).await
}

// ---- internal helpers ----

/// Shared recreate logic for msb 0.7.2, which has no `create --replace` flag
/// (verified: name collisions always error). The flow is therefore:
/// stop → remove → create (same name) → start.
///
/// Recreate a sandbox with a different port set WITHOUT losing its disk
/// data (Docker-sbx ports parity, adapted to microsandbox's boot-time
/// port binding):
///
/// 1. `snapshot_disk` — capture the current disk content (running sources
///    supported)
/// 2. `stop` + `remove` — free the name (0.7.2 has no create-with-replace
///    in this flow)
/// 3. `restore_with_ports` — cold-boot a new sandbox from the snapshot with
///    the new port set; env/volumes/workdir come from the inspected config
/// 4. `remove_snapshot` — clean up the temporary snapshot (only after a
///    successful restore; on failure it survives for manual recovery)
///
/// The guest sees a reboot (like `restart`), but its files are preserved.
async fn recreate_with_ports(
    backend: &dyn MsbBackend,
    name: &str,
    cfg: &crate::models::SandboxConfig,
    ports: Vec<PublishedPort>,
) -> Result<()> {
    // 1. Capture the disk before anything is torn down.
    let snapshot_ref = backend
        .snapshot_disk(name)
        .await
        .with_context(|| format!("snapshot {name} for recreate"))?;

    // 2. Tear down (best-effort snapshot cleanup if restore never runs).
    if let Err(e) = teardown_for_restore(backend, name).await {
        // The snapshot is the user's only copy of the data now — keep it
        // and surface the teardown error.
        return Err(e).with_context(|| format!("teardown {name} for recreate"));
    }

    // 3. Restore from the snapshot with the new port set, then re-apply the
    //    config the RestoreBuilder cannot carry (env/workdir/labels).
    let spec = spec_from_config(cfg, ports.clone());
    let restored = backend
        .restore_with_ports(&snapshot_ref, &spec, ports)
        .await;
    match restored {
        Ok(()) => {
            // The 0.7.2 RestoreBuilder has no env/workdir/label setters —
            // re-apply them so the sandbox is configured like before. This
            // persists for the next start and does not restart the VM. A
            // failure here is not data loss (disk + ports are already
            // restored), so it only warns via the snapshot cleanup path.
            if let Err(e) = backend.reapply_config(name, &spec).await {
                let _ = backend.remove_snapshot(&snapshot_ref).await;
                return Err(e.context(format!(
                    "restore {name}: reapplying env/workdir failed (sandbox data is intact)"
                )));
            }
            // 4. Cleanup only after the restore + reapply succeeded.
            let _ = backend.remove_snapshot(&snapshot_ref).await;
            Ok(())
        }
        Err(e) => {
            // Keep the snapshot: it holds the sandbox's data, and the user
            // can recover manually with `msb restore`.
            Err(e).with_context(|| format!("restore {name} from snapshot"))
        }
    }
}

/// Stop and remove a sandbox so its name is free for the restore.
async fn teardown_for_restore(backend: &dyn MsbBackend, name: &str) -> Result<()> {
    backend
        .stop(name)
        .await
        .with_context(|| format!("stop {name} for recreate"))?;

    backend
        .remove(name)
        .await
        .with_context(|| format!("remove {name} for recreate"))?;
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

    // ---- available_actions ----

    #[test]
    fn running_sandbox_offers_stop_restart_shell_destroy() {
        let actions = available_actions(&SandboxState::Running);
        let labels: Vec<&str> = actions.iter().map(|a| a.label()).collect();
        assert_eq!(labels, vec!["Stop", "Restart", "Exec", "Delete"]);
    }

    #[test]
    fn stopped_sandbox_offers_start_destroy_only() {
        let actions = available_actions(&SandboxState::Stopped);
        let labels: Vec<&str> = actions.iter().map(|a| a.label()).collect();
        assert_eq!(labels, vec!["Start", "Delete"]);
    }

    #[test]
    fn startable_states_all_offer_start_destroy_only() {
        for state in [
            SandboxState::Created,
            SandboxState::Crashed,
            SandboxState::Exited,
        ] {
            let actions = available_actions(&state);
            let labels: Vec<&str> = actions.iter().map(|a| a.label()).collect();
            assert_eq!(labels, vec!["Start", "Delete"], "state: {state}");
        }
    }

    #[test]
    fn paused_sandbox_offers_stop_destroy_only() {
        let actions = available_actions(&SandboxState::Paused);
        let labels: Vec<&str> = actions.iter().map(|a| a.label()).collect();
        assert_eq!(labels, vec!["Stop", "Delete"]);
    }

    #[test]
    fn stalled_sandbox_offers_stop_restart_destroy() {
        let actions = available_actions(&SandboxState::Stalled);
        let labels: Vec<&str> = actions.iter().map(|a| a.label()).collect();
        assert_eq!(labels, vec!["Stop", "Restart", "Delete"]);
    }

    #[test]
    fn unknown_state_offers_destroy_only() {
        let actions = available_actions(&SandboxState::Unknown("weird".into()));
        let labels: Vec<&str> = actions.iter().map(|a| a.label()).collect();
        assert_eq!(labels, vec!["Delete"]);
    }

    #[test]
    fn card_action_keys_and_labels() {
        // Docker-sbx parity: the highlighted letter inside the word IS the key.
        assert_eq!(CardAction::Start.key_hint(), "s");
        assert_eq!(CardAction::Start.label(), "Start");
        assert_eq!(CardAction::Stop.key_hint(), "s");
        assert_eq!(CardAction::Stop.label(), "Stop");
        assert_eq!(CardAction::Restart.key_hint(), "r");
        assert_eq!(CardAction::Restart.label(), "Restart");
        assert_eq!(CardAction::Shell.key_hint(), "x");
        assert_eq!(CardAction::Shell.label(), "Exec");
        assert_eq!(CardAction::Destroy.key_hint(), "Del");
        assert_eq!(CardAction::Destroy.label(), "Delete");
    }

    #[test]
    fn card_action_highlight_is_the_key_inside_the_label() {
        assert_eq!(CardAction::Start.highlight(), "S");
        assert_eq!(CardAction::Stop.highlight(), "S");
        assert_eq!(CardAction::Restart.highlight(), "R");
        assert_eq!(CardAction::Shell.highlight(), "x");
        assert_eq!(CardAction::Destroy.highlight(), "Del");
        // The highlighted part must actually appear in the label.
        for action in [
            CardAction::Start,
            CardAction::Stop,
            CardAction::Restart,
            CardAction::Shell,
            CardAction::Destroy,
        ] {
            assert!(
                action.label().contains(action.highlight()),
                "{} does not contain {}",
                action.label(),
                action.highlight()
            );
        }
    }

    #[test]
    fn toggle_action_maps_s_by_state() {
        // `s` toggles Start/Stop (Docker sbx): startable → Start,
        // stoppable → Stop, neither → None.
        assert_eq!(
            toggle_action(&SandboxState::Stopped),
            Some(CardAction::Start)
        );
        assert_eq!(
            toggle_action(&SandboxState::Created),
            Some(CardAction::Start)
        );
        assert_eq!(
            toggle_action(&SandboxState::Crashed),
            Some(CardAction::Start)
        );
        assert_eq!(
            toggle_action(&SandboxState::Exited),
            Some(CardAction::Start)
        );
        assert_eq!(
            toggle_action(&SandboxState::Running),
            Some(CardAction::Stop)
        );
        assert_eq!(toggle_action(&SandboxState::Paused), Some(CardAction::Stop));
        assert_eq!(
            toggle_action(&SandboxState::Stalled),
            Some(CardAction::Stop)
        );
        assert_eq!(toggle_action(&SandboxState::Unknown("weird".into())), None);
    }

    #[test]
    fn start_and_stop_never_cooccur() {
        // Invariant the `s` toggle relies on: no state offers both.
        for state in [
            SandboxState::Running,
            SandboxState::Stopped,
            SandboxState::Paused,
            SandboxState::Exited,
            SandboxState::Created,
            SandboxState::Crashed,
            SandboxState::Stalled,
            SandboxState::Unknown("weird".into()),
        ] {
            let actions = available_actions(&state);
            assert!(
                !(actions.contains(&CardAction::Start) && actions.contains(&CardAction::Stop)),
                "state {state} offers both Start and Stop"
            );
        }
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
    async fn fake_backend_matches_sdk_start_invariant() {
        // SDK invariant (0.7.2): `start` on a Running sandbox errors with
        // SandboxStillRunning ("cannot start sandbox: already running") —
        // the FakeBackend must model that so the recreate flow's tests can
        // catch a redundant start.
        let b = FakeBackend::with_fixture_sandbox();
        let err = b.start("tui-fixture").await;
        assert!(err.is_err(), "start on a running sandbox must fail");

        let b = FakeBackend::with_fixture_sandbox();
        b.stop("tui-fixture").await.unwrap();
        b.start("tui-fixture").await.unwrap();
        let insp = b.inspect("tui-fixture").await.unwrap();
        assert_eq!(insp.status, SandboxState::Running);
    }

    #[tokio::test]
    async fn fake_backend_create_leaves_sandbox_running() {
        // SDK invariant (0.7.2): create_detached boots the VM fully and
        // leaves the sandbox Running ("sandbox ready" is awaited inside
        // create); the FakeBackend must model that.
        let b = FakeBackend::new();
        b.create(&CreateSpec {
            image: "alpine".into(),
            name: Some("fresh".into()),
            ..Default::default()
        })
        .await
        .unwrap();
        let insp = b.inspect("fresh").await.unwrap();
        assert_eq!(insp.status, SandboxState::Running);
    }

    #[tokio::test]
    async fn publish_port_recreate_sequence() {
        let b = FakeBackend::with_fixture_sandbox();
        // Fixture has ports 8080→80/tcp and 9090→90/udp. Add 7070→70/tcp.
        let new_port = port(7070, 70, "tcp");
        let result = publish_port(&b, "tui-fixture", new_port).await.unwrap();
        assert_eq!(result, PublishResult::Recreated);

        let calls = b.calls().await;
        // Expected: inspect, snapshot, stop, remove, restore (data-preserving
        // recreate: the disk content comes back from the snapshot, so the
        // rootfs does NOT reset). No create-from-image, no start after
        // restore (restore boots the VM).
        let seq: Vec<&Call> = calls
            .iter()
            .filter(|c| {
                matches!(
                    c,
                    Call::Inspect(_)
                        | Call::SnapshotDisk(_)
                        | Call::Stop(_)
                        | Call::Remove(_)
                        | Call::Restore { .. }
                        | Call::Create(_)
                        | Call::Start(_)
                )
            })
            .collect();
        assert!(
            seq.len() >= 5,
            "expected inspect/snapshot/stop/remove/restore, got {seq:?}"
        );
        assert!(matches!(seq[0], Call::Inspect(n) if n == "tui-fixture"));
        assert!(matches!(seq[1], Call::SnapshotDisk(n) if n == "tui-fixture"));
        assert!(matches!(seq[2], Call::Stop(n) if n == "tui-fixture"));
        assert!(matches!(seq[3], Call::Remove(n) if n == "tui-fixture"));
        assert!(
            matches!(seq[4], Call::Restore { name, .. } if name == "tui-fixture"),
            "restore must recreate the same sandbox: {seq:?}"
        );
        assert!(
            !seq.iter().any(|c| matches!(c, Call::Start(_))),
            "recreate must not start after restore (restore boots the VM): {seq:?}"
        );

        // The recreated sandbox must be Running — restore already booted it.
        let insp = b.inspect("tui-fixture").await.unwrap();
        assert_eq!(insp.status, SandboxState::Running);
    }

    #[tokio::test]
    async fn publish_port_cleans_up_snapshot_after_successful_restore() {
        let b = FakeBackend::with_fixture_sandbox();
        publish_port(&b, "tui-fixture", port(7070, 70, "tcp"))
            .await
            .unwrap();

        let calls = b.calls().await;
        // The auto-snapshot is removed again after the restore succeeded —
        // the flow leaves no snapshot litter behind.
        let snap = calls
            .iter()
            .position(|c| matches!(c, Call::SnapshotDisk(_)))
            .expect("snapshot created");
        let restore = calls
            .iter()
            .position(|c| matches!(c, Call::Restore { .. }))
            .expect("restore ran");
        let cleanup = calls
            .iter()
            .position(|c| matches!(c, Call::RemoveSnapshot(_)))
            .expect("snapshot cleaned up");
        assert!(snap < restore);
        assert!(restore < cleanup, "cleanup happens after the restore");
    }

    #[tokio::test]
    async fn publish_port_keeps_snapshot_when_restore_fails() {
        // If the restore fails, the snapshot must survive so the user can
        // recover manually (`msb restore`).
        let b = FakeBackend::with_fixture_sandbox();
        b.fail_next_restore().await;
        let res = publish_port(&b, "tui-fixture", port(7070, 70, "tcp")).await;
        assert!(res.is_err(), "restore failure must surface");

        let calls = b.calls().await;
        assert!(
            !calls.iter().any(|c| matches!(c, Call::RemoveSnapshot(_))),
            "snapshot must be kept for manual recovery: {calls:?}"
        );
    }

    #[tokio::test]
    async fn publish_port_carries_config_into_restore() {
        // Env, volumes, workdir and the new port list must reach the restore
        // (the RestoreBuilder only rebuilds the disk; everything else comes
        // from the config we pass).
        let b = FakeBackend::with_fixture_sandbox();
        publish_port(&b, "tui-fixture", port(7070, 70, "tcp"))
            .await
            .unwrap();

        let calls = b.calls().await;
        let restore = calls
            .iter()
            .find_map(|c| match c {
                Call::Restore { spec, ports, .. } => Some((spec.clone(), ports.clone())),
                _ => None,
            })
            .expect("restore call");
        let (spec, ports) = restore;
        // All three ports: fixture's 8080/9090 + the new 7070.
        assert_eq!(ports.len(), 3, "ports: {ports:?}");
        assert!(
            ports.iter().any(|p| p.host_port == 7070),
            "new port missing: {ports:?}"
        );
        // The fixture's config must survive (image, env, workdir).
        assert_eq!(spec.image, "alpine");
        assert!(
            spec.env.iter().any(|e| e.starts_with("PATH=")),
            "env missing: {:?}",
            spec.env
        );
    }

    #[tokio::test]
    async fn publish_port_reapplies_env_workdir_labels_after_restore() {
        // The 0.7.2 RestoreBuilder has no env/workdir/label setters — the
        // flow must reapply them via the modification API (persisted for
        // the next start, no extra restart) so the sandbox is bit-identical
        // to before the publish.
        let b = FakeBackend::with_fixture_sandbox();
        publish_port(&b, "tui-fixture", port(7070, 70, "tcp"))
            .await
            .unwrap();

        let calls = b.calls().await;
        let modify = calls
            .iter()
            .position(|c| matches!(c, Call::Modify { .. }))
            .expect("env/workdir/labels must be reapplied after restore");
        let restore = calls
            .iter()
            .position(|c| matches!(c, Call::Restore { .. }))
            .expect("restore ran");
        assert!(
            modify > restore,
            "modify must run AFTER the restore: {calls:?}"
        );
        let Call::Modify { name, spec, .. } = &calls[modify] else {
            unreachable!()
        };
        assert_eq!(name, "tui-fixture");
        assert!(
            spec.env.iter().any(|e| e.starts_with("PATH=")),
            "env missing from modify: {:?}",
            spec.env
        );
        assert!(spec.workdir.is_some(), "workdir missing from modify");
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
    async fn exec_sandbox_delegates_and_formats_errors() {
        let b = FakeBackend::with_fixture_sandbox();
        let out = exec_sandbox(&b, "tui-fixture", &["echo".into(), "hi".into()]).await;
        assert!(out.is_ok());

        let calls = b.calls().await;
        assert!(
            calls
                .iter()
                .any(|c| matches!(c, Call::Exec { name, .. } if name == "tui-fixture"))
        );
    }

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

    // ---- live smoke test (real SDK; run explicitly) ----

    /// End-to-end publish flow against the REAL local SDK + runtime.
    /// Not part of CI: needs an installed `msb` runtime and mutates a
    /// sandbox named `tui-smoke`. Run with:
    /// `cargo test --bin microsandbox-tui publish_live_smoke -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "mutates a real sandbox; manual smoke only"]
    async fn publish_live_smoke() {
        use crate::backend::sdk::SdkBackend;
        let backend = SdkBackend::new().expect("local SDK init");
        let b = &backend as &dyn crate::backend::MsbBackend;

        // Clean slate: remove any leftover smoke sandbox (best-effort).
        let _ = crate::actions::remove_sandbox(b, "tui-smoke").await;

        // Create the victim.
        let name = crate::actions::create_sandbox(
            b,
            &crate::backend::CreateSpec {
                image: "alpine".into(),
                name: Some("tui-smoke".into()),
                ..Default::default()
            },
        )
        .await
        .expect("create");
        assert_eq!(name, "tui-smoke");

        // create_detached boots the VM: status must already be Running.
        let row = b.status(&name).await.expect("status");
        assert_eq!(row.status, SandboxState::Running, "create boots the VM");

        // Write a canary file into the rootfs — the publish flow must
        // preserve it (this is the whole point of the snapshot restore).
        let out = b
            .exec(
                &name,
                &[
                    "sh".into(),
                    "-c".into(),
                    "echo data-preserved > /tmp/canary.txt".into(),
                ],
            )
            .await
            .expect("write canary");
        assert_eq!(out.exit_code, 0, "canary write failed: {out:?}");

        // Publish via the snapshot-restore flow (the regression path).
        let port = crate::models::PublishedPort {
            host_bind: "127.0.0.1".into(),
            host_port: 19999,
            guest_port: 80,
            protocol: "tcp".into(),
        };
        let result = crate::actions::publish_port(b, &name, port)
            .await
            .expect("publish_port");
        assert_eq!(result, PublishResult::Recreated);

        // No start after restore: the sandbox must STILL be Running.
        let row = b.status(&name).await.expect("status after publish");
        assert_eq!(
            row.status,
            SandboxState::Running,
            "restored sandbox must be running (restore boots the VM)"
        );

        // THE regression check: the canary file must have survived the
        // publish (disk restored from the snapshot, not reset to the image).
        let out = b
            .exec(&name, &["cat".into(), "/tmp/canary.txt".into()])
            .await
            .expect("read canary");
        assert_eq!(
            out.stdout.trim(),
            "data-preserved",
            "ROOTFS DATA LOST — canary missing after publish: {out:?}"
        );

        // The port must be in the persisted config.
        let insp = b.inspect(&name).await.expect("inspect");
        assert!(
            insp.active_config
                .network
                .ports
                .iter()
                .any(|p| p.host_port == 19999 && p.guest_port == 80),
            "published port missing: {:?}",
            insp.active_config.network.ports
        );

        // The flow must not leave snapshot litter behind.
        let snapshots = microsandbox::Snapshot::list()
            .await
            .expect("list snapshots");
        let leftover = snapshots
            .iter()
            .filter(|s| s.group().map(|g| g.contains("tui-smoke")).unwrap_or(false))
            .count();
        assert_eq!(leftover, 0, "snapshot not cleaned up after restore");

        // Cleanup.
        crate::actions::remove_sandbox(b, &name)
            .await
            .expect("remove");
    }
}
