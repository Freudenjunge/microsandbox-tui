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
- **tokio** — async runtime for shelling out to `msb` concurrently
- **serde** / **serde_json** — parse `msb --format json` output
- **clap** — CLI args for the TUI itself (e.g. `msb-tui`, `msb-tui create`)

### Backend: `msb` CLI (not the Rust SDK)

The TUI drives the installed `msb` CLI via `tokio::process::Command` with
`--format json` output. This is cleaner than embedding the Rust SDK because:

1. The SDK's `download-binaries` feature would install its own `msb` + `libkrunfw`,
   conflicting with the user's existing installation
2. The CLI is the stable, versioned interface; the SDK re-exports internal crates
   that churn between minor versions
3. The CLI already has `--format json` on every command we need
4. No heavy compile-time dependencies (no libkrunfw linkage at build time)

### Prerequisites

- `msb` CLI installed and on `$PATH` (verified at startup, with a helpful error
  linking to the install script if missing)
- KVM enabled (Linux), Apple Silicon (macOS), or WHP (Windows)

## Views

### 1. Dashboard (default view)

```
┌─ microsandbox-tui ────────────────────────────────────────────────────┐
│  Sandboxes (3)                                  [c] Create  [q] Quit  │
│┌─────────────────────┐ ┌─────────────────────┐ ┌─────────────────────┐│
││ ● my-app     running│ │ ○ devbox     stopped│ │ ● worker     running││
││ python:3.12         │ │ ubuntu              │ │ alpine              ││
││ CPU 12%  MEM 240M   │ │ CPU --   MEM --     │ │ CPU 3%   MEM 56M    ││
││ Net ↓0  ↑1.2K       │ │                     │ │ Net ↓0   ↑0         ││
││ Ports 8080→80       │ │                     │ │ Ports —             ││
││ Uptime 5m 32s       │ │                     │ │ Uptime 12s          ││
│└─────────────────────┘ └─────────────────────┘ └─────────────────────┘│
│  [enter] inspect  [e] exec  [l] logs  [s] ssh  [p] ports  [n] network  │
│  [r] restart  [x] stop  [del] rm  [↑↓] select                         │
└────────────────────────────────────────────────────────────────────────┘
```

- Sandbox cards laid out in a responsive grid (2–3 columns depending on width)
- Each card: name, state indicator (● running / ○ stopped / ⏸ paused / ✗ exited),
  image, live CPU%, memory used, net I/O, published ports, uptime
- Polls `msb ls --format json` every 5s for sandbox list
- Polls `msb metrics --all --format json` every 1s for live stats
- Selected card highlighted; actions operate on selection

### 2. Create Sandbox form

```
┌─ Create Sandbox ──────────────────────────────────────────────────────┐
│                                                                       │
│  Image:   [python___________]  (autocomplete from `msb images`)       │
│  Name:    [my-sandbox_______]  (auto-generated if empty)              │
│  CPUs:    [1___]   Memory:    [512M____]                              │
│  Workdir: [/app______________]                                        │
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
│  [Tab] next field  [Enter] create  [Esc] cancel                       │
└────────────────────────────────────────────────────────────────────────┘
```

- Named by default (persistent) — ephemeral is an advanced toggle
- Image field autocompletes from `msb images --format json`
- Port entries use Docker syntax: `HOST:GUEST` or `BIND:HOST:GUEST`, with `/udp` suffix
- Network profile presets map to `--net` flags:
  - **public** → `--net public` (default: internet allowed, private blocked)
  - **private** → `--net private` (LAN/internal only)
  - **host** → `--net host` (host machine access)
  - **none** → `--no-net` (no network)
  - **custom** → user-defined `--net-rule` list + `--net-default`
- On confirm: runs `msb create --name <name> <flags> <image>` (detached, idle)
- Shows progress while pulling image (first pull can take time)

### 3. Port Forwards view

```
┌─ Ports: my-app ───────────────────────────────────────────────────────┐
│                                                                       │
│  HOST BIND    HOST PORT  GUEST PORT  PROTO                            │
│  127.0.0.1    8080       80          tcp                              │
│  0.0.0.0      9090       90          tcp                              │
│                                                                       │
│  [+] Publish port   [-] Unpublish   [Esc] back                        │
└────────────────────────────────────────────────────────────────────────┘
```

- Lists current published ports from `msb inspect --format json` → `network.ports[]`
- **Publish**: prompts for `HOST:GUEST` (or `BIND:HOST:GUEST`), then:
  1. Read current config via `msb inspect --format json`
  2. Stop the sandbox: `msb stop <name>`
  3. Remove it: `msb rm <name>`
  4. Recreate with old config + new port: `msb create --name <name> -p <new> ... <image>`
  5. Start: `msb start <name>`
  - ⚠️ Warns that recreation is required (microsandbox 0.7.2 can't modify ports live).
    Volume data survives; rootfs state resets unless snapshotted.
  - NOTE: 0.7.2 has no `create --replace` flag (name collisions always error), so
    recreate = remove + create with the same name.
- **Unpublish**: same recreate flow, minus the port
- Future: if `msb modify` gains `--port` support, switch to live modify

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

- Visual editor for `--net-rule` strings
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

- Lists volumes from `msb volumes --format json`
- Create: name, kind (dir/disk), size
- Remove: `msb volume rm <name>`
- Shows which sandboxes mount each volume (cross-referenced from `msb ls` + inspect)

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

- Streams `msb logs -f <name> --json` and renders lines with timestamps
- Toggle follow mode, grep filter, tail count, source filter (stdout/stderr/system)

### 7. Exec / SSH

- **Exec**: pops a command input at the bottom, runs `msb exec <name> -- <cmd>`,
  shows output inline. For interactive commands, suspends the TUI and runs `msb exec`
  in foreground (like `docker exec -it`).
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

- Create: `msb snapshot create --from-sandbox <name> [--full]`
- Restore: `msb restore <group>:<member> --name <new-name>`
- Remove: `msb snapshot rm <group>:<member>`

## App State & Event Loop

```
┌─────────────┐     ┌──────────────────┐     ┌────────────────┐
│  Input      │────▶│  App State       │◀────│  Background     │
│  (crossterm) │     │  (current view,  │     │  Pollers        │
└─────────────┘     │   selected sbx,  │     │  (msb ls,       │
                    │   form state)    │     │   msb metrics)  │
                    └────────┬─────────┘     └────────────────┘
                             │
                    ┌────────▼─────────┐
                    │  Render          │
                    │  (ratatui)       │
                    └──────────────────┘
```

- **Event loop**: `tokio::select!` over crossterm input events + metric poll ticks
- **Pollers**: 
  - Sandbox list: every 5s (`msb ls --format json`)
  - Metrics: every 1s (`msb metrics --all --format json`)
  - Volumes/images: on-demand when entering those views
- **Actions**: spawn `tokio::process::Command` for `msb` subcommands, parse JSON
  output, update state, re-render
- **Logs stream**: long-lived `msb logs -f --json` child process, lines sent over
  a `tokio::sync::mpsc` channel to the logs view

## Data Models

```rust
// From `msb ls --format json`
struct SandboxSummary {
    name: String,
    image: String,
    status: String,        // "Running", "Stopped", "Paused", "Exited"
    created_at: String,
}

// From `msb inspect --format json` (active_config subset)
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

// From `msb metrics --format json`
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

// From `msb volumes --format json`
struct Volume {
    name: String,
    kind: String,           // "dir" or "disk"
    capacity_bytes: Option<u64>,
    used_bytes: Option<u64>,
    created_at: String,
}

// From `msb images --format json`
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
| `Tab` | global | cycle views (dashboard → volumes → snapshots) |
| `?` | global | help overlay |
| `↑↓` | dashboard | select sandbox card |
| `Enter` | dashboard | inspect selected sandbox |
| `c` | dashboard | create sandbox form |
| `e` | dashboard | exec command in sandbox |
| `l` | dashboard | logs panel |
| `s` | dashboard | SSH into sandbox |
| `p` | dashboard | port forwards view |
| `n` | dashboard | network rules editor |
| `r` | dashboard | restart sandbox |
| `x` | dashboard | stop sandbox |
| `Delete` | dashboard | remove sandbox (confirm) |
| `f` | logs | toggle follow |
| `Esc` | any sub-view | back to dashboard |

## Port Publish/Unpublish — Implementation Detail

Microsandbox 0.7.2 binds published ports at VM boot time (the libkrun process opens
host-side TCP/UDP listeners). `msb modify` cannot change ports or network rules —
only CPUs, memory, env, labels, secrets, and workdir.

**Recreate flow** (used by port publish/unpublish and network rule changes):

1. `msb inspect <name> --format json` → read full `active_config`
2. Compute new config (add/remove port, add/remove rule)
3. `msb stop <name>`
4. `msb rm <name>` (0.7.2 has no `create --replace`; a name collision errors out)
5. Build `msb create --name <name>` command with all original flags plus the change
6. `msb start <name>`
7. Warn user: rootfs state resets on recreate. Volume data persists. To preserve
   rootfs state, snapshot first (`msb snapshot create --from-sandbox <name>`)

This is a known limitation. If `msb modify` gains `--port` / `--net-rule` in a
future release, the TUI will switch to live modification without recreation.

## Default Network Profile

The default profile for new sandboxes is **public** (`--net public`):

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
│   └── DESIGN.md
└── src/
    ├── main.rs              # CLI entry, app bootstrap
    ├── app.rs               # App state, view routing, event loop
    ├── event.rs             # crossterm input handling
    ├── backend/
    │   ├── mod.rs           # MsbBackend trait
    │   └── cli.rs           # tokio::process::Command impl
    ├── models.rs            # SandboxSummary, SandboxConfig, Metrics, etc.
    ├── actions.rs           # high-level ops: create, stop, publish_port, etc.
    └── ui/
        ├── mod.rs           # render dispatch
        ├── dashboard.rs     # sandbox cards grid
        ├── create.rs        # create sandbox form
        ├── ports.rs         # port forwards view
        ├── network.rs       # network rules editor
        ├── volumes.rs       # volume manager
        ├── logs.rs          # logs streaming panel
        ├── snapshots.rs     # snapshot manager
        ├── inspect.rs       # sandbox detail view
        └── help.rs          # keybindings overlay
```

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

## Phase 3

11. Image pull / remove management
12. Configuration file support (`--conf sandbox.yaml`)
13. Label-based filtering
14. Theme customization
15. Interactive exec (suspend TUI for `msb exec --` with TTY)