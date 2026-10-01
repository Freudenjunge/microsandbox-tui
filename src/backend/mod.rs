//! Backend abstraction over microsandbox.
//!
//! The [`MsbBackend`] trait is the seam between UI and the outside world:
//! [`sdk::SdkBackend`] drives the typed SDK in production; [`fake::FakeBackend`]
//! serves unit tests.

#![allow(dead_code)]

pub mod fake;
pub mod sdk;

use anyhow::Result;
use async_trait::async_trait;

use crate::models::{
    Image, Metrics, PublishedPort, SandboxInspect, SandboxStatusRow, SandboxSummary, Volume,
};

/// A live log line delivered to the logs view.
#[derive(Debug, Clone, PartialEq)]
pub struct LogLine {
    /// Session id (0 for non-session/system entries).
    pub id: u64,
    /// Stream source (stdout/stderr/output/system).
    pub source: String,
    /// Decoded entry text.
    pub data: String,
    /// Entry timestamp.
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Operations the TUI performs against microsandbox.
///
/// Implementations: [`sdk::SdkBackend`] (typed SDK, production) and a fake for
/// unit tests. UI code depends only on this trait.
#[async_trait]
pub trait MsbBackend: Send + Sync {
    // ---- read ----

    /// All sandboxes, newest first, plus per-sandbox warnings for rows whose
    /// stored config could not be parsed (such rows are still listed, with
    /// image "?"); the caller surfaces the warnings (e.g. as error events).
    async fn list_sandboxes(&self) -> Result<(Vec<SandboxSummary>, Vec<String>)>;

    /// Status row for one sandbox.
    async fn status(&self, name: &str) -> Result<SandboxStatusRow>;

    /// Full lifecycle info for one sandbox.
    async fn inspect(&self, name: &str) -> Result<SandboxInspect>;

    /// One metrics sample for every sandbox.
    async fn metrics(&self) -> Result<Vec<Metrics>>;

    /// All named volumes.
    async fn list_volumes(&self) -> Result<Vec<Volume>>;

    /// All cached images.
    async fn list_images(&self) -> Result<Vec<Image>>;

    // ---- lifecycle ----

    /// Start the named sandbox.
    async fn start(&self, name: &str) -> Result<()>;

    /// Gracefully stop the named sandbox.
    async fn stop(&self, name: &str) -> Result<()>;

    /// Stop and start the named sandbox.
    async fn restart(&self, name: &str) -> Result<()>;

    /// Remove the named sandbox (must be stopped).
    async fn remove(&self, name: &str) -> Result<()>;

    /// Create a disk snapshot of `name` (running sources supported) and
    /// return its restore reference (group/member name or id).
    async fn snapshot_disk(&self, name: &str) -> Result<String>;

    /// Restore `snapshot_ref` into a detached sandbox `name`, publishing
    /// `ports` and applying the rest of `spec` (env, volumes, workdir).
    /// Cold-boots the captured disk — the sandbox's data is preserved.
    async fn restore_with_ports(
        &self,
        snapshot_ref: &str,
        spec: &CreateSpec,
        ports: Vec<PublishedPort>,
    ) -> Result<()>;

    /// Delete a snapshot by its restore reference (best-effort cleanup).
    async fn remove_snapshot(&self, snapshot_ref: &str) -> Result<()>;

    /// Reapply a sandbox's env/workdir/labels after a restore (persisted
    /// for the next start; the running VM is not restarted). The 0.7.2
    /// RestoreBuilder cannot set these itself.
    async fn reapply_config(&self, name: &str, spec: &CreateSpec) -> Result<()>;

    // ---- creation ----

    /// Create a sandbox from this spec; returns the sandbox name.
    async fn create(&self, spec: &CreateSpec) -> Result<String>;

    /// Stream log lines for one sandbox.
    /// Dropping the returned stream tears down the follow task.
    async fn logs_follow(&self, name: &str) -> Result<LogStream>;

    /// Run a command inside a sandbox and capture its output.
    ///
    /// Non-interactive: stdin is not connected, stdout and stderr are
    /// captured separately, and the command's exit code is returned.
    async fn exec(&self, name: &str, cmd: &[String]) -> Result<ExecOutput>;
}

/// The result of running a command inside a sandbox.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecOutput {
    /// Captured stdout of the command.
    pub stdout: String,
    /// Captured stderr of the command (streams are separated).
    pub stderr: String,
    /// Process exit code of the executed command.
    pub exit_code: i32,
}

/// Creation parameters for [`MsbBackend::create`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CreateSpec {
    /// OCI image reference (e.g. `alpine`, `python:3.12`).
    pub image: String,
    /// Sandbox name; `None` = auto-generate one.
    pub name: Option<String>,
    /// vCPU count.
    pub cpus: Option<u32>,
    /// Memory limit, e.g. `512M`, `1G`.
    pub memory: Option<String>,
    /// Working directory inside the sandbox.
    pub workdir: Option<String>,
    /// Published ports.
    pub ports: Vec<PublishedPort>,
    /// Volume mounts, in `SOURCE:DEST[:OPTIONS]` form.
    pub volumes: Vec<String>,
    /// Environment variables, `KEY=VALUE` form.
    pub env: Vec<String>,
    /// Labels, `KEY=VALUE` form.
    pub labels: Vec<String>,
    /// Network profile(s): `public`, `private`, `host`, or `None` for default.
    pub net_profile: Option<String>,
    /// Raw `--net-rule` tokens.
    pub net_rules: Vec<String>,
}

/// A follow stream of log lines for one sandbox.
pub type LogStream = std::pin::Pin<Box<dyn tokio_stream::Stream<Item = LogLine> + Send>>;
