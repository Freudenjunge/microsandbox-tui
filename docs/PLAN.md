# Phase 1 — MVP TUI

Goal: a working dashboard with live sandbox cards, create form, lifecycle actions, exec/logs, and read-only port listing.

## Task List

### 1. Data models + fixtures
- [x] Add `src/models.rs` with serde structs for `SandboxSummary`, `SandboxConfig`, `PublishedPort`, `NetworkConfig`, `Mount`, `Resources`, `Metrics`, `Volume`, `Image`, `EnvVar`, `RuntimeConfig`, `Lifecycle`, `ImageConfig`.
- [x] Add real captured JSON fixtures under `src/fixtures/`:
  - `sandbox-list.json`
  - `sandbox-inspect.json`
  - `metrics.json`
  - `volumes.json`
  - `images.json`
- [x] Write unit tests that parse every fixture and assert all expected fields.

### 2. Backend trait + CLI implementation
- [x] Define `MsbBackend` trait in `src/backend/mod.rs` with async methods:
  - `list_sandboxes`, `status`, `inspect`, `metrics`
  - `start`, `stop`, `restart`, `remove`
  - `create`, `exec`, `logs_follow`
  - `list_volumes`, `list_images`
- [x] Implement `CliBackend` in `src/backend/cli.rs` using `tokio::process::Command` and `--format json`.
- [ ] Add a `FakeBackend` (in-memory) for unit tests of the action layer.
- [x] Write unit tests for CLI command vector generation and JSON parsing.

### 3. High-level actions
- [ ] Add `src/actions.rs` with user-facing operations:
  - `create_sandbox`, `stop_sandbox`, `start_sandbox`, `restart_sandbox`, `remove_sandbox`
  - `exec_in_sandbox`, `stream_logs`
  - `publish_port`, `unpublish_port` (stop → recreate → start with warning)
- [ ] Unit-test the recreate flow against `FakeBackend`.

### 4. App state + event loop
- [x] Add `src/app.rs` with `App` struct holding current view, selected sandbox index, poller handles, last error, and pending action state.
- [x] Set up `tokio::select!` event loop in `src/event.rs` (or `app.rs`) combining crossterm events, metric poll ticks, and list poll ticks.
- [ ] Render a basic frame with status/error line.

### 5. Dashboard UI
- [ ] Implement `src/ui/dashboard.rs`: responsive card grid showing name, state, image, CPU%, memory, net I/O, ports, uptime.
- [ ] Bind keys: `↑↓` select, `Enter` inspect, `c` create, `e` exec, `l` logs, `p` ports, `r` restart, `x` stop, `Del` remove (confirm), `q` quit.

### 6. Create-sandbox form
- [ ] Implement `src/ui/create.rs` with fields for image, name, CPUs, memory, workdir, ports, volumes, env vars, network profile.
- [ ] Image autocomplete from `list_images`.
- [ ] On submit, call `actions::create_sandbox` and return to dashboard with spinner/status.

### 7. Logs panel
- [ ] Implement `src/ui/logs.rs`: stream `msb logs -f --json` in a long-lived task, render with timestamps.
- [ ] Bind `f` follow toggle, `Esc` back.

### 8. Ports view (read-only Phase 1)
- [ ] Implement `src/ui/ports.rs`: show published ports from `inspect.network.ports`.
- [ ] Bind `+` / `-` only if backend supports recreation; otherwise show message "Publish/unpublish requires recreate in msb 0.7.2".

### 9. Help overlay
- [ ] Implement `src/ui/help.rs`: context-aware keybinding overlay shown with `?`.

### 10. Main wiring
- [ ] Wire everything in `src/main.rs`: parse CLI, verify `msb` on PATH, init terminal, run event loop, restore terminal on exit.
- [ ] Add `cargo test` and `cargo run -- --help` smoke tests in CI or commit hook.

### 11. Final Phase 1 polish
- [ ] Ensure all Phase 1 code compiles with `cargo clippy -- -D warnings` and `cargo test`.
- [ ] Update `docs/DESIGN.md` with any deviations discovered during implementation.
- [ ] Update this plan: mark all tasks `[x]` and write a brief Phase 1 completion note.
