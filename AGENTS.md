# AGENTS.md

## What this is

Single-crate Rust binary: `msb-tui`, a ratatui/crossterm dashboard for
microsandbox microVMs. Rust 2024 edition. It talks to microsandbox
**exclusively through the typed `microsandbox` 0.7.8 Rust SDK**
(`src/backend/sdk.rs`) — no subprocess calls to the `msb` CLI, no JSON
scraping. This is a deliberate design decision, not an accident; see
`docs/DESIGN.md` for architecture, trade-offs, and the recreate flow.

## Gates (exact order; both CI and release re-run these)

```sh
cargo fmt --check
cargo clippy -- -D warnings   # all warnings are errors — fix, don't #[allow]
cargo test
cargo check
```

`cargo test` needs no runtime or services: model tests parse fixtures from
`src/fixtures/`, action-layer tests run against the in-memory `FakeBackend`
(`src/backend/fake.rs`). All tests are inline `#[cfg(test)]` modules — there
is no `tests/` dir; run one with `cargo test <name>`.

## Build prerequisite

`libcap-ng-dev` (Debian/Ubuntu) is required to build/link — a system link
dependency of the microsandbox SDK. CI installs it; a fresh machine fails at
build time without it.

## Non-obvious invariants

- **Detached lifecycle**: the SDK's default is *attached* sandboxes, which die
  with the calling process. Every create/start/restart must run detached
  (`detached(true)`, `start_detached()`, `RestartOptions { detached: true }`)
  so sandboxes survive the TUI being closed.
- **Backend trait first**: high-level ops live in `src/actions.rs` and go
  through the `MsbBackend` trait (`src/backend/mod.rs`); never call SDK types
  from views or actions directly. Add new capabilities to the trait, then
  implement in `sdk.rs` and `fake.rs`.
- **Port/network changes require a recreate-from-snapshot**: the pinned SDK
  0.7.8 still cannot modify ports or network rules on a live VM. The flow is
  snapshot → stop → remove → restore-with-new-ports → re-apply
  env/workdir/labels (details in `docs/DESIGN.md`, "Port Publish/Unpublish").
  The remove must come first: the RestoreBuilder has no replace option (the
  image-based `SandboxBuilder::replace()` exists but resets the rootfs).
  Upstream PR #1789 (unreleased; merged after v0.7.8) adds
  modify-based port changes — adopt it and retire the snapshot-recreate
  port flow when that SDK release lands; network rules stay recreate-only
  (upstream #1538 open).
- **Theme is the single color source**: view code uses `ui::theme::THEME` and
  shared chrome (`ui/chrome.rs`); no hardcoded `Color::` values anywhere else.
- **The UI never blocks**: event loop is `tokio::select!` over input + pollers;
  long SDK calls run on spawned tasks and report back through the event
  channel.
- **Built-in templates are embedded** via `include_str!` in `src/template.rs`
  from `src/templates/*.toml` — editing one has no effect until rebuild. User
  templates in `~/.config/microsandbox-tui/templates/` shadow built-ins by id.

## Releases

Pushing a `v*` tag triggers `release.yml`: gates re-run, then a 3-target
build matrix (Linux x86_64/arm64, macOS aarch64) publishes the GitHub Release
with `.tar.gz` + `.sha256` assets. Just tag the release commit: the workflow
injects the tag as `MSB_TUI_VERSION`, so `Cargo.toml` stays pinned at
`0.0.0-dev` — never bump `version` by hand.