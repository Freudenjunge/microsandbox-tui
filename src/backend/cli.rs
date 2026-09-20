//! CLI backend: drives the installed `msb` binary.

#![allow(dead_code)]

use std::process::Stdio;

use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio::process::Command;
use tokio_stream::StreamExt as _;
use tokio_util::codec::{FramedRead, LinesCodec};

use crate::models::{
    Image, Metrics, SandboxInspect, SandboxState, SandboxStatusRow, SandboxSummary, Volume,
};

use super::{CreateSpec, ExecOutput, LogEntry, LogLine, LogStream, MsbBackend};

/// Backend that shells out to the user's installed `msb` CLI.
#[derive(Debug, Clone)]
pub struct CliBackend {
    /// Path or name of the msb binary (test-injectable).
    program: String,
}

impl CliBackend {
    /// Locate `msb` on `$PATH` at construction time.
    pub fn new() -> Result<Self> {
        let program = which::which("msb")
            .map(|p| p.to_string_lossy().into_owned())
            .map_err(|_| {
                anyhow::anyhow!(
                    "msb CLI not found on PATH. Install it with:\n  \
                     curl -fsSL https://install.microsandbox.dev | sh"
                )
            })?;
        Ok(Self { program })
    }

    /// Build a backend that runs a specific binary path (used by tests).
    pub fn with_program(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
        }
    }

    /// Construct a `Command` for a subcommand with a diagnostic context.
    fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(args).stdin(Stdio::null());
        cmd
    }

    /// The program name/path this backend invokes (used by tests).
    pub fn program(&self) -> &str {
        &self.program
    }

    /// Run a command, return stdout as string with context-rich errors.
    async fn run(&self, args: &[&str]) -> Result<String> {
        let out = self
            .cmd(args)
            .output()
            .await
            .with_context(|| format!("failed to spawn: msb {}", args.join(" ")))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            anyhow::bail!("msb {} failed: {}", args.join(" "), stderr);
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Run a subcommand and parse its JSON output as `T`.
    async fn run_json<T: serde::de::DeserializeOwned>(&self, args: &[&str]) -> Result<T> {
        let text = self.run(args).await?;
        serde_json::from_str(&text).with_context(|| {
            format!(
                "unexpected JSON from msb {}: {}",
                args.join(" "),
                &text[..text.len().min(400)]
            )
        })
    }
}

/// Flatten `Vec<PublishedPort>` into repeated `-p FLAG` pairs.
pub(crate) fn port_flags(ports: &[crate::models::PublishedPort]) -> Vec<String> {
    let mut out = Vec::with_capacity(ports.len() * 2);
    for p in ports {
        out.push("-p".to_string());
        out.push(p.to_cli_flag());
    }
    out
}

#[async_trait]
impl MsbBackend for CliBackend {
    async fn list_sandboxes(&self) -> Result<Vec<SandboxSummary>> {
        self.run_json(&["ls", "--format", "json"]).await
    }

    async fn status(&self, name: &str) -> Result<SandboxStatusRow> {
        self.run_json(&["ps", name, "--format", "json"]).await
    }

    async fn inspect(&self, name: &str) -> Result<SandboxInspect> {
        self.run_json(&["inspect", name, "--format", "json"]).await
    }

    async fn metrics(&self) -> Result<Vec<Metrics>> {
        self.run_json(&["metrics", "--all", "--format", "json"])
            .await
    }

    async fn list_volumes(&self) -> Result<Vec<Volume>> {
        self.run_json(&["volumes", "--format", "json"]).await
    }

    async fn list_images(&self) -> Result<Vec<Image>> {
        self.run_json(&["images", "--format", "json"]).await
    }

    async fn start(&self, name: &str) -> Result<()> {
        self.run(&["start", name]).await.map(|_| ())
    }

    async fn stop(&self, name: &str) -> Result<()> {
        self.run(&["stop", name]).await.map(|_| ())
    }

    async fn restart(&self, name: &str) -> Result<()> {
        self.run(&["restart", name]).await.map(|_| ())
    }

    async fn remove(&self, name: &str) -> Result<()> {
        self.run(&["rm", name]).await.map(|_| ())
    }

    async fn create(&self, spec: &CreateSpec) -> Result<String> {
        let mut args: Vec<String> = vec!["create".into()];
        args.extend(spec.to_args());
        let argrefs: Vec<&str> = args.iter().map(String::as_str).collect();
        let text = self.run(&argrefs).await?;
        // `msb create` prints progress lines; the sandbox name is echoed.
        let name = spec.name.clone().unwrap_or_else(|| {
            text.lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or_default()
                .trim()
                .to_string()
        });
        Ok(name)
    }

    async fn logs_follow(&self, name: &str) -> Result<LogStream> {
        let mut child = self
            .cmd(&["logs", name, "-f", "--json"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to spawn msb logs {name}"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("msb logs produced no stdout"))?;
        let framed = FramedRead::new(stdout, LinesCodec::new());
        let stream = framed.filter_map(|line| match line {
            Ok(line) if !line.trim().is_empty() => serde_json::from_str::<LogEntry>(&line)
                .ok()
                .map(LogLine::from),
            _ => None,
        });
        let stream: LogStream = Box::pin(stream);
        Ok(stream)
    }

    async fn exec(&self, name: &str, cmd: &[String]) -> Result<ExecOutput> {
        // `msb exec <name> -- <cmd...>`: stdout and stderr are captured
        // separately (verified on msb 0.7.2), stdin is closed.
        let cmd_strs: Vec<&str> = std::iter::once(name)
            .chain(std::iter::once("--"))
            .chain(cmd.iter().map(String::as_str))
            .collect();
        let out = self
            .cmd(&cmd_strs)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .with_context(|| format!("failed to spawn: msb exec {name}"))?;

        Ok(ExecOutput {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            exit_code: out.status.code().unwrap_or(-1),
        })
    }
}

/// Ensure `SandboxState` import is used (referenced in docs of trait consumers).
#[allow(unused)]
fn _state_marker(_: Option<SandboxState>) {}
