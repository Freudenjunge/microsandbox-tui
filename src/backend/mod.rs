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

/// One page of a cursor-keyed list endpoint (backend-agnostic twin of the
/// SDK's `SandboxPage`).
pub struct ListPage<T> {
    /// Items in this page.
    pub items: Vec<T>,
    /// Opaque continuation, `None` when this is the final page.
    pub next: Option<String>,
}

/// Page a cursor-keyed list endpoint **to completion**.
///
/// The SDK answers cursor-keyed list requests in pages (a default builder
/// request delivers 20 rows — the first page only). Treating a single
/// page as the whole fleet silently hides everything older: the dashboard
/// misses cards, the autostart prune drops marks of unknown-but-real
/// sandboxes (data loss), and the boot pass never starts them. `fetch`
/// receives the previous page's cursor (`None` = first page). A backend
/// that never answers `next: None` stops after `max_pages` pages with an
/// error instead of looping forever.
pub(crate) async fn list_all_pages<T, E, Fut, F>(
    mut fetch: F,
    max_pages: u32,
) -> anyhow::Result<Vec<T>>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: std::future::Future<Output = std::result::Result<ListPage<T>, E>>,
    E: std::fmt::Display,
{
    let mut out: Vec<T> = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..max_pages {
        let page = fetch(cursor.take())
            .await
            .map_err(|e| anyhow::anyhow!("list pages failed: {e}"))?;
        out.extend(page.items);
        match page.next {
            Some(next) => cursor = Some(next),
            None => return Ok(out),
        }
    }
    anyhow::bail!("list cursor never ends after {max_pages} pages — aborting")
}

#[cfg(test)]
mod tests {
    use super::*;

    type FutBox = std::pin::Pin<
        Box<dyn std::future::Future<Output = std::result::Result<ListPage<u32>, String>>>,
    >;

    fn page(items: &[u32], next: Option<&str>) -> ListPage<u32> {
        ListPage {
            items: items.to_vec(),
            next: next.map(String::from),
        }
    }

    /// A ready future resolving to `r`, typed for the fetch closure.
    fn fut(r: std::result::Result<ListPage<u32>, String>) -> FutBox {
        Box::pin(std::future::ready(r))
    }

    #[tokio::test]
    async fn follows_cursors_until_exhausted() {
        // Chain: [1] --c1--> [2] --c2--> [3] --None. Each call receives
        // exactly the previous page's cursor; the first gets none.
        let mut received: Vec<Option<String>> = Vec::new();
        let mut chain = std::collections::VecDeque::new();
        chain.push_back(Ok(page(&[1], Some("c1"))));
        chain.push_back(Ok(page(&[2], Some("c2"))));
        chain.push_back(Ok(page(&[3], None)));
        let items = list_all_pages(
            |cursor: Option<String>| {
                received.push(cursor);
                fut(chain.pop_front().expect("canned chain covers every call"))
            },
            100,
        )
        .await
        .unwrap();
        assert_eq!(items, vec![1, 2, 3]);
        assert_eq!(
            received,
            vec![None, Some("c1".into()), Some("c2".into())],
            "cursor must be threaded through every request"
        );
    }

    #[tokio::test]
    async fn stops_on_a_cursorless_final_page() {
        let items = list_all_pages(|_| fut(Ok(page(&[7], None))), 100)
            .await
            .unwrap();
        assert_eq!(items, vec![7]);
    }

    #[tokio::test]
    async fn first_page_failure_bubbles() {
        let err = list_all_pages::<u32, String, _, _>(|_| fut(Err("boom".to_string())), 100)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("boom"), "{err}");
    }

    #[tokio::test]
    async fn later_page_failure_bubbles() {
        let mut chain = std::collections::VecDeque::new();
        chain.push_back(Ok(page(&[1], Some("c1"))));
        chain.push_back(Err("mid-flight".to_string()));
        let err = list_all_pages::<u32, String, _, _>(
            |_| fut(chain.pop_front().expect("canned chain covers every call")),
            100,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("mid-flight"), "{err}");
    }

    #[tokio::test]
    async fn a_repeating_cursor_stops_at_the_cap_instead_of_looping() {
        let items = list_all_pages::<u32, String, _, _>(|_| fut(Ok(page(&[1], Some("same")))), 3)
            .await
            .expect_err("must not loop forever");
        assert!(
            items.to_string().contains("3"),
            "cap must be part of the error: {items}"
        );
    }
}
