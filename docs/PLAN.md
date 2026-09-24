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
- [x] Title bar: traffic dots, `microsandbox vX.Y.Z` (runtime detection),
      session/size/pid, LIVE pill, clock.
- [x] Tab bar: `[1] SANDBOXES (n)  [2] LOGS  [3] PORTS` + runtime/daemon state
      on the right; number keys switch views.
- [x] Update banner (runtime `banner_text`) restyled as full-width warn band.
- [x] Footer keyhint bar shared by all views.

### 3. Dashboard
- [x] Fleet stats bar (n running / stopped, sum of memory from metrics).
- [x] Card grid: status pill, image chip, CPU gauge `[■■■□□…]`, memory,
      net I/O rates (computed from consecutive metric samples), ports, uptime
      from `created_at`.
- [ ] Status/event panel (real status + error + banner lines only).
- [ ] Right sidebar: embedded quick create (image picker + name, `Ctrl+A`
      opens the full form) + selected-sandbox log preview (bounded tail).

### 4. Logs
- [x] Toolbar rows: target, grep input, follow/source/clear pills.
- [x] Stream inspector header: sandbox name, image, matched/total, mem/cpu.
- [x] Line rendering: line numbers, timestamps, source badges, level colors,
      warn/err row tint.
- [x] Buffer metrics bar (lines, matched, scroll position).

### 5. Ports
- [x] Global host⇄guest matrix over all sandboxes (state, sandbox, bind,
      guest port, proto).
- [x] Inspector sidebar for the selected binding (real fields only).
- [x] Publish/unpublish wiring through the recreate flow with confirm dialog.

### 6. Create + Help polish
- [x] Create form restyled onto the new chrome/palette.
- [x] Help overlay updated (number-key view switching, pane focus).

### 7. Gates + docs
- [x] `cargo fmt --check && cargo clippy -- -D warnings && cargo test &&
      cargo check` green.
- [x] `docs/DESIGN.md` views section updated; deviations recorded.
- [ ] Manual smoke test note.

---

# Phase 2.7 — Design fidelity pass (MVP polish)

Goal: tighten the Stitch fidelity after the 2.6 rework — drop the mockup's
fake window chrome bits (traffic-light dots, clock), then close the largest
design gap: the dashboard right sidebar with an embedded quick-create panel
(image picker + name + launch, `Ctrl+A` for the full form) and a live
selected-sandbox log preview, exactly like the mockup's right column.

## Task List

### 1. Chrome trims
- [x] Remove the fake traffic-light dots from the title bar.
- [x] Remove the clock from the title bar.
- [x] Keep: name+version, session info, LIVE pill, tab bar, banner, footer.

### 2. Dashboard sidebar
- [x] Two-pane body on wide terminals (cards 65% / sidebar 35%), single-pane
      below ~100 columns.
- [x] Sidebar top: `⚡ QUICK CREATE MICROVM` panel — name field, image
      picker (reuses `CreateForm` state), `[Enter] Launch`, `Ctrl+A` opens
      the full advanced form; submit queues the same detached create.
- [x] Sidebar bottom: `STREAM: <selected sandbox>` — last lines of the
      selected sandbox's log tail (bounded), hint `l` for the full view.

### 3. Gates + push
- [x] All four gates green; push; smoke-test note.

**Phase 2.7 complete.** 148 unit tests, all gates green. Rendered structure
verified off-screen (TestBackend test) and on a real pty (render dump).
Fidelity fixes along the way: tab-rail UTF-8 panic (blank frame), double
borders in the zoned surface, framed chrome (title/tabs/body/footer inside
one window), continuous separators. The full TUI smoke test against a live
runtime (create/logs/ports round-trip) remains the maintainer's manual step.

---

# Phase 2.8 — Smoke-test findings (runtime correctness)

Findings from the first live smoke test (2026-09-22):

1. **No loading indicator** — after TUI start or create, the list stays
   empty until the first poller fires; nothing says "loading".
2. **`list_sandboxes` fails for TUI-created sandboxes** —
   `stored config … invalid type: null, expected a string at column 201`:
   the SDK stores `"workdir": null` for sandboxed created without a
   workdir, but our `RuntimeConfig.workdir` DTO is `String`. One broken
   row fails the whole list, so nothing is displayed.
3. **Publish-port from the ports view has no form** — `-` (unpublish)
   works via confirm, but `+` does nothing (documented deviation).

## Task List

### 1. Tolerant config deserialization (fixes the broken list)
- [x] `RuntimeConfig.workdir: Option<String>` (stored JSON uses `null`
      for "no workdir"; `entrypoint` stays `Option<Value>`).
- [x] Audit every strict (non-`default`, non-`Option`) field in
      `models.rs` against a real stored config; relax where the SDK may
      emit `null`/absent.
- [x] `list_sandboxes` degrades per-sandbox: a row whose config can't be
      parsed still appears (name, state, created_at, image `"?"`) with the
      parse error surfaced as `AppEvent::Error`, instead of failing the
      whole list.
- [x] Unit tests: a stored config with `null` workdir + `entrypoint: []`
      parses; the "Test2 regression" JSON shape is covered.

### 2. Loading indicators
- [x] `App` tracks initial-load state per poller (sandboxes/metrics);
      dashboard shows `Loading…` in the card area until the first list
      arrives.
- [x] After a create submit, the status line already shows
      `Creating <name>…`; also show it until the next list refresh.

### 3. Publish-port form on the ports view
- [x] `+` opens an inline bind form (target sandbox fixed to selection;
      host bind default `127.0.0.1`, host port, guest port, protocol
      toggle tcp/udp), `Enter` confirm → recreate flow with the existing
      confirm dialog, `Esc` cancel.
- [x] Footer hints updated (`[+] Publish`, `[-] Unpublish`).

### 4. Gates
- [x] All four gates green; push.

**Phase 2.8 complete.** 153 unit tests, all gates green. Fixes verified
against the real stored-config JSON captured from the user's database
(`sandbox-stored-null-workdir.json` fixture). Remaining manual step: re-run
the smoke test — the two existing 'Test'/'Test2' sandboxes should now list
after restart, and `+` on the ports view should publish through the
recreate flow.

Follow-up (2.8a) after user feedback "ports belong to a sandbox, not
global": publish/unpublish now target the SELECTED matrix row's sandbox
(the form title shows it), and the port cache refetches for ALL sandboxes
on view entry, `r`, and after every recreate op — bindings used to go to
the view's opening sandbox and the cache never refreshed.

---

# Phase 2.9 — Navigation redesign: sandbox rail + detail tabs

User verdict on 2.6–2.8: the global-tab model (Sandboxes/Logs/Ports as peer
views + matrix) is confusing; ports belong to ONE sandbox (`sbx <name>
publish port:port` mental model). New IA, agreed with the maintainer:

- **Left rail (always visible): the sandbox cards, stacked vertically.**
  ↑/↓ selects; the card IS the list (no extra "Sandboxes" tab).
- **Right detail pane with per-sandbox tabs**: `[1] OVERVIEW` (live
  metrics + this sandbox's ports), `[2] LOGS`, `[3] PORTS`
  (publish/unpublish of THIS sandbox, `sbx`-style), `[4] EXEC`.
- **Create becomes a modal on `c`** (image picker + name; `Ctrl+A`
  expands advanced fields inside the modal).
- **STREAM preview panel is removed** (no clear purpose).
- **EXEC** like Docker Sandbox: inline captured exec in the tab (current
  window) + suspend TUI and open an interactive shell in the foreground
  ("new window" equivalent, `msb ssh <name>` per DESIGN.md §7).

## Task List

### 1. App skeleton: rail + detail tabs
- [x] Replace the `View` dispatch with: chrome (title/footer) once, then
      body = rail (cards, vertical, scrollable) + divider + detail pane.
- [x] `App::detail: DetailTab` (Overview/Logs/Ports/Exec); keys `1-4`
      switch, `Tab` cycles, `↑↓` always moves the sandbox selection,
      `Enter` → Overview.
- [x] Remove the STREAM preview panel and the global Ports view/Inspect
      placeholder; LOGS/PORTS render chrome-less inside the detail pane,
      scoped to the selected sandbox.

### 2. EXEC tab
- [x] `ExecState`: input buffer + history (cmd → captured output) per
      session; `Enter` runs the parsed command via the SDK captured exec;
      output appends in-tab.
- [x] `S` suspends the TUI (leave alternate screen) and runs an
      interactive shell in the foreground terminal; TUI resumes on exit
      (DESIGN.md §7 behavior; documented architecture deviation: the
      interactive path shells out to `msb ssh`, the SDK's 0.7.2 exec has
      no TTY mode).
- [x] Unit tests for input parsing / history append.

### 3. Create modal on `c`
- [x] `c` opens the existing quick form (full-screen form retained after
      user feedback; the exec tab was removed instead — 'e' now opens the
      interactive shell window directly, per the maintainer's correction).

### 4. Polish + gates
- [x] Help overlay + footer hints updated to the new navigation.
- [x] All four gates green; push; smoke-test handoff.

**Phase 2.9 complete.** 155 unit tests, all gates green. Follow-up fixes
from the next smoke round: workdir on cards (Ports line removed), the
PORTS detail tab's keys were dead until the dispatch was rerouted, `e` =
interactive shell — refined to a PARALLEL window (maintainer): `e` spawns
a new terminal (konsole/gnome-terminal/alacritty/foot fallback chain)
with `msb ssh <name>` while the dashboard keeps running; EXEC tab removed
entirely; `s`/`S` = start.

**Deviations from the mockups (deliberate):**
- All invented telemetry is omitted: ENGINE/DAEMON strings, eBPF/cgroupv2/
  iptables messages, per-port traffic counters, security policies, buffer
  KB/lines-per-second stats — the 0.7.2 SDK exposes none of these.
- Glow effects (mockup box-shadows) are approximated with the accent border
  + bold; ratatui has no glow.
- The dashboard's right sidebar (embedded quick create + log preview) is
  **not** part of this phase — the tab bar navigates to the full views
  instead; the sidebar can be added later without breaking the chrome.
- Publish-port on the ports view is wired to the recreate flow via confirm,
  but there is no dedicated bind-new-port form yet (`+` currently does
  nothing; a form is future work).
- Level badges in logs are heuristic (keyword scan), since LogLine carries
  no structured level field.

---

# Phase 2.10 — sbx-style workspace: default workdir + mount-current-dir toggle

Docker `sbx` parity for workspaces, agreed with the maintainer:
- `Mount current dir` checkbox in the create form, **checked by default**
  (sbx mounts the current directory on every run).
- Checked → the TUI's CWD (captured at form open) is bind-mounted **at the
  same absolute path** inside the guest and becomes the default workdir; a
  deliberately typed Workdir overrides the auto value.
- Unchecked → workdir default is `/home/agent/workspace` (the Docker sbx
  template default for mountless sandboxes); no mount added.
- Dashboard cards show the mounted dirs (sbx `WORKSPACE` column): binds as
  `HOST⇄GUEST` (`⇢` when read-only), named volumes as `name:GUEST`, `—` when
  mountless.

## Task List

### 1. Create form
- [x] `CreateForm` gains `mount_cwd: bool` (default `true`) + `cwd: String`
      captured at form open; `toggle_mount_cwd()` keeps the workdir field
      coherent (restore mount path on re-check, drop auto-fill on un-check).
- [x] `FormField::MountCwd` toggle in QUICK (3 fields) + ALL (11 fields) Tab
      order; `Space` toggles, `Enter` advances; checkbox rendered with the
      exact mount path next to it.
- [x] `to_create_spec()`: prepends the `CWD:CWD` bind, sets workdir
      (explicit input > mount path > `/home/agent/workspace`), validates the
      captured CWD (absolute + still exists), quick-mode summary shows
      `no mount` only when deviating from the default.

### 2. Backend + models
- [x] `SandboxSummary.mounts: Vec<String>` (compact display tokens) +
      `mount_display()` (⇄/⇢ bind arrows, `name:GUEST`, guest-path fallback);
      `map_mounts()` in `sdk.rs` shared by SdkBackend + FakeBackend.
- [x] Unit tests: form state (5 new), mount display shapes, fixture-driven
      `map_mounts`, FakeBackend create→inspect→summary roundtrip
      (workdir verbatim, bind + named mounts, card display).

### 3. Docs + gates
- [x] `docs/DESIGN.md`: create-form mockup + workspace rules, card line
      (Ports → Workdir/Mounts).
- [x] All four gates green (166 unit tests).

**Phase 2.10 complete.** Deviation note: the sbx "workspace" concept maps to
a plain bind mount + workdir in microsandbox terms — no template/agent
machinery is implied; the checkbox only manipulates `CreateSpec.volumes` and
`CreateSpec.workdir`, keeping the SDK the single source of truth.

### 4. Smoke-test fix: mountless create failed (2026-09-23)
- [x] `✗ create failed: … could not validate workdir in guest
      "/home/agent/workspace": stat: No such file` — the SDK **stats** the
      workdir inside the guest rootfs at create time; `/home/agent/workspace`
      exists only in Docker sbx **template** images, not in plain OCI images.
      Fix: unchecking the mount removes the auto workdir entirely
      (`None` = image default, sbx `sbx run` behavior for mountless
      sandboxes); the `/home/agent/workspace` fallback const is removed.
      `toggle_mount_cwd` clears the auto-filled workdir on un-check and
      restores it on re-check; tests updated (166 green).

---

# Phase 2.11 — state-gated card actions (Docker-sbx parity)

Goal: the selected sandbox card shows its currently available actions in a
compact action row, like Docker sbx — Start only when stopped, Stop/Exec
only when running, Restart only running/stalled, Delete always. Keys follow
sbx: `s` toggles start/stop by state, `x` = E**x**ec, `r` = Restart,
`Del` = **Del**ete. Unavailable actions are not shown; pressing their key
sets an explanatory status line instead of opening a dialog.

## Task List

### 1. Action model (`src/actions.rs`)
- [x] `CardAction` enum (Start/Stop/Restart/Shell/Destroy) with
      `key_hint()` (sbx keys: s/s/r/x/Del), `label()` (Start/Stop/Restart/
      Exec/Delete), `highlight()` (the key substring inside the label).
- [x] `available_actions(&SandboxState)` — the availability matrix; Destroy
      always available; Start/Stop never co-occur (invariant tested).
- [x] `toggle_action(&SandboxState)` — which half of the `s` toggle applies
      (startable → Start, stoppable → Stop, else None).
- [x] Unit tests: per-state availability (7), keys/labels/highlight (2),
      toggle mapping + co-occurrence invariant (2).

### 2. Key gating (`src/app.rs`)
- [x] `s`/`S` = start/stop toggle via `toggle_action` (confirm dialog as
      before); `x` = exec/shell (running only); `r` = restart
      (running/stalled); `Del` = remove (always).
- [x] `confirm_gated` + `gated_state` helpers: unavailable keys set a
      status message (`'name' is <state> — cannot <verb>`), no dialog.
- [x] Unit tests: toggle stop states, exec gating, restart gating, destroy
      always, empty-list no-ops (6 updated/new).

### 3. Card action row (`src/ui/dashboard.rs`)
- [x] Selected card is one row taller (10 rows vs 9) and ends in the action
      row: available action words with the key substring highlighted
      (bold + role color), e.g. `Stop  Restart  Exec  Delete`.
- [x] `rail_card_heights` / `rail_visible_slice` variable-height rail
      layout (also fixes the pre-existing Uptime-row clipping at height 8).
- [x] Footer slimmed to global keys only (`[c] Create [Tab] Next tab [?]
      Help [q] Quit`) — state-dependent hints moved onto the card.
- [x] Unit tests: heights, rail slice, action-line content/width fits rail
      (42 cols), offscreen render shows the row (7 new).

### 4. Docs + gates
- [x] `docs/DESIGN.md`: keybindings table + dashboard section updated
      (sbx keys, action-row behavior).
- [x] Help overlay updated to the new key map.
- [x] All four gates green (187 unit tests).

### 5. Smoke-test fix: publish flow failed after recreate (2026-09-24)
- [x] `✗ Publish 127.0.0.1:8888→80/tcp on 'Test' — failed: start Test
      after recreate: start Test: sandbox still running` — root cause:
      0.7.2 `create_detached` boots the VM and waits for "sandbox
      ready"; the recreated sandbox is **Running** when create returns.
      The explicit `start` afterwards was redundant and tripped
      `SandboxStillRunning`. Fix: `recreate_with_ports` no longer starts
      after create. FakeBackend now models both SDK invariants (start on
      Running errors; create leaves Running) so the sequence is
      regression-tested (189 green). Note: the port was actually
      published — the VM was live with the listener bound; only the
      status message was wrong.

### 6. Smoke-test fix: status never cleared; ports never shown (2026-09-24)
- [x] `published 8888:80 on Test` stayed on the status line forever:
      `OpDone`/`Error` had no expiry. Fix: status/error messages are
      transient — tick-aged with `STATUS_TTL_TICKS` (20 ticks ≈ 5 s),
      `set_status()` restarts the TTL; the busy spinner is exempt.
- [x] Ports were never displayed (even after TUI restart): the fetch
      trigger checked `app.view == View::Ports` — dead pre-2.9 state
      (ports are a detail TAB since 2.9), so the trigger could never
      fire. Fix: unit-tested `ports_fetch_needed()` policy — fetch when
      the cache is dirty (ports-tab entry, `r`, publish/unpublish
      recreate) or never primed (initial fetch right after the first
      list, so cards show ports immediately).
- [x] Live smoke test `publish_live_smoke` (real SDK, `#[ignore]`d):
      create → Running → publish recreate → still Running, port in the
      persisted config.

### 7. Ports-tab UX + data-preserving publish flow (2026-09-24)
- [x] Ports-tab navigation was broken: `render_matrix` read the
      highlighted row from a stub that always returned 0, so the
      highlight froze on the first row and `↑↓` seemed dead (you could
      only unpublish the topmost binding). Fix: `selected` is passed
      through to `render_matrix`; the `[↑↓] select binding` hint shows
      in the toolbar. The `r` refresh key was dropped (ports now refresh
      via the fetch policy: on entry, after recreate ops).
- [x] Publish/unpublish no longer resets the rootfs. The recreate flow
      snapshots the sandbox disk first, then stop → remove →
      `Sandbox::restore(snapshot)` with the new port set → reapply
      env/workdir/labels via the modification API (the 0.7.2
      RestoreBuilder has no setters for those) → remove the temporary
      snapshot. The sandbox's DATA IS PRESERVED (like `restart`, only
      the processes restart). On restore failure the snapshot is kept
      for manual recovery (`msb restore`).
- [x] Backend: `snapshot_disk` / `restore_with_ports` / `remove_snapshot` /
      `reapply_config` on `MsbBackend` (+ FakeBackend modeling, incl.
      restore-failure injection). Note: grouped snapshot members are
      immutable — `snapshot_disk` removes the deterministic member
      (`<name>:tui-pre-publish`) before creating it; builder `.force()`
      is rejected for installed groups (local SDK).
- [x] Live smoke test extended: canary file written into the rootfs
      must SURVIVE the publish (verified: `data-preserved`); no snapshot
      litter after the flow.
