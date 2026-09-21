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
- [x] Add a `FakeBackend` (in-memory) for unit tests of the action layer.
- [x] Write unit tests for CLI command vector generation and JSON parsing.

### 3. High-level actions
- [x] Add `src/actions.rs` with user-facing operations:
  - `create_sandbox`, `stop_sandbox`, `start_sandbox`, `restart_sandbox`, `remove_sandbox`
  - `exec_sandbox`, `stream_logs`
  - `publish_port`, `unpublish_port` (stop → remove → recreate → start with warning)
- [x] Unit-test the recreate flow against `FakeBackend`.

### 4. App state + event loop
- [x] Add `src/app.rs` with `App` struct holding current view, selected sandbox index, poller handles, last error, and pending action state.
- [x] Set up `tokio::select!` event loop in `src/event.rs` (or `app.rs`) combining crossterm events, metric poll ticks, and list poll ticks.
- [x] Render a basic frame with status/error line.

### 5. Dashboard UI
- [x] Implement `src/ui/dashboard.rs`: responsive card grid showing name, state, image, CPU%, memory, net I/O, ports, uptime.
- [x] Bind keys: `↑↓` select, `Enter` inspect, `c` create, `e` exec, `l` logs, `p` ports, `r` restart, `x` stop, `Del` remove (confirm), `q` quit.

### 6. Create-sandbox form
- [x] Implement `src/ui/create.rs` with fields for image, name, CPUs, memory, workdir, ports, volumes, env vars, network profile.
- [x] Image autocomplete from `list_images`.
- [x] On submit, call `actions::create_sandbox` and return to dashboard with spinner/status.

### 7. Logs panel
- [x] Implement `src/ui/logs.rs`: stream `msb logs -f --json` in a long-lived task, render with timestamps.
- [x] Bind `f` follow toggle, `Esc` back.

### 8. Ports view (read-only Phase 1)
- [x] Implement `src/ui/ports.rs`: show published ports from `inspect.network.ports`.
- [x] Bind `+` / `-` only if backend supports recreation; otherwise show message "Publish/unpublish requires recreate in msb 0.7.2".

### 9. Help overlay
- [x] Implement `src/ui/help.rs`: context-aware keybinding overlay shown with `?`.

### 10. Main wiring
- [x] Wire everything in `src/main.rs`: parse CLI, verify `msb` on PATH, init terminal, run event loop, restore terminal on exit.
- [x] Add `cargo test` and `cargo run -- --help` smoke tests in CI (`.github/workflows/ci.yml`).

### 11. Final Phase 1 polish
- [x] Ensure all Phase 1 code compiles with `cargo clippy -- -D warnings` and `cargo test`.
- [x] Update `docs/DESIGN.md` with any deviations discovered during implementation.
- [x] Update this plan: mark all tasks `[x]` and write a brief Phase 1 completion note.

**Phase 1 complete.** 136 unit tests, all gates green. Deviations recorded:
- msb 0.7.2 has no `create --replace`; recreate = stop → rm → create → start (updated in AGENTS.md/DESIGN.md).
- Exec in Phase 1 runs a demo command via the confirm dialog; free-form exec input is Phase 2.
- Log tail task is spawned when the Logs view opens and aborted on leave (one sandbox at a time).

---

# Phase 2 — Full migration from msb CLI backend to the microsandbox Rust SDK

Goal: replace the `msb` CLI/JSON layer with the typed `microsandbox` 0.7.2 SDK
(`MsbBackend` seam stays; `SdkBackend` maps SDK types → `models.rs` DTOs). Add
runtime install/version management. Keep `FakeBackend` for unit tests.

Verified SDK facts (from crates.io 0.7.2 sources, 2026-09-20):
- SDK sandboxes default to ATTACHED mode: a plain `.create()`/`start()` ties the
  VM to the TUI process. All create/start/restart paths must use
  `detached(true)` / `start_detached()` / `RestartOptions { detached: true, .. }`.
- `SandboxModificationBuilder` (0.7.2) has NO port methods — publish/unpublish
  keeps the recreate flow. `modify()` covers cpus/memory/env/labels/workdir/secrets.
- Logs: `handle.follow_logs(&LogOptions)` gives a replay+follow stream with
  opaque resume cursors — no poll-and-diff loop needed.
- Fleet metrics: `all_sandbox_metrics_reports_local(&local, include_exited)`
  (running-only default: `all_sandbox_metrics()`); needs the `LocalBackend` handle.
- Runtime version: `setup::resolve_runtime_version(path)` reads the version
  embedded in an `msb` binary without executing it; SDK version is
  `microsandbox_utils::PREBUILT_VERSION`. Install/update via
  `setup::install_runtime(&GlobalConfig, InstallOptions{..})`.
- Network profiles: `NetworkPolicy::from_profiles([NetworkProfile::Public])`
  et al. (crates.io 0.7.2 has no `prebuilt` feature; use `download-binaries`).

## Task List

### 1. Plan + probe removal
- [x] Add this Phase 2 section to `docs/PLAN.md`.
- [x] Delete `src/api_probe.rs` (throwaway SDK probe) and its `mod` in `main.rs`.

### 2. SdkBackend behind the MsbBackend trait
- [x] Add `src/backend/sdk.rs`: implement every `MsbBackend` method with the SDK.
- [x] Create/start/restart use detached mode (persistence independent of TUI).
- [x] Map SDK types → `models.rs` DTOs (anti-corruption layer stays).
- [x] Unit-test the testable parts: CreateSpec → SDK builder mapping, DTO
      conversions (config→summary/inspect, metrics report→Metrics, ports).
- [ ] Live behavior verified by manual smoke test (not in CI).

### 3. Runtime management (install / update / version banner)
- [x] Detect installed `msb` version (PATH + `~/.microsandbox/bin`) and compare
      against the SDK version (`PREBUILT_VERSION`).
- [x] Dashboard banner: installed < SDK → offer update; installed > SDK → hint.
- [x] "Install/Update microsandbox" action via `setup::install_runtime`,
      non-blocking with spinner/status (tokio task).

### 4. Switch main.rs to SdkBackend; retire CliBackend
- [x] `main.rs` constructs `SdkBackend` (with `LocalBackend`) as the default.
- [x] Remove `CliBackend`, its fixtures-based CLI tests, and the `which` dep if
      unused (full-switch decision; FakeBackend keeps action-layer tests green).
- [x] Logs view: switch the tail task to `follow_logs` streams.
- [x] Keep recreate flow for publish/unpublish (no port modify in 0.7.2).

### 5. Docs + final polish
- [x] Rewrite the "Backend" section of `docs/DESIGN.md` (CLI → SDK rationale:
      typed API, version management, richer exec/logs/fs APIs; install policy;
      detached-mode note; honest removal of the CLI-JSON rationale).
- [x] Update file-structure section; mark plan tasks `[x]`.
- [x] All gates green: `cargo fmt --check && cargo clippy -- -D warnings &&
      cargo test && cargo check`, run locally. (The GitHub Actions workflow was
      removed at the maintainer's request; CI no longer runs.)

**Phase 2 complete** (pending the manual smoke test of task 2 against a live
runtime). 145 unit tests, all gates green locally. Deviations recorded:
- Full SDK switch, no CLI fallback: `CliBackend` deleted entirely; missing
  `msb` no longer exits at startup — the dashboard banner offers install (`U`).
- `SandboxModificationBuilder` 0.7.2 has no port methods, so publish/unpublish
  and network-rule edits keep the stop → remove → recreate → start flow.
- `CreateSpec::to_args` (CLI argument builder) removed with the CLI layer.
- GitHub Actions workflow removed at maintainer request; gates are local-only.

---

# Phase 2.5 — UX polish (findings from the manual smoke test)

The first live smoke test surfaced correctness and UX issues. Fixes and
redesigns are tracked here.

## Task List

### 1. Smoke-test correctness fixes
- [x] `q` never exited: tokio runtime drop waits on the SDK's parked
      blocking threads (sqlx/SQLite workers). `main` now shuts the runtime
      down without draining them (commit 9587aac).
- [x] Opaque base background behind every view + overlays (translucent
      fish/terminal themes bled through `Color::Reset`).

### 2. Central dark theme
- [x] Add `src/ui/theme.rs`: one polished dark palette (opaque `bg`, three
      text tones, accent, semantic `ok`/`warn`/`err`); sweep dashboard,
      logs, ports, help, and shared widgets onto it. `create.rs` follows
      with task 3.
- [x] Help overlay text corrected: `s` = start (not SSH).

### 3. Create form: quick + advanced + image picker
- [x] Quick path (default): image picker + optional name; SDK defaults for
      everything else (cpus/memory, `public` network, no ports/volumes/env).
- [x] Image picker: list is always visible (pulled images first, then
      curated suggestions), type-to-filter, `↑↓`+`Enter` to select; typed
      custom references pass through.
- [x] `Ctrl+A` toggles advanced fields (cpus, memory, workdir, ports,
      volumes, env, network profile) prefilled empty = runtime defaults;
      a one-line summary keeps them visible from quick mode.
- [x] Theming + footer hints for the new form; unit tests for state logic.

### 4. Re-run smoke test
- [ ] Dashboard render with theme, create flow end-to-end, logs, exec,
      stop, remove; `q` exits cleanly; `msb ls` shows no leftovers.

---

# Phase 2.6 — Stitch design rework (`design/stitch_microsandbox_tui_design_system/`)

Goal: restyle the TUI after the Stitch mockups — neon-cyberpunk accents on a
GitHub-Dark base: title bar with LIVE/clock, tab bar, full-width update banner,
dashboard card grid with quick-create sidebar, logs toolbar + stream inspector,
ports matrix. **Invented mockup telemetry is deliberately not rendered**
(firecracker/eBPF/cgroupv2/iptables strings, per-port traffic counters, security
policies): only data the SDK actually provides is shown, per the architecture
rules.

## Task List

### 1. Palette
- [x] Swap `THEME` in `src/ui/theme.rs` to the Stitch palette: bg `#0d1117`,
      fg `#f0f6fc`, text `#c9d1d9`, muted `#6e7681`, accent `#00f0ff`,
      ok `#00ff88`, warn `#ffe600`, err `#ff2a6d`.
- [x] Add `border` (`#30363d`), `panel` (`#161b22`), `selection` (`#0f2937`)
      roles for cards, tables, and highlighted rows.
- [x] Update the Theme section of `docs/DESIGN.md`.

### 2. Shared chrome (`src/ui/chrome.rs`)
- [ ] Title bar: traffic dots, `microsandbox vX.Y.Z` (runtime detection),
      session/size/pid, LIVE pill, clock.
- [ ] Tab bar: `[1] SANDBOXES (n)  [2] LOGS  [3] PORTS` + runtime/daemon state
      on the right; number keys switch views.
- [ ] Update banner (runtime `banner_text`) restyled as full-width warn band.
- [ ] Footer keyhint bar shared by all views.

### 3. Dashboard
- [ ] Fleet stats bar (n running / stopped, sum of memory from metrics).
- [ ] Card grid: status pill, image chip, CPU gauge `[■■■□□…]`, memory,
      net I/O rates (computed from consecutive metric samples), ports, uptime
      from `created_at`.
- [ ] Status/event panel (real status + error + banner lines only).
- [ ] Right sidebar: embedded quick create (image picker + name, `Ctrl+A`
      opens the full form) + selected-sandbox log preview (bounded tail).

### 4. Logs
- [ ] Toolbar rows: target, grep input, follow/source/clear pills.
- [ ] Stream inspector header: sandbox name, image, matched/total, mem/cpu.
- [ ] Line rendering: line numbers, timestamps, source badges, level colors,
      warn/err row tint.
- [ ] Buffer metrics bar (lines, matched, scroll position).

### 5. Ports
- [ ] Global host⇄guest matrix over all sandboxes (state, sandbox, bind,
      guest port, proto).
- [ ] Inspector sidebar for the selected binding (real fields only).
- [ ] Publish/unpublish wiring through the recreate flow with confirm dialog.

### 6. Create + Help polish
- [ ] Create form restyled onto the new chrome/palette.
- [ ] Help overlay updated (number-key view switching, pane focus).

### 7. Gates + docs
- [ ] `cargo fmt --check && cargo clippy -- -D warnings && cargo test &&
      cargo check` green.
- [ ] `docs/DESIGN.md` views section updated; deviations recorded.
- [ ] Manual smoke test note.
