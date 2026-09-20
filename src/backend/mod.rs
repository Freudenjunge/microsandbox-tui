//! Backend abstraction over the `msb` CLI.
//!
//! NOTE(dead_code): the trait, types, and CLI impl gain runtime callers in
//! Task 3 (actions) and Task 4 (app). Remove these allows once those land.

#![allow(dead_code)]

pub mod cli;
pub mod fake;
#[cfg(test)]
mod tests;

use anyhow::Result;
use async_trait::async_trait;

use crate::models::{
    Image, Metrics, PublishedPort, SandboxInspect, SandboxStatusRow, SandboxSummary, Volume,
};

/// One decoded entry of `msb logs --json` (JSON Lines).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LogEntry {
    /// Entry body, with trailing newline as emitted.
    pub d: String,
    /// Session id.
    pub id: u64,
    /// Source stream: `stdout`, `stderr`, `output`, `system`.
    pub s: String,
    /// RFC 3339 timestamp.
    pub t: String,
    /// (unused by CLI today; kept optional for forward compatibility)
    #[serde(default, skip_serializing)]
    pub e: Option<serde_json::Value>,
}

impl LogEntry {
    /// Parsed timestamp.
    pub fn timestamp(&self) -> chrono::ParseResult<chrono::DateTime<chrono::Utc>> {
        chrono::DateTime::parse_from_rfc3339(&self.t).map(chrono::DateTime::<chrono::Utc>::from)
    }

    /// Source stream (alias, matches models naming).
    pub fn source(&self) -> &str {
        &self.s
    }

    /// Payload data.
    pub fn data(&self) -> &str {
        &self.d
    }
}

/// A live log line delivered by a follow task.
#[derive(Debug, Clone, PartialEq)]
pub struct LogLine {
    /// Session id from `msb logs --json`.
    pub id: u64,
    /// Stream source (stdout/stderr/output/system).
    pub source: String,
    /// Decoded entry text.
    pub data: String,
    /// Entry timestamp.
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl From<LogEntry> for LogLine {
    fn from(e: LogEntry) -> Self {
        let timestamp = e
            .timestamp()
            .unwrap_or(chrono::DateTime::<chrono::Utc>::UNIX_EPOCH);
        Self {
            id: e.id,
            source: e.s,
            data: e.d,
            timestamp,
        }
    }
}

/// Operations the TUI performs against microsandbox.
///
/// Implementations: [`cli::CliBackend`] (drives installed `msb`) and a fake for
/// unit tests. UI code depends only on this trait.
#[async_trait]
pub trait MsbBackend: Send + Sync {
    // ---- read ----

    /// `msb ls --format json` — all sandboxes.
    async fn list_sandboxes(&self) -> Result<Vec<SandboxSummary>>;

    /// `msb ps <name> --format json` — status row for one sandbox.
    async fn status(&self, name: &str) -> Result<SandboxStatusRow>;

    /// `msb inspect <name> --format json`.
    async fn inspect(&self, name: &str) -> Result<SandboxInspect>;

    /// `msb metrics --all --format json` — one sample for every sandbox.
    async fn metrics(&self) -> Result<Vec<Metrics>>;

    /// `msb volumes --format json`.
    async fn list_volumes(&self) -> Result<Vec<Volume>>;

    /// `msb images --format json`.
    async fn list_images(&self) -> Result<Vec<Image>>;

    // ---- lifecycle ----

    /// `msb start <name>`.
    async fn start(&self, name: &str) -> Result<()>;

    /// `msb stop <name>`.
    async fn stop(&self, name: &str) -> Result<()>;

    /// `msb restart <name>`.
    async fn restart(&self, name: &str) -> Result<()>;

    /// `msb rm <name>` (sandbox must be stopped).
    async fn remove(&self, name: &str) -> Result<()>;

    // ---- creation ----

    /// Create a sandbox.
    ///
    /// Implementations build the `msb create` invocation from this spec; the
    /// image command (if any) runs the resolved image default.
    async fn create(&self, spec: &CreateSpec) -> Result<String>;

    /// Spawn `msb logs <name> -f --json`, returning a stream of decoded lines.
    /// Dropping the returned stream kills the child process.
    async fn logs_follow(&self, name: &str) -> Result<LogStream>;

    /// Run a command inside a sandbox via `msb exec` and capture its output.
    ///
    /// Non-interactive: stdin is not connected, stdout and stderr are
    /// captured separately, and the command's exit code is returned.
    async fn exec(&self, name: &str, cmd: &[String]) -> Result<ExecOutput>;
}

/// The result of running a command inside a sandbox via `msb exec`.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecOutput {
    /// Captured stdout of the command.
    pub stdout: String,
    /// Captured stderr of the command (`msb exec` separates the streams).
    pub stderr: String,
    /// Process exit code of the executed command.
    pub exit_code: i32,
}

/// Creation parameters for [`MsbBackend::create`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CreateSpec {
    /// OCI image reference (e.g. `alpine`, `python:3.12`).
    pub image: String,
    /// Sandbox name; empty = let `msb` generate one.
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
    /// Network profile(s): `public`, `private`, `host`, or None for msb default.
    pub net_profile: Option<String>,
    /// Raw `--net-rule` tokens.
    pub net_rules: Vec<String>,
}

impl CreateSpec {
    /// Build the `msb create` argument vector (without the `msb` program name).
    pub fn to_args(&self) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();
        if let Some(name) = &self.name {
            args.push("--name".into());
            args.push(name.clone());
        }
        if let Some(cpus) = self.cpus {
            args.push("-c".into());
            args.push(cpus.to_string());
        }
        if let Some(mem) = &self.memory {
            args.push("-m".into());
            args.push(mem.clone());
        }
        if let Some(wd) = &self.workdir {
            args.push("-w".into());
            args.push(wd.clone());
        }
        for p in &self.ports {
            args.push("-p".into());
            args.push(p.to_cli_flag());
        }
        for v in &self.volumes {
            args.push("-v".into());
            args.push(v.clone());
        }
        for e in &self.env {
            args.push("-e".into());
            args.push(e.clone());
        }
        for l in &self.labels {
            args.push("--label".into());
            args.push(l.clone());
        }
        if let Some(profile) = &self.net_profile {
            args.push("--net".into());
            args.push(profile.clone());
        }
        for rule in &self.net_rules {
            args.push("--net-rule".into());
            args.push(rule.clone());
        }
        args.push(self.image.clone());
        args
    }
}

/// A handle to a running `msb logs -f` child process.
pub type LogStream = std::pin::Pin<Box<dyn tokio_stream::Stream<Item = LogLine> + Send>>;
