# microsandbox-tui — Design Document

A Docker `sbx`-style terminal dashboard for [microsandbox](https://github.com/superradcompany/microsandbox).

## Goal

Give users a fast, visual way to create, manage, and interact with microsandbox
microVMs — without memorizing CLI flags. Modeled after Docker's `sbx` TUI: sandbox
cards with live metrics, quick create flow, port forwarding, network rules, and
one-key access to exec/logs/ssh.

## Tech Stack

- **Rust 2024 edition** — matches microsandbox's ecosystem, compiles to a single binary
- **ratatui** + **crossterm** — TUI framework + terminal backend
- **tokio** — async runtime for SDK calls, pollers, and background tasks
- **serde** / **serde_json** — serde derives for the `models.rs` DTO layer and
  fixture-based unit tests
- **clap** — CLI args for the TUI itself (e.g. `msb-tui`, `msb-tui create`)

### Backend: microsandbox Rust SDK (Phase 2)

The TUI talks to microsandbox exclusively through the typed `microsandbox`
0.7.2 SDK (`SdkBackend` wrapping `LocalBackend`). The earlier `msb`
CLI/JSON approach (spawn `msb`, parse `--format json`) was replaced because:

1. **Typed API** — no stringly-typed JSON parsing at runtime; SDK structs are
   checked at compile time and the `models.rs` DTO mapping is unit-tested.
2. **Runtime management** — the SDK ships `setup::install_runtime` and the
   `PREBUILT_VERSION` constant, which power the dashboard's version banner and
   the in-TUI install/update action. A CLI-driven TUI can only tell the user to
   run a shell command.
3. **Richer APIs** — `follow_logs` streams with resume cursors (replay +
   follow in one call), captured non-interactive exec, and direct metric
   reports; all of these would be poll-and-diff hacks over CLI JSON.
4. **One source of truth** — the SDK and the installed `msb` runtime are
   version-locked; the banner warns on drift instead of silently mis-parsing.

Trade-offs accepted: the binary now links `libkrun`-adjacent crates (needs
`libcap-ng` at build time) and is bound to the 0.7.2 SDK API surface.

### Detached mode (policy)

The SDK defaults to **attached** sandboxes: a plain `create()`/`start()` ties
the VM's lifetime to the calling process. The TUI must never do this — all
create/start/restart paths run detached (`detached(true)`,
`start_detached()`, `RestartOptions { detached: true, .. }`) so sandboxes
survive the TUI being closed.

### Prerequisites

- None at startup. If no `msb` runtime is detected, the dashboard shows a
  banner and the `U` key opens a confirmation dialog that installs/updates the
  runtime in the background (`setup::install_runtime`).
- KVM enabled (Linux), Apple Silicon (macOS), or WHP (Windows) — for actually
  running sandboxes.

## Theme

One polished dark palette lives in `src/ui/theme.rs` (`ui::theme::THEME`) and
is the single source of color for every view, derived from the Stitch design
system (`design/stitch_microsandbox_tui_design_system/`):

- **Opaque base background** (`#0d1117`, GitHub-Dark) — painted behind every
  frame; translucent terminal profiles never bleed through, on any platform.
- **Three text tones** (`fg` `#f0f6fc` > `text` `#c9d1d9` > `muted` `#6e7681`)
  carry the whole hierarchy.
- **One accent** (neon cyan `#00f0ff`) reserved for selection, focus, and
  interactive hints.
- **Semantic colors** only where state has meaning: `ok` `#00ff88` (running),
  `warn` `#ffe600` (banners), `err` `#ff2a6d` (errors, destructive).
- **Structural tones**: `border` `#30363d` (card/panel borders), `panel`
  `#161b22` (raised panel background), `selection` `#0f2937` (row/card
  highlight).

Additional palettes (light, terminal-palette "system") can be added as more
`Theme` statics later; nothing else changes when they do.

## Views

The Stitch design system (`design/stitch_microsandbox_tui_design_system/`)
drives the visual language: shared chrome bands on every full-screen view
(`src/ui/chrome.rs`) — title bar (traffic dots, `microsandbox vX.Y.Z`,
session info, clock), tab bar with filled-pill active tab
(`[1] SANDBOXES  [2] LOGS  [3] PORTS`, number keys switch), full-width warn
banner for runtime updates, and a footer keyhint bar. Mockup-only telemetry
(firecracker/eBPF/cgroupv2/iptables strings, per-port traffic counters,
security policies) is deliberately **not** rendered — the 0.7.2 SDK provides
none of it; panels show only real backend data.

### 1. Dashboard (default view)

```
┌─ microsandbox-tui ────────────────────────────────────────────────────┐
│  Sandboxes (3)                                  [c] Create  [q] Quit  │
│┌─────────────────────┐ ┌─────────────────────┐ ┌─────────────────────┐│
││ ● my-app     running│ │ ○ devbox     stopped│ │ ● worker     running││
││ python:3.12         │ │ ubuntu              │ │ alpine              ││
││ CPU 12%  MEM 240M   │ │ CPU --   MEM --     │ │ CPU 3%   MEM 56M    ││
││ Net ↓0  ↑1.2K       │ │                     │ │ Net ↓0   ↑0         ││
││ Workdir /app        │ │                     │ │ Workdir /           ││
││ Mounts /a⇄/a —      │ │                     │ │ Mounts —            ││
││ Uptime 5m 32s       │ │                     │ │ Uptime 12s          ││
│└─────────────────────┘ └─────────────────────┘ └─────────────────────┘│
│  [c] Create  [Tab] Next tab  [?] Help  [q] Quit                       │
│  Navigation: Tab / ↑↓←→                                               │
└───────────────────────────────────────────────────────────────────────┘
```

- Sandbox cards laid out in the left rail (stacked vertically, scrollable)
- Each card: name, image chip, right-aligned uppercase status pill
  (● RUNNING / ○ STOPPED / ⏸ PAUSED / ✗ EXITED, colored per state), live CPU%
  with a `[■■■□□□□□□□]` gauge, memory used / limit with percent, net I/O with
  ↓/↑ arrows, workdir, mounted dirs (`HOST⇄GUEST` for writable binds — the
  sbx workspace shows as `PATH⇄PATH`; `name:GUEST` for named volumes; `—`
  when mountless), uptime
- The selected card is one row taller and ends in a state-gated action row
  showing only the currently available actions as words with the key letter
  highlighted: e.g. `Stop  Restart  Exec  Delete` for a running sandbox,
  `Start  Delete` for a stopped one (Docker-sbx parity)
- Selected card highlighted with the accent border; actions operate on selection
- Polls the SDK sandbox list every 5s
- Polls fleet metric reports every 1s for live stats

### 2. Create Sandbox form

```
┌─ Create Sandbox ──────────────────────────────────────────────────────┐
│                                                                       │
│  Image:   [python___________]  (autocomplete from image list)         │
│  Name:    [my-sandbox_______]  (auto-generated if empty)              │
│  [x] Mount current dir  → /home/user/proj (space toggles)             │
│  CPUs:    [1___]   Memory:    [512M____]                              │
│  Workdir: [/home/user/proj___]                                        │
│                                                                       │
│  ── Network ──                                                        │
│  Profile: ( ) public  (•) private  ( ) host  ( ) none  ( ) custom     │
│  Ports:   [8080:80___________]  [+ Add]  [- Remove]                   │
│           [0.0.0.0:9090:90_____]                                      │
│  Rules:   [allow@api.example.com___]  [+ Add]                         │
│           (empty = use profile defaults)                              │
│                                                                       │
│  ── Volumes ──                                                        │
│  [./src:/app_______________]  [+ Add]                                 │
│  [mydata:/data______________]                                         │
│                                                                       │
│  ── Environment ──                                                    │
│  [DEBUG=true_______________]  [+ Add]                                 │
│                                                                       │
│  [Tab] next field  [Space] toggle mount  [Enter] create  [Esc] cancel │
└────────────────────────────────────────────────────────────────────────┘
```

- **Workspace mount (Docker-sbx parity)**: the `Mount current dir` checkbox
  defaults to **checked** and bind-mounts the TUI's current working directory
  into the sandbox **at the same absolute path** (sbx mounts workspaces at
  their host path so stack traces line up). While checked it also sets the
  default workdir to that path; a deliberately typed Workdir overrides it.
  **Unchecked → no explicit workdir**: the SDK validates the workdir with
  `stat` inside the guest rootfs at create time, and plain OCI images don't
  contain `/home/agent/workspace` (it is baked into sbx template images
  only) — mountless sandboxes start in the image's own working directory,
  exactly like `sbx run` without a workspace. Validation: the captured CWD
  must be absolute and still exist at submit time.
- **Templates first**: `c` opens the template picker (master-detail, Spec
  `docs/superpowers/specs/2026-09-25-templates-create-rework-design.md`):
  left the template list (user files shadow built-ins by id), right the
  effective values with `(default)` markers. `Enter` creates directly
  (name collisions get `-2`, `-3` … suffixes), `e` opens the form
  pre-filled, `n` an empty form, `d` deletes a user template file
  (built-ins are only shadowable). Template files live in
  `~/.config/microsandbox-tui/templates/*.toml` with `[meta]`+`[spec]`
  sections; the built-ins (`opencode`, `opencode2`, `shell`) are embedded
  examples of the format. `rootfs = "snapshot:…"` is reserved for future
  golden-image sources and has no effect yet.
- **Grouped full-screen form** (via `e`/`n`): all fields visible in five
  sections — BASIS (image picker, name), RESSOURCEN (cpus/memory),
  MOUNTS (mount toggle, workdir, volumes), NETZWERK (profile, ports,
  rules), SONSTIGES (env, labels). Empty resource/list fields mean
  "runtime default". `Ctrl+S` saves the current form as a template
  (name + description dialog; overwriting an existing template file asks
  for confirmation).
- Named by default (persistent) — ephemeral is an advanced toggle
- Image field: an always-visible picker lists pulled images first, then
  curated suggestions; typing filters, `↑↓`+`Enter` adopts
- **Enter creates from anywhere in the form** (Docker-sbx convention; the
  image picker's `Enter` adopts instead). List fields commit typed items
  and navigate with plain `Enter` (no accidental create mid-list) —
  `Ctrl+Enter` creates from anywhere. `Tab`/`Shift+Tab` navigate fields
- Advanced fields prefilled **empty** = use the runtime default
- Port entries use Docker syntax: `HOST:GUEST` or `BIND:HOST:GUEST`, with `/udp` suffix
- Network profile presets map to SDK network profiles:
  - **public** → `NetworkProfile::Public` (default: internet allowed, private blocked)
  - **private** → `NetworkProfile::Private` (LAN/internal only)
  - **host** → host access profile
  - **none** → networking disabled
  - **custom** → user-defined network rule list + default policy
- On confirm: builds a detached sandbox via `SandboxBuilder` (idle after create)
- Shows progress while pulling image (first pull can take time)

### 3. Port Forwards view

The Stitch "HOST ⇄ GUEST PORT FORWARDING MATRIX": a global table over **all**
sandboxes (state, sandbox, host bind, guest port, proto) flattened from the
lazily refreshed `inspect` port cache, with a selected-binding inspector
sidebar on the right.

```
┌─ ■ HOST ⇄ GUEST PORT MATRIX ──────────────────────────────────────────┐
│  STATE      SANDBOX       HOST BIND      GUEST PORT                  │
│  ● ACTIVE   my-app        127.0.0.1:8080 80/tcp                      │
│  ● ACTIVE   devbox        0.0.0.0:9090   90/udp                      │
│  ✗ EXITED   worker        127.0.0.1:5050 50/tcp                      │
└───────────────────────────────────────────────────────────────────────┘
  ⚡ SELECTED BINDING            [+] Publish  [-] Unpublish  [Esc] back
  Sandbox: my-app
  Bind:     127.0.0.1:8080
  Guest:    80/tcp
  Recreate: required on edit
```

- `↑↓` moves the binding selection across sandboxes
- **Unpublish** (`-`): confirm dialog, then the recreate flow (see below)
- **Publish**: recreate flow with confirm (see "Port Publish/Unpublish —
  Implementation Detail" below; a dedicated bind-new-port form is future work)
  - ⚠️ Warns that recreation is required (microsandbox 0.7.2 can't modify ports live).
    Volume data survives; rootfs state resets unless snapshotted.
- Future: if the SDK gains live port modification, switch to live modify

### 4. Network Rules editor

```
┌─ Network Rules: my-app ───────────────────────────────────────────────┐
│                                                                       │
│  Default egress:  (•) allow  ( ) deny                                 │
│  Default ingress: (•) allow  ( ) deny                                 │
│                                                                       │
│  Rules (evaluated first, before profile defaults):                    │
│  #  Action  Direction  Target              Proto  Ports               │
│  1  allow   egress      api.example.com    tcp    443                 │
│  2  deny    egress      *.ads.example.com  any    any                 │
│  3  allow   ingress     private            tcp    22                  │
│                                                                       │
│  [+] Add rule  [e] edit  [-] remove  [Enter] apply  [Esc] cancel      │
└────────────────────────────────────────────────────────────────────────┘
```

- Visual editor for network rule strings
- Fields: action (allow/deny), direction (egress/ingress/any), target (domain/IP/
  CIDR/suffix/group), protocol (tcp/udp/icmpv4/icmpv6/any), ports (single/range/any)
- Apply uses the same recreate flow as port changes (network rules also require
  boot-time configuration in 0.7.2)

### 5. Volumes view

```
┌─ Volumes ─────────────────────────────────────────────────────────────┐
│                                                                       │
│  NAME        KIND  SIZE     USED      CREATED                          │
│  my-data     dir   —        0         2026-09-20 08:42                │
│  docker-data disk  10G      2.3G      2026-09-19 14:10                │
│                                                                       │
│  [+] Create  [r] remove  [Enter] inspect  [Esc] back                  │
└────────────────────────────────────────────────────────────────────────┘
```

- Lists volumes from the SDK volume list
- Create: name, kind (dir/disk), size
- Remove: volume removal via the SDK
- Shows which sandboxes mount each volume (cross-referenced from the sandbox
  list + inspect)

### 6. Logs panel

```
┌─ Logs: my-app (following) ────────────────────────────────────────────┐
│ [2026-09-20 08:43:01] Starting development server at 0.0.0.0:80      │
│ [2026-09-20 08:43:02] * Running on http://0.0.0.0:80/                 │
│ [2026-09-20 08:43:15] 127.0.0.1 - "GET / HTTP/1.1" 200 -              │
│ ...                                                                   │
│ [f] follow  [g] grep  [t] tail  [s] source  [Esc] back               │
└────────────────────────────────────────────────────────────────────────┘
```

- Streams logs via the SDK's `follow_logs` (replay + follow with resume
  cursor) and renders lines with timestamps
- Toggle follow mode, grep filter, tail count, source filter (stdout/stderr/system)

### 7. Exec / SSH

- **Exec**: pops a command input at the bottom, runs the command via the SDK's
  captured exec API, shows output inline. For interactive commands, suspends
  the TUI and runs the command in foreground (like `docker exec -it`).
- **SSH**: suspends the TUI and runs `msb ssh <name>` in foreground for a full
  interactive shell.

### 8. Snapshots view

```
┌─ Snapshots: my-app ───────────────────────────────────────────────────┐
│                                                                       │
│  GROUP      MEMBER     TYPE      CREATED         SIZE                  │
│  my-app     ready      full      2026-09-20      512M                 │
│  my-app     v2         disk      2026-09-19      128M                 │
│                                                                       │
│  [c] create  [r] restore  [x] remove  [Esc] back                     │
└────────────────────────────────────────────────────────────────────────┘
```

- Create: `snapshot create` from the sandbox via the SDK
- Restore: restore a snapshot group/member as a new sandbox
- Remove: snapshot removal via the SDK

## App State & Event Loop

```
┌─────────────┐     ┌──────────────────┐     ┌────────────────┐
│  Input      │────▶│  App State       │◀────│  Background     │
│  (crossterm) │     │  (current view,  │     │  Pollers        │
└─────────────┘     │   selected sbx,  │     │  (sandbox list, │
                    │   form state)    │     │   metrics)      │
                    └────────┬─────────┘     └────────────────┘
                             │
                    ┌────────▼─────────┐
                    │  Render          │
                    │  (ratatui)       │
                    └──────────────────┘
```

- **Event loop**: `tokio::select!` over crossterm input events + metric poll ticks
- **Pollers**: 
  - Sandbox list: every 5s (SDK list via `SdkBackend`)
  - Metrics: every 1s (SDK fleet metric reports)
  - Volumes/images: on-demand when entering those views
- **Actions**: spawn on tokio tasks calling `SdkBackend` methods (long-running
  SDK calls never block the UI), update state, re-render
- **Logs stream**: SDK `follow_logs` stream (replay + follow), lines sent over
  a `tokio::sync::mpsc` channel to the logs view

## Data Models

```rust
// Mapped from the SDK sandbox list
struct SandboxSummary {
    name: String,
    image: String,
    status: String,        // "Running", "Stopped", "Paused", "Exited"
    created_at: String,
}

// Mapped from the SDK inspect config (active_config subset)
struct SandboxConfig {
    name: String,
    image: ImageConfig,
    resources: Resources,
    network: NetworkConfig,
    mounts: Vec<Mount>,
    env: Vec<EnvVar>,
    labels: HashMap<String, String>,
    lifecycle: Lifecycle,
    runtime: RuntimeConfig,
}

struct NetworkConfig {
    enabled: bool,
    ports: Vec<PublishedPort>,
    strict: bool,
}

struct PublishedPort {
    host_bind: String,     // "127.0.0.1" or "0.0.0.0"
    host_port: u16,
    guest_port: u16,
    protocol: String,      // "tcp" or "udp"
}

struct Resources {
    cpus: u32,
    max_cpus: u32,
    memory_mib: u32,
    max_memory_mib: u32,
}

// Mapped from SDK metric reports
struct Metrics {
    name: String,
    state: String,
    cpu_percent: f64,
    cpus: u32,
    memory_bytes: u64,
    memory_limit_bytes: u64,
    net_rx_bytes: u64,
    net_tx_bytes: u64,
    disk_read_bytes: u64,
    disk_write_bytes: u64,
    uptime_secs: f64,
}

// Mapped from the SDK volume list
struct Volume {
    name: String,
    kind: String,           // "dir" or "disk"
    capacity_bytes: Option<u64>,
    used_bytes: Option<u64>,
    created_at: String,
}

// Mapped from the SDK image list
struct Image {
    reference: String,
    digest: String,
    size_bytes: u64,
    architecture: String,
    os: String,
}
```

## Keybindings

| Key | Context | Action |
|-----|---------|--------|
| `q` | global | quit |
| `1` / `2` / `3` | global | detail tabs: overview / logs / ports |
| `?` | global | help overlay |
| `↑↓` | dashboard | select sandbox card |
| `Enter` | dashboard | inspect selected sandbox |
| `c` | dashboard | create flow: template picker (master-detail) |
| `Enter` | template picker | create sandbox from the selected template |
| `e` | template picker | open the grouped form pre-filled from the template |
| `n` | template picker | open the grouped form empty |
| `d` | template picker | delete user template file (confirm; built-ins shadow-only) |
| `Ctrl+S` | create form | save current form as template (confirm on overwrite) |
| `x` | dashboard | exec: interactive shell (running sandboxes only) |
| `s` | dashboard | start/stop toggle for the selected sandbox (state-dependent) |
| `r` | dashboard | restart sandbox (running/stalled) |
| `Del` | dashboard | remove sandbox (confirm) |
| `U` | dashboard | install/update runtime (confirm) |
| `f` | logs | toggle follow |
| `g` / `/` | logs | grep filter |
| `s` | logs | cycle source filter |
| `↑↓` / `PgUp`/`PgDn` | logs | scroll buffer |
| `↑↓` | ports | select binding in matrix |
| `-` | ports | unpublish selected binding (confirm) |
| `Esc` | any sub-view | back to dashboard |

The selected card shows its currently available actions as words with the
key letter highlighted inside (Docker-sbx parity): `S`tart / `S`top /
`R`estart / E`x`ec / `Del`ete. Actions that don't apply to the sandbox's
current state (e.g. Start while running) are not offered; pressing their
key shows an explanatory status line instead.

## Port Publish/Unpublish — Implementation Detail

Microsandbox 0.7.2 binds published ports at VM boot time (the libkrun process opens
host-side TCP/UDP listeners). `SandboxModificationBuilder` cannot change ports or
network rules — only CPUs, memory, env, labels, secrets, and workdir.

**Recreate flow** (used by port publish/unpublish and network rule changes,
driven entirely through `SdkBackend`):

1. `inspect(name)` → read the full active config
2. Compute new config (add/remove port, add/remove rule)
3. `stop(name)`
4. `remove(name)` (0.7.2 has no create-with-replace; a name collision errors out)
5. `create(spec)` with all original settings plus the change (detached)
6. `start(name)` (detached)
7. Warn user: rootfs state resets on recreate. Volume data persists. To preserve
   rootfs state, snapshot first.

This is a known limitation. If the SDK gains live port / network-rule
modification in a future release, the TUI will switch to live modification
without recreation.

## Default Network Profile

The default profile for new sandboxes is **public** (`NetworkProfile::Public`):

- Outbound internet access allowed
- Private networks (LAN, loopback, link-local, metadata) blocked
- DNS through the sandbox gateway enabled automatically
- Inbound: published ports accessible from host (127.0.0.1 by default)

This matches microsandbox's own default and Docker Sandbox's default posture.

## File Structure

```
microsandbox-tui/
├── Cargo.toml
├── README.md
├── docs/
│   ├── DESIGN.md
│   └── PLAN.md
└── src/
    ├── main.rs              # CLI entry, app bootstrap, op dispatch
    ├── app.rs               # App state, view routing, event loop
    ├── event.rs             # crossterm input handling
    ├── backend/
    │   ├── mod.rs           # MsbBackend trait, CreateSpec, LogLine
    │   ├── sdk.rs           # SdkBackend (typed microsandbox 0.7.2 SDK)
    │   └── fake.rs          # in-memory FakeBackend for action-layer tests
    ├── runtime.rs           # msb version detect + install/update (banner)
    ├── models.rs            # SandboxSummary, SandboxConfig, Metrics, etc.
    ├── actions.rs           # high-level ops: create, stop, publish_port, etc.
    ├── fixtures/            # captured JSON for model-layer unit tests
    └── ui/
        ├── mod.rs           # render dispatch + status/banner lines
        ├── chrome.rs        # shared bands: title bar, tab bar, banner, footer
        ├── theme.rs         # Stitch palette (single color source)
        ├── dashboard.rs     # sandbox cards grid + runtime banner
        ├── create.rs        # create sandbox form (quick + advanced)
        ├── ports.rs         # host⇄guest port matrix + inspector
        ├── logs.rs          # logs streaming panel (toolbar + tints)
        └── help.rs          # keybindings overlay
```

Planned additions in later phases: `ui/inspect.rs`, `ui/network.rs`,
`ui/volumes.rs`, `ui/snapshots.rs`.

## MVP Scope (Phase 1)

1. Dashboard with live sandbox cards + metrics
2. Create sandbox form (image, name, cpus, memory, ports, volumes, env)
3. Stop / start / restart / remove sandbox
4. Exec command (non-interactive) + logs view (follow mode)
5. Port forwards view (read-only listing)

## Phase 2

6. Port publish/unpublish (recreate flow)
7. Network rules editor
8. SSH (suspend TUI, foreground `msb ssh`)
9. Volumes view (create/remove)
10. Snapshots view (create/restore/remove)
11. Templates (built-ins + user TOML files, master-detail create entry)
12. Create form rework (grouped full-screen form, `Ctrl+S` save-as-template)

## Phase 3

11. Image pull / remove management
12. Configuration file support (`--conf sandbox.yaml`)
13. Label-based filtering
14. Theme customization
15. Interactive exec (suspend TUI for `msb exec --` with TTY)