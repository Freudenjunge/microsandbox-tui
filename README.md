<div align="center">

<h1>msb-tui</h1>

A Docker `sbx`-style terminal dashboard for [microsandbox](https://github.com/superradcompany/microsandbox).

[![CI](https://github.com/Freudenjunge/microsandbox-tui/actions/workflows/ci.yml/badge.svg)](https://github.com/Freudenjunge/microsandbox-tui/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/Freudenjunge/microsandbox-tui)](https://github.com/Freudenjunge/microsandbox-tui/releases/latest)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)

</div>

## Why

microsandbox sandboxes are real microVMs — isolated, fast, disposable — but driving them by
CLI means memorizing a dozen flags and incantations. **msb-tui** puts a live dashboard in
your terminal: create sandboxes from templates, start, stop and restart them, exec a shell,
stream logs, and publish ports, all without leaving your flow. It talks to microsandbox
through the typed Rust SDK — never by scraping CLI output — so what the dashboard shows is
what the API knows.

## Features

- **Live sandbox rail** — cards with status, image, CPU gauge, memory, net I/O rates,
  uptime, and mounts, refreshed as metrics arrive.
- **Per-sandbox detail tabs** — Overview (live metrics and this sandbox's ports), Logs,
  and Ports, one keystroke deep.
- **20 built-in templates** — a bare Shell preset plus the 19-entry sbx Hub catalog of
  popular AI agent harnesses, ready to launch with `Enter`.
- **User-defined TOML templates** — save any form as a template with `Ctrl+S`; files in
  `~/.config/microsandbox-tui/templates/` shadow built-ins by id.
- **Snapshot-preserving port publishing** — publish/unpublish recreates the sandbox
  through a snapshot/restore cycle, so your data survives the change.
- **In-TUI runtime install** — no `msb` binary at startup? The banner offers a one-key
  install/update of the microsandbox runtime.
- **Polished dark theme** — responsive layout from small terminals to wide desktops.

## Screenshots

> 📸 Captures coming soon — the TUI in action:
>
> | Capture | What it shows |
> |---|---|
> | dashboard | sandbox rail + detail tabs |
> | create | template picker |
> | ports | ports tab |
>
> PRs adding captures under `docs/img/` are welcome.

## Requirements

| OS | Architecture | Virtualization | Notes |
|---|---|---|---|
| Linux | x86_64, ARM64 | KVM | recommended path |
| macOS | Apple Silicon | Hypervisor.framework | Intel Macs are unsupported (a microsandbox limitation) |
| Windows 11 | x64, ARM64 | Windows Hypervisor Platform | preview, per the microsandbox docs |

The `msb` runtime is **not** required at startup. If no runtime is detected, the dashboard
shows a banner — press `U` to install or update microsandbox from inside the TUI.

## Installation

### Prebuilt binary (Linux/macOS)

Grab a tarball from [GitHub Releases](https://github.com/Freudenjunge/microsandbox-tui/releases/latest).
Every release ships one archive per target, each with a `.sha256` sibling:

| Platform | Target triplet | Asset |
|---|---|---|
| Linux x86_64 | `x86_64-unknown-linux-gnu` | `msb-tui-v0.1.0-x86_64-unknown-linux-gnu.tar.gz` |
| Linux ARM64 | `aarch64-unknown-linux-gnu` | `msb-tui-v0.1.0-aarch64-unknown-linux-gnu.tar.gz` |
| macOS Apple Silicon | `aarch64-apple-darwin` | `msb-tui-v0.1.0-aarch64-apple-darwin.tar.gz` |

Example install on Linux x86_64:

```sh
curl -fsSLO https://github.com/Freudenjunge/microsandbox-tui/releases/download/v0.1.0/msb-tui-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
curl -fsSLO https://github.com/Freudenjunge/microsandbox-tui/releases/download/v0.1.0/msb-tui-v0.1.0-x86_64-unknown-linux-gnu.tar.gz.sha256
sha256sum --check msb-tui-v0.1.0-x86_64-unknown-linux-gnu.tar.gz.sha256   # shasum -a 256 -c on macOS
tar -xzf msb-tui-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
sudo install -m 0755 msb-tui-v0.1.0-x86_64-unknown-linux-gnu/msb-tui /usr/local/bin/
```

### From the repository

```sh
cargo install --git https://github.com/Freudenjunge/microsandbox-tui.git
```

This builds and installs the `msb-tui` command from the default branch.

### From source

```sh
git clone https://github.com/Freudenjunge/microsandbox-tui.git
cd microsandbox-tui
cargo install --path .
```

## Usage

```sh
msb-tui            # open the dashboard
msb-tui --create   # jump straight into the create form
```

On first run, the dashboard checks for an `msb` runtime on your PATH. If none is found, a
banner explains the situation and offers `U` to install the matching microsandbox runtime
in the background — the TUI stays responsive while it downloads.

A typical round trip:

1. Press `c` — the template picker opens, "Create from scratch" on top and the catalog
   below.
2. Pick a template with `↑/↓` and hit `Enter` to launch it directly (or `e` to tweak the
   form first).
3. Watch the new card appear in the rail — status, CPU, memory, and net rates go live as
   the sandbox boots.
4. `2` opens the Logs tab and streams its output; `3` opens Ports and publishes a binding
   with `+`.
5. `x` opens an interactive shell for the selected sandbox in a new terminal window, with
   the dashboard running in parallel.

## Keybindings

### Global

| Key | Action |
|---|---|
| `q` | Quit |
| `?` | Help overlay |

### Dashboard

| Key | Action |
|---|---|
| `↑`/`↓` | Select sandbox card |
| `1`/`2`/`3` | Detail tabs: overview / logs / ports |
| `Tab` | Cycle detail tabs |
| `c` | Create sandbox |
| `x` | Exec — interactive shell in a new terminal window |
| `s` | Start/stop toggle (state-dependent) |
| `r` | Restart |
| `Del` | Remove sandbox (confirm) |
| `U` | Install/update the microsandbox runtime |

### Logs tab

| Key | Action |
|---|---|
| `f` | Toggle follow |
| `g` | Grep filter (enters grep input mode) |
| `s` | Source filter (stdout / stderr / system) |
| `↑↓` / `PgUp` / `PgDn` | Scroll |

### Ports tab

| Key | Action |
|---|---|
| `+` or `p` | Publish a port (form → confirm; recreates via snapshot/restore) |
| `-` or `_` | Unpublish the selected binding (confirm) |
| `↑`/`↓` | Select binding |
| `Esc` / `q` / `1` | Back to overview |

### Create flow

| Key | Action |
|---|---|
| `↑`/`↓` | Navigate (row 0 = "Create from scratch") |
| `Enter` | Create from the selected template directly |
| `e` | Open the form prefilled from the template |
| `n` | Open the empty form |
| `d` | Delete a user template (confirm) |
| `Ctrl+S` | Save the current form as a template |
| `Esc` | Back to the dashboard |

## Templates

The create flow opens on a picker with 20 built-in templates: a bare Shell preset plus 19
AI agent harnesses from the sbx org on Docker Hub. Each catalog entry is a
community-maintained reference to the public `sbx/<name>-image` base image — entries
launch with 2 vCPUs, 4G of memory, and your current directory mounted.

| Template | Image | Description |
|---|---|---|
| Shell | `alpine` | Bare sandbox with CWD mount — generic starting point |
| Pi | `sbx/pi-image` | Pi coding agent |
| Hermes Agent | `sbx/hermes-agent-image` | The self-improving AI agent by Nous Research |
| OpenClaw | `sbx/openclaw-image` | Personal AI assistant with multi-platform chat |
| Kiro | `sbx/kiro-image` | Kiro CLI by AWS with interactive device-flow auth |
| Crush | `sbx/crush-image` | Multi-provider AI coding agent from Charm |
| Junie | `sbx/junie-image` | The AI coding agent by JetBrains |
| GitHub Copilot | `sbx/copilot-image` | GitHub Copilot CLI, GitHub's agentic coding CLI |
| Aider | `sbx/aider-image` | AI pair programming in your terminal |
| Droid | `sbx/droid-image` | Droid CLI by Factory, an agentic coding CLI |
| Open Interpreter | `sbx/open-interpreter-image` | Let language models run code on your computer |
| OpenHands | `sbx/openhands-image` | AI software engineer by All Hands AI |
| Antigravity | `sbx/antigravity-image` | Google's agent-first dev platform — the agy terminal agent |
| Vibe | `sbx/vibe-image` | Mistral AI's open source coding agent |
| Docker Agent | `sbx/docker-agent-image` | Docker's multi-provider agentic coding CLI |
| Claude Code | `sbx/claude-image` | Anthropic's Claude Code, resolved by the sandbox proxy |
| OpenCode | `sbx/opencode-image` | OpenCode, the open-source terminal coding agent |
| Codex | `sbx/codex-image` | OpenAI Codex CLI, OpenAI's agentic coding CLI |
| Cursor | `sbx/cursor-image` | Cursor Agent, the CLI coding agent from Cursor |
| Devin | `sbx/devin-image` | Devin CLI by Cognition, resolved by the sandbox proxy |

**Your own templates.** Save any create form with `Ctrl+S`, or drop a TOML file into
`~/.config/microsandbox-tui/templates/<id>.toml` — a user file with the same id as a
built-in shadows it, so every entry above can be customized. Example:

```toml
[meta]
name = "My OpenCode box"
description = "OpenCode with a bigger brain"

[spec]
image = "sbx/opencode-image"
name = "my-opencode"
cpus = 4
memory = "8G"
mount_cwd = true
ports = ["127.0.0.1:4096:4096"]
```

All `[spec]` keys are optional except `image`. Two conventions worth knowing:

- `mount_cwd = true` (the default) follows the sbx workspace convention: the TUI's current
  directory is bind-mounted into the sandbox at the same absolute path and becomes the
  default working directory, so paths and stack traces line up between host and guest.
- `net_profile` selects the network preset: `public` (the default — internet allowed,
  private networks blocked), `private` (LAN/internal only), or `host` (host machine
  access).

## How it works

msb-tui talks to microsandbox exclusively through the typed `microsandbox` 0.7.2 Rust
SDK — there is no CLI scraping and no guessed JSON. Sandboxes are created and started
detached, so they survive the TUI process; close the dashboard, reopen it, and your fleet
is still running. Port changes ride a snapshot-preserving recreate: the sandbox disk is
snapshotted, the sandbox is stopped and removed, then restored with the new port set and
its configuration reapplied — your data survives the change. The full architecture and
design decisions live in [docs/DESIGN.md](docs/DESIGN.md).

## Development

All changes run the four gates:

```sh
cargo fmt --check
cargo clippy -- -D warnings
cargo test
cargo check
```

The suite currently has 276 passing unit tests. Release planning and task history are
tracked in [docs/PLAN.md](docs/PLAN.md).

## License

Apache-2.0 — see [LICENSE](LICENSE).

microsandbox-tui is an independent community project. It is not affiliated with, endorsed by, or sponsored by Superrad Company (microsandbox), Docker (the `sbx` TUI and the sbx Hub org), or the maintainers of the products shipped as templates (Claude Code, OpenCode, Aider, Cursor, Codex, and others). Product names and trademarks belong to their respective owners and are used here only to describe what this tool manages.

## Acknowledgements

Inspired by Docker's `sbx` TUI; built on [microsandbox](https://github.com/superradcompany/microsandbox).
